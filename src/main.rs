use std::{
    str::FromStr,
    sync::{atomic::AtomicBool, Arc, Mutex},
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
use cached::proc_macro::cached;
use clap::{command, Parser};
use ore::{self, Metadata, Miner, BUS, DIFFICULTY, EPOCH_DURATION, METADATA, MINER};
use rand::Rng;
use rayon::prelude::*;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    clock::Clock,
    commitment_config::CommitmentConfig,
    hash::{hashv, Hash},
    signature::{read_keypair_file, Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};

struct MinerH<'a> {
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
    let cluster: Cluster = Cluster::Custom(args.rpc_url, args.rpc_ws_url);
    let keypair = read_keypair_file(args.identity.clone()).unwrap();

    // Initialize Ore program, if needed.
    initialize_program(cluster.clone(), args.identity.clone()).await;

    // Initialize miner account, if needed.
    initialize_miner_account(cluster.clone(), args.identity.clone()).await;

    // Sync local state with on-chain data.
    let metadata = get_metadata(cluster.clone()).await;
    let miner_acc = get_miner_account(cluster.clone(), keypair.pubkey()).await;
    let mint = metadata.mint;

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
    let miner = Arc::new(MinerH::new(cluster.clone(), &keypair, beneficiary, mint));

    // Start mining.
    miner.mine(cluster, miner_acc.hash).await;
}

impl<'a> MinerH<'a> {
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

    pub async fn mine(&self, cluster: Cluster, hash: Hash) {
        let mut hash = hash;
        let miner_address = miner_pubkey(self.keypair.pubkey());
        let mut rng = rand::thread_rng();
        loop {
            // Find a valid hash.
            let (next_hash, nonce) = self.find_next_hash_par(hash);
            println!(
                "Found a valid hash {:?} nonce: {:?}",
                next_hash.clone(),
                nonce
            );

            // Check if epoch needs to be reset
            let metadata = get_metadata(cluster.clone()).await;
            let clock = get_clock_account(cluster.clone()).await;
            let epoch_end_at = metadata.epoch_start_at.saturating_add(EPOCH_DURATION);
            if clock.unix_timestamp.ge(&epoch_end_at) {
                // Submit restart epoch tx.
                let sig = self
                    .ore()
                    .request()
                    .instruction(Instruction {
                        program_id: ore::ID,
                        accounts: (ore::accounts::StartEpoch {
                            signer: self.keypair.pubkey(),
                            metadata: metadata_pubkey(),
                            bus_0: bus_pubkey(0),
                            bus_1: bus_pubkey(1),
                            bus_2: bus_pubkey(2),
                            bus_3: bus_pubkey(3),
                            bus_4: bus_pubkey(4),
                            bus_5: bus_pubkey(5),
                            bus_6: bus_pubkey(6),
                            bus_7: bus_pubkey(7),
                        })
                        .to_account_metas(Some(false)),
                        data: ore::instruction::StartEpoch {}.data(),
                    })
                    .signer(self.keypair)
                    .send()
                    .await
                    .expect("Failed to submit transaction");
                println!("Sig: {}", sig);
            }

            // Submit mine tx.
            let sig = self
                .ore()
                .request()
                .instruction(Instruction {
                    program_id: ore::ID,
                    accounts: (ore::accounts::Mine {
                        signer: self.keypair.pubkey(),
                        beneficiary: self.beneficiary,
                        miner: miner_address,
                        metadata: metadata_pubkey(),
                        mint: self.mint,
                        token_program: anchor_spl::token::ID,
                        bus: bus_pubkey(rng.gen_range(0..8)),
                    })
                    .to_account_metas(Some(false)),
                    data: ore::instruction::Mine {
                        hash: next_hash.clone(),
                        nonce,
                    }
                    .data(),
                })
                .signer(self.keypair)
                .send()
                .await
                .expect("Failed to submit transaction");
            println!("Sig: {}", sig);
            hash = next_hash.clone();
        }
    }

    fn find_next_hash(&self, hash: Hash) -> (Hash, u64) {
        let mut next_hash: Hash;
        let mut nonce = 0u64;
        loop {
            let b = [
                hash.to_bytes().as_slice(),
                self.keypair.pubkey().to_bytes().as_slice(),
                nonce.to_be_bytes().as_slice(),
            ]
            .concat();
            next_hash = hashv(&[&b]);
            if next_hash.le(&DIFFICULTY) {
                break;
            } else {
                println!("Invalid hash: {} Nonce: {:?}", next_hash.to_string(), nonce);
            }
            nonce += 1;
        }
        (next_hash, nonce)
    }

