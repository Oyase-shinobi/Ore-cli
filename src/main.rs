use std::{
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
};

use anchor_client::{
    anchor_lang::{
        prelude::Pubkey,
        solana_program::{instruction::Instruction, sysvar},
        system_program, AnchorDeserialize, InstructionData, ToAccountMetas,
    },
    Client, Cluster, Program,
};
use anchor_spl::token::{spl_token, TokenAccount};
use base64::Engine;
use bnum::types::U256;
use cached::proc_macro::cached;
use clap::{command, Parser};
use futures::StreamExt;
use ore::{self, Metadata, MineEvent, METADATA, RADIX};
use solana_client::nonblocking::{pubsub_client::PubsubClient, rpc_client::RpcClient};
use solana_sdk::{
    commitment_config::CommitmentConfig,
    signature::{read_keypair_file, Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};
use tokio::runtime::Runtime;

// const KEYPAIR: &str = "/home/ubuntu/.config/solana/id.json";
// const MINT: &str = "/home/ubuntu/.config/solana/ore-mint.json";
const PROGRAM_DATA: &str = "Program data: ";

struct Miner<'a> {
    pub keypair: &'a Keypair,
    pub beneficiary: Pubkey,
    pub mint: Pubkey,
    pub cluster: Cluster,
}

#[derive(Parser, Debug)]
#[command(about, version)]
struct Args {
    #[arg(
        long,
        value_name = "KEYPAIR_FILEPATH",
        help = "Identity keypair to sign transactions"
    )]
    identity: String,

    #[arg(
        long,
        value_name = "TOKEN_ACCOUNT_ADDRESS",
        help = "Token account to receive mining rewards."
    )]
    beneficiary: Option<String>,

    #[arg(
        long,
        value_name = "URL",
        help = "URL to connect to Solana cluster",
        default_value = "https://api.mainnet-beta.solana.com"
    )]
    rpc_url: String,

    #[arg(
        long,
        value_name = "WEBSOCKET_URL",
        help = "Websocket URL to stream data from Solana cluster",
        default_value = "wss://api.mainnet-beta.solana.com/"
    )]
    rpc_ws_url: String,
}

#[tokio::main]
async fn main() {
    // Initialize runtime.
    let args = Args::parse();
    let runtime = Runtime::new().unwrap();
    let cluster: Cluster = Cluster::Custom(args.rpc_url, args.rpc_ws_url);
    let keypair = read_keypair_file(args.identity.clone()).unwrap();

    // Initialize Ore program, if needed.
    initialize_program(cluster.clone(), args.identity.clone()).await;

    // Sync local state with on-chain data.
    let metadata = get_metadata(cluster.clone()).await;
    let mint = metadata.mint;
    let hash = Arc::new(RwLock::new(metadata.hash));
    let difficulty = Arc::new(RwLock::new(metadata.difficulty));
    let flag = Arc::new(AtomicBool::new(true));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<MineEvent>();

    // Initialize beneficiary token account.
    let beneficiary = match args.beneficiary {
        Some(beneficiary) => {
            Pubkey::from_str(&beneficiary).expect("Failed to parse beneficiary address")
        }
        None => {
            initialize_token_account(cluster.clone(), args.identity, keypair.pubkey(), mint).await
        }
    };

    // Initalize miner
    let miner = Arc::new(Miner::new(cluster.clone(), &keypair, beneficiary, mint));

    // Subscribe to streaming program logs.
    runtime.spawn(async move {
        let sub_client = PubsubClient::new(cluster.ws_url())
            .await
            .expect("Failed to create pubsub client");

        let (mut notifications, _unsubscribe) = sub_client
            .logs_subscribe(
                solana_client::rpc_config::RpcTransactionLogsFilter::Mentions(vec![
                    ore::ID.to_string()
                ]),
                solana_client::rpc_config::RpcTransactionLogsConfig {
                    commitment: Some(CommitmentConfig::processed()),
                },
            )
            .await
            .expect("Failed to subscribe");

        while let Some(logs) = notifications.next().await {
            for log in &logs.value.logs[..] {
                if let Some(log) = log.strip_prefix(PROGRAM_DATA) {
                    if let Ok(borsh_bytes) = base64::engine::general_purpose::STANDARD.decode(log) {
                        let mut slice = &borsh_bytes[8..];
                        if let Ok(e) = MineEvent::deserialize(&mut slice) {
                            tx.send(e).expect("Failed to send event");
                        }
                    }
                }
            }
        }
    });

    // Sync local state with program logs.
    runtime.spawn({
        let flag = flag.clone();
        let hash = hash.clone();
        let difficulty = difficulty.clone();
        async move {
            while let Some(v) = rx.recv().await {
                let mut w_hash = hash.write().expect("Failed to acquire write lock");
                let mut w_difficulty = difficulty.write().expect("Failed to acquire write lock");
                *w_hash = v.hash;
                *w_difficulty = v.difficulty;
                flag.store(true, Ordering::Relaxed);
            }
        }
    });

    // Start mining.
    miner.mine(hash, difficulty, flag).await;
}

impl<'a> Miner<'a> {
    pub fn new(cluster: Cluster, keypair: &'a Keypair, beneficiary: Pubkey, mint: Pubkey) -> Self {
        Self {
            keypair,
            beneficiary,
            mint,
            cluster,
        }
    }

    pub fn client(&self) -> Client<&'a Keypair> {
        Client::new_with_options(
            self.cluster.clone(),
            self.keypair,
            CommitmentConfig::processed(),
        )
    }

    pub fn ore(&self) -> Program<&'a Keypair> {
        self.client()
            .program(ore::ID)
            .expect("Failed to get Ore program")
    }

    pub async fn mine(
        &self,
        hash: Arc<RwLock<String>>,
        difficulty: Arc<RwLock<String>>,
        flag: Arc<AtomicBool>,
    ) {
        loop {
            // Find a valid hash.
            let (next_hash, nonce) =
                self.find_next_hash(hash.clone(), difficulty.clone(), flag.clone());
            println!("Got hash {:?} nonce: {:?}", next_hash, nonce);

            // Submit mine tx.
            let sig = self
                .ore()
                .request()
                .instruction(Instruction {
                    program_id: ore::ID,
                    accounts: (ore::accounts::Mine {
                        signer: self.keypair.pubkey(),
                        beneficiary: self.beneficiary,
                        metadata: metadata_pubkey(),
                        mint: self.mint,
                        token_program: anchor_spl::token::ID,
                    })
                    .to_account_metas(Some(false)),
                    data: ore::instruction::Mine {
                        hash: next_hash,
                        nonce,
                    }
                    .data(),
                })
                .signer(self.keypair)
                .send()
                .await
                .expect("Failed to submit transaction");
            println!("Sig: {}", sig);
        }
    }

    fn find_next_hash(
        &self,
        hash: Arc<RwLock<String>>,
        difficulty: Arc<RwLock<String>>,
        flag: Arc<AtomicBool>,
    ) -> (String, u64) {
        let mut difficulty_ = U256::MAX;
        let mut hash_ = String::new();
        let mut next_hash: String;
        let mut nonce = 0;
        loop {
            // If flag is set, refetch difficulty and hash values.
            // Check every 10_000 hashes.
            if nonce % 10_000 == 0 {
                if flag.load(Ordering::Relaxed) {
                    let r_difficulty = difficulty.read().expect("Failed to acquire read lock");
                    let r_hash = hash.read().expect("Failed to acquire read lock");
                    difficulty_ = U256::parse_str_radix(&*r_difficulty, RADIX);
                    hash_ = r_hash.clone();
                    drop(r_difficulty);
                    drop(r_hash);
                    nonce = 0;
                    flag.store(false, Ordering::Relaxed);
                }
            }

            // Search for valid hashes
            let msg = format!("{}-{}-{}", hash_, self.keypair.pubkey(), nonce);
            next_hash = sha256::digest(msg);
            let next_hash_ = U256::parse_str_radix(&next_hash, RADIX);
            if next_hash_.le(&difficulty_) {
                break;
            } else {
                println!("Invalid hash: {:?} Nonce: {:?}", next_hash, nonce);
            }
            nonce += 1;
        }
        (next_hash, nonce)
    }
}