    fn find_next_hash_par(&self, hash: Hash) -> (Hash, u64) {
        let found_solution = Arc::new(AtomicBool::new(false));
        let solution = Arc::new(Mutex::<(Hash, u64)>::new((
            Hash::new_from_array([0; 32]),
            0,
        )));
        let seed = u64::MAX.saturating_div(4);
        let data = [0, seed, seed.saturating_mul(2), seed.saturating_mul(3)];
        println!("Searching for a valid hash...");
        data.par_iter().for_each(|n| {
            let mut next_hash: Hash;
            let mut nonce: u64 = *n;
            loop {
                if nonce % 10_000 == 0 {
                    if found_solution.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                }
                let b = [
                    hash.to_bytes().as_slice(),
                    self.keypair.pubkey().to_bytes().as_slice(),
                    nonce.to_be_bytes().as_slice(),
                ]
                .concat();
                next_hash = hashv(&[&b]);
                if next_hash.le(&DIFFICULTY) {
                    found_solution.store(true, std::sync::atomic::Ordering::Relaxed);
                    let mut w_solution = solution.lock().expect("failed to lock mutex");
                    *w_solution = (next_hash, nonce);
                    return;
                }
                nonce += 1;
            }
        });

        let r_solution = solution.lock().expect("Failed to get lock");
        *r_solution
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

pub async fn get_miner_account(cluster: Cluster, authority: Pubkey) -> Miner {
    let client =
        RpcClient::new_with_commitment(cluster.url().to_string(), CommitmentConfig::processed());
    let miner_address = miner_pubkey(authority);
    let data = client
        .get_account_data(&miner_address)
        .await
        .expect("Failed to get miner account");
    Miner::deserialize(&mut &data.as_slice()[8..]).expect("Failed to parse miner account")
}

pub async fn get_clock_account(cluster: Cluster) -> Clock {
    let client =
        RpcClient::new_with_commitment(cluster.url().to_string(), CommitmentConfig::processed());
    let data = client
        .get_account_data(&sysvar::clock::ID)
        .await
        .expect("Failed to get miner account");
    bincode::deserialize::<Clock>(&data).expect("Failed to deserialize clock")
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
    let ix_1 = Instruction {
        program_id: ore::ID,
        accounts: (ore::accounts::InitializeMetadata {
            signer: signer.pubkey(),
            metadata: metadata_pubkey(),
            mint: mint.pubkey(),
            rent: sysvar::rent::ID,
            system_program: system_program::ID,
            token_program: anchor_spl::token::ID,
        })
        .to_account_metas(Some(false)),
        data: ore::instruction::InitializeMetadata {}.data(),
    };
    let ix_2 = Instruction {
        program_id: ore::ID,
        accounts: (ore::accounts::InitializeBusses {
            signer: signer.pubkey(),
            system_program: system_program::ID,
            bus_0: bus_pubkey(0),
            bus_1: bus_pubkey(1),
            bus_2: bus_pubkey(2),
            bus_3: bus_pubkey(3),
            bus_4: bus_pubkey(4),
            bus_5: bus_pubkey(5),
            bus_6: bus_pubkey(6),
            bus_7: bus_pubkey(7),
        })
        .to_account_metas(Some(false)),
        data: ore::instruction::InitializeBusses {}.data(),
    };

    // Sign and send transaction.
    let mut transaction = Transaction::new_with_payer(&[ix_1, ix_2], Some(&signer.pubkey()));
    let recent_blockhash = client.get_latest_blockhash().await.unwrap();
    transaction.sign(&[&signer, &mint], recent_blockhash);
    let result = client.send_and_confirm_transaction(&transaction).await;
    match result {
        Ok(signature) => println!("Transaction successful with signature: {:?}", signature),
        Err(e) => println!("Transaction failed: {:?}", e),
    }
}

async fn initialize_miner_account(cluster: Cluster, keypair_filepath: String) {
    // Return early if program is initialized
    let signer = read_keypair_file(keypair_filepath).unwrap();
    let miner_address = miner_pubkey(signer.pubkey());
    let client =
        RpcClient::new_with_commitment(cluster.url().to_string(), CommitmentConfig::processed());
    if client.get_account(&miner_address).await.is_ok() {
        return;
    }

    // Build instructions.
    let ix = Instruction {
        program_id: ore::ID,
        accounts: (ore::accounts::RegisterMiner {
            signer: signer.pubkey(),
            miner: miner_address,
            system_program: system_program::ID,
        })
        .to_account_metas(Some(false)),
        data: ore::instruction::RegisterMiner {}.data(),
    };

    // Sign and send transaction.
    let mut transaction = Transaction::new_with_payer(&[ix], Some(&signer.pubkey()));
    let recent_blockhash = client.get_latest_blockhash().await.unwrap();
    transaction.sign(&[&signer], recent_blockhash);
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

#[cached]
fn miner_pubkey(authority: Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[MINER, authority.as_ref()], &ore::ID).0
}

#[cached]
fn bus_pubkey(id: u8) -> Pubkey {
    Pubkey::find_program_address(&[BUS, &[id]], &ore::ID).0
}

// const PROGRAM_DATA: &str = "Program data: ";

// Subscribe to streaming program logs.
// runtime.spawn(async move {
//     let sub_client = PubsubClient::new(cluster.ws_url())
//         .await
//         .expect("Failed to create pubsub client");

//     let (mut notifications, _unsubscribe) = sub_client
//         .logs_subscribe(
//             solana_client::rpc_config::RpcTransactionLogsFilter::Mentions(vec![
//                 ore::ID.to_string()
//             ]),
//             solana_client::rpc_config::RpcTransactionLogsConfig {
//                 commitment: Some(CommitmentConfig::processed()),
//             },
//         )
//         .await
//         .expect("Failed to subscribe");

//     while let Some(logs) = notifications.next().await {
//         for log in &logs.value.logs[..] {
//             if let Some(log) = log.strip_prefix(PROGRAM_DATA) {
//                 if let Ok(borsh_bytes) = base64::engine::general_purpose::STANDARD.decode(log) {
//                     let mut slice = &borsh_bytes[8..];
//                     if let Ok(e) = MineEvent::deserialize(&mut slice) {
//                         tx.send(e).expect("Failed to send event");
//                     }
//                 }
//             }
//         }
//     }
// });

// Sync local state with program logs.
// runtime.spawn({
//     // let flag = flag.clone();
//     // let hash = hash.clone();
//     // let difficulty = difficulty.clone();
//     async move {
//         while let Some(v) = rx.recv().await {
//             // let mut w_hash = hash.write().expect("Failed to acquire write lock");
//             // let mut w_difficulty = difficulty.write().expect("Failed to acquire write lock");
//             // *w_hash = v.hash;
//             // *w_difficulty = v.difficulty;
//             // flag.store(true, Ordering::Relaxed);
//         }
//     }
// });