pub async fn get_metadata(cluster: Cluster) -> Metadata {
    let client =
        RpcClient::new_with_commitment(cluster.url().to_string(), CommitmentConfig::processed());
    let data = client
        .get_account_data(&metadata_pubkey())
        .await
        .expect("Failed to get metadata account");
    Metadata::deserialize(&mut &data.as_slice()[8..]).expect("Failed to parse metadata account")
}

async fn initialize_program(cluster: Cluster, keypair_filepath: String) {
    // Return early if program is initialized
    let client =
        RpcClient::new_with_commitment(cluster.url().to_string(), CommitmentConfig::processed());
    if client.get_account(&metadata_pubkey()).await.is_ok() {
        return;
    }

    // Build instructions.
    let mint = Keypair::new();
    let signer = read_keypair_file(keypair_filepath).unwrap();
    let ix = Instruction {
        program_id: ore::ID,
        accounts: (ore::accounts::Initialize {
            signer: signer.pubkey(),
            metadata: metadata_pubkey(),
            mint: mint.pubkey(),
            rent: sysvar::rent::ID,
            system_program: system_program::ID,
            token_program: anchor_spl::token::ID,
        })
        .to_account_metas(Some(false)),
        data: ore::instruction::Initialize {}.data(),
    };

    // Sign and send transaction.
    let mut transaction = Transaction::new_with_payer(&[ix], Some(&signer.pubkey()));
    let recent_blockhash = client.get_latest_blockhash().await.unwrap();
    transaction.sign(&[&signer, &mint], recent_blockhash);
    let result = client.send_and_confirm_transaction(&transaction).await;
    match result {
        Ok(signature) => println!("Transaction successful with signature: {:?}", signature),
        Err(e) => println!("Transaction failed: {:?}", e),
    }
}

async fn initialize_token_account(
    cluster: Cluster,
    keypair_filepath: String,
    authority: Pubkey,
    mint: Pubkey,
) -> Pubkey {
    // Initialize client.
    let signer = read_keypair_file(keypair_filepath).unwrap();
    let client =
        RpcClient::new_with_commitment(cluster.url().to_string(), CommitmentConfig::processed());

    // Build instructions.
    let token_account_keypair = Keypair::new();
    let token_account_pubkey = token_account_keypair.pubkey();
    let rent = client
        .get_minimum_balance_for_rent_exemption(TokenAccount::LEN)
        .await
        .unwrap();
    let create_account_instruction = system_instruction::create_account(
        &signer.pubkey(),
        &token_account_pubkey,
        rent,
        TokenAccount::LEN as u64,
        &spl_token::id(),
    );
    let initialize_account_instruction = spl_token::instruction::initialize_account(
        &spl_token::id(),
        &token_account_pubkey,
        &mint,
        &authority,
    )
    .unwrap();

    // Sign and send transaction.
    let mut transaction = Transaction::new_with_payer(
        &[create_account_instruction, initialize_account_instruction],
        Some(&signer.pubkey()),
    );
    let recent_blockhash = client.get_latest_blockhash().await.unwrap();
    transaction.sign(&[&signer, &token_account_keypair], recent_blockhash);
    let result = client.send_and_confirm_transaction(&transaction).await;
    match result {
        Ok(signature) => println!("Transaction successful with signature: {:?}", signature),
        Err(e) => println!("Transaction failed: {:?}", e),
    }

    // Return token account address
    token_account_pubkey
}

#[cached]
fn metadata_pubkey() -> Pubkey {
    Pubkey::find_program_address(&[METADATA], &ore::ID).0
}
