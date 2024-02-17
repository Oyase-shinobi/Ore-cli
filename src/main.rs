use std::{
    str::FromStr,
    sync::{atomic::AtomicBool, Arc, Mutex},
};

use cached::proc_macro::cached;
use clap::{command, Parser, Subcommand};
use ore::{
    self,
    state::{Proof, Treasury},
    utils::AccountDeserialize,
    BUS_ADDRESSES, BUS_COUNT, EPOCH_DURATION, MINT_ADDRESS, PROOF, TREASURY_ADDRESS,
};
use solana_client::{client_error::ClientErrorKind, nonblocking::rpc_client::RpcClient};
use solana_program::{program_pack::Pack, pubkey::Pubkey, sysvar};
use solana_sdk::{
    clock::Clock,
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    keccak::{hashv, Hash as KeccakHash},
    signature::{read_keypair_file, Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};
use spl_associated_token_account::get_associated_token_address;
use spl_token::state::Account as TokenAccount;

const NUM_THREADS: u64 = 6;

struct Miner<'a> {
    pub keypair: &'a Keypair,
    pub cluster: String,
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
        value_name = "URL",
        help = "URL to connect to Solana cluster",
        default_value = "https://api.mainnet-beta.solana.com"
    )]
    rpc_url: String,

    // #[arg(
    //     long,
    //     value_name = "WEBSOCKET_URL",
    //     help = "Websocket URL to stream data from Solana cluster",
    //     default_value = "wss://api.mainnet-beta.solana.com/"
    // )]
    // rpc_ws_url: String,

    // Subcommands
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    #[command(about = "Use your local computer to mine Ore")]
    Mine(Mine),

    #[command(about = "Claim available mining rewards")]
    Claim(Claim),
}

// Arguments specific to the foo subcommand
#[derive(Parser, Debug)]
struct Mine {
    // TODO Thread count
}

// Arguments specific to the bar subcommand
#[derive(Parser, Debug)]
struct Claim {
    #[arg(
        long,
        value_name = "AMOUNT",
        help = "The amount of rewards to claim. Defaults to max."
    )]
    amount: Option<u64>,

    #[arg(
        long,
        value_name = "TOKEN_ACCOUNT_ADDRESS",
        help = "Token account to receive mining rewards."
    )]
    beneficiary: Option<String>,
}

#[tokio::main]
async fn main() {
    // Initialize miner.
    let args = Args::parse();
    let cluster = args.rpc_url;
    let keypair = read_keypair_file(args.identity.clone()).unwrap();
    let miner = Arc::new(Miner::new(cluster.clone(), &keypair));

    // Initialize Ore program, if needed.
    initialize_program(cluster.clone(), args.identity.clone()).await;

    // Initialize proof account, if needed.
    initialize_proof_account(cluster.clone(), args.identity.clone()).await;

    // Execute user command.
    match args.command {
        Commands::Mine(_cmd) => {
            miner.mine().await;
        }
        Commands::Claim(cmd) => {
            let beneficiary = match cmd.beneficiary {
                Some(beneficiary) => {
                    Pubkey::from_str(&beneficiary).expect("Failed to parse beneficiary address")
                }
                None => {
                    initialize_token_account(cluster.clone(), args.identity, keypair.pubkey()).await
                }
            };
            // TODO Amount
            miner.claim(cluster, beneficiary, 0).await;
        }
    }
}

impl<'a> Miner<'a> {
    pub fn new(cluster: String, keypair: &'a Keypair) -> Self {
        Self { keypair, cluster }
    }

    pub async fn mine(&self) {
        loop {
            // Find a valid hash.
            let treasury = get_treasury(self.cluster.clone()).await;
            let proof = get_proof(self.cluster.clone(), self.keypair.pubkey()).await;
            let (next_hash, nonce) =
                self.find_next_hash_par(proof.hash.into(), treasury.difficulty.into());
            println!(
                "Found a valid hash {:?} nonce: {:?}",
                next_hash.clone(),
                nonce
            );

            // TODO Retry if bus has insufficient funds.
            // Submit mine tx.
            let client =
                RpcClient::new_with_commitment(self.cluster.clone(), CommitmentConfig::processed());
            let mut bus_id = 0;
            let mut invalid_busses: Vec<u8> = vec![];
            let recent_blockhash = client.get_latest_blockhash().await.unwrap();
            loop {
                // Find a valid bus.
                if invalid_busses.len().eq(&(BUS_COUNT as usize)) {
                    // All busses are drained. Wait until next epoch.
                    std::thread::sleep(std::time::Duration::from_millis(1000));
                }
                if invalid_busses.contains(&bus_id) {
                    bus_id += 1;
                }

                // Check if epoch needs to be reset
                let treasury = get_treasury(self.cluster.clone()).await;
                let clock = get_clock_account(self.cluster.clone()).await;
                let epoch_end_at = treasury.epoch_start_at.saturating_add(EPOCH_DURATION);
                if clock.unix_timestamp.ge(&epoch_end_at) {
                    // Submit restart epoch tx.
                    // TODO
                    let reset_ix = ore::instruction::reset(self.keypair.pubkey());
                    let tx = Transaction::new_signed_with_payer(
                        &[reset_ix],
                        Some(&self.keypair.pubkey()),
                        &[self.keypair],
                        recent_blockhash,
                    );
                    let sig = client.send_and_confirm_transaction(&tx).await.ok();
                    println!("Reset: {:?}", sig);
                }

                // Submit request.
                const COMPUTE_BUDGET: u32 = 3000; // Determined from on local testing
                let ix_cu_budget = ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_BUDGET);
                let ix_mine = ore::instruction::mine(
                    self.keypair.pubkey(),
                    BUS_ADDRESSES[bus_id as usize],
                    next_hash.into(),
                    nonce,
                );
                let tx = Transaction::new_signed_with_payer(
                    &[ix_cu_budget, ix_mine],
                    Some(&self.keypair.pubkey()),
                    &[self.keypair],
                    recent_blockhash,
                );
                let result = client.send_and_confirm_transaction(&tx).await;
                match result {
                    Ok(sig) => {
                        println!("Sig: {}", sig);
                        break;
                    }
                    Err(err) => {
                        match err.kind {
                            ClientErrorKind::RpcError(err) => {
                                // TODO Why is BusInsufficientFunds an RpcError but EpochNeedsReset is a TransactionError ?
                                //      Unhandled error Error { request: None, kind: TransactionError(InstructionError(0, Custom(6003))) }
                                //      thread 'main' panicked at 'Failed to submit transaction: SolanaClientError(Error { request: None, kind: TransactionError(InstructionError(0, Custom(6000))) })', src/main.rs:193:26
                                if err.to_string().contains("Transaction simulation failed: Error processing Instruction 0: custom program error: 0x1775") {
                                    // Bus has no remaining funds. Use a different one.
                                    println!("Bus {} is drained. Finding another one.", bus_id);
                                    invalid_busses.push(bus_id);
                                } else {
                                    log::error!("{:?}", err.to_string());
                                }
                            }
                            _ => {
                                println!("Unhandled error {:?}", err);
                            }
                        }
                    }
                }
            }
        }
    }

    async fn claim(&self, cluster: String, beneficiary: Pubkey, amount: u64) {
        let pubkey = self.keypair.pubkey();
        let client = RpcClient::new_with_commitment(cluster, CommitmentConfig::processed());
        let ix = ore::instruction::claim(pubkey, beneficiary, amount);
        let recent_blockhash = client.get_latest_blockhash().await.unwrap();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&pubkey),
            &[self.keypair],
            recent_blockhash,
        );
        let sig = client.send_and_confirm_transaction(&tx).await.ok();
        println!("Sig: {:?}", sig);
    }

    fn _find_next_hash(&self, hash: KeccakHash, difficulty: KeccakHash) -> (KeccakHash, u64) {
        let mut next_hash: KeccakHash;
        let mut nonce = 0u64;
        loop {
            next_hash = hashv(&[
                hash.to_bytes().as_slice(),
                self.keypair.pubkey().to_bytes().as_slice(),
                nonce.to_be_bytes().as_slice(),
            ]);
            if next_hash.le(&difficulty) {
                break;
            } else {
                println!("Invalid hash: {} Nonce: {:?}", next_hash.to_string(), nonce);
            }
            nonce += 1;
        }
        (next_hash, nonce)
    }

    fn find_next_hash_par(&self, hash: KeccakHash, difficulty: KeccakHash) -> (KeccakHash, u64) {
        let found_solution = Arc::new(AtomicBool::new(false));
        let solution = Arc::new(Mutex::<(KeccakHash, u64)>::new((
            KeccakHash::new_from_array([0; 32]),
            0,
        )));

        println!("Searching for a valid hash...");
        let pubkey = self.keypair.pubkey();
        let thread_handles: Vec<_> = (0..NUM_THREADS)
            .map(|i| {
                std::thread::spawn({
                    let found_solution = found_solution.clone();
                    let solution = solution.clone();
                    move || {
                        let n = u64::MAX.saturating_div(NUM_THREADS).saturating_mul(i);
                        let mut next_hash: KeccakHash;
                        let mut nonce: u64 = n;
                        loop {
                            if nonce % 10_000 == 0 {
                                if found_solution.load(std::sync::atomic::Ordering::Relaxed) {
                                    return;
                                }
                            }
                            next_hash = hashv(&[
                                hash.to_bytes().as_slice(),
                                pubkey.to_bytes().as_slice(),
                                nonce.to_be_bytes().as_slice(),
                            ]);
                            if next_hash.le(&difficulty) {
                                found_solution.store(true, std::sync::atomic::Ordering::Relaxed);
                                let mut w_solution = solution.lock().expect("failed to lock mutex");
                                *w_solution = (next_hash, nonce);
                                return;
                            }
                            nonce += 1;
                        }
                    }
                })
            })
            .collect();

        for thread_handle in thread_handles {
            thread_handle.join().unwrap();
        }

        let r_solution = solution.lock().expect("Failed to get lock");
        *r_solution
    }
}

pub async fn get_treasury(cluster: String) -> Treasury {
    let client = RpcClient::new_with_commitment(cluster, CommitmentConfig::processed());
    let data = client
        .get_account_data(&TREASURY_ADDRESS)
        .await
        .expect("Failed to get treasury account");
    *Treasury::try_from_bytes(&data).expect("Failed to parse treasury account")
}

pub async fn get_proof(cluster: String, authority: Pubkey) -> Proof {
    let client = RpcClient::new_with_commitment(cluster, CommitmentConfig::processed());
    let proof_address = proof_pubkey(authority);
    let data = client
        .get_account_data(&proof_address)
        .await
        .expect("Failed to get miner account");
    *Proof::try_from_bytes(&data).expect("Failed to parse miner account")
}

pub async fn get_clock_account(cluster: String) -> Clock {
    let client = RpcClient::new_with_commitment(cluster, CommitmentConfig::processed());
    let data = client
        .get_account_data(&sysvar::clock::ID)
        .await
        .expect("Failed to get miner account");
    bincode::deserialize::<Clock>(&data).expect("Failed to deserialize clock")
}

async fn initialize_program(cluster: String, keypair_filepath: String) {
    // Return early if program is initialized
    let client = RpcClient::new_with_commitment(cluster, CommitmentConfig::processed());
    if client.get_account(&TREASURY_ADDRESS).await.is_ok() {
        return;
    }

    // Sign and send transaction.
    let signer = read_keypair_file(keypair_filepath).unwrap();
    let ix = ore::instruction::initialize(signer.pubkey());
    let mut transaction = Transaction::new_with_payer(&[ix], Some(&signer.pubkey()));
    let recent_blockhash = client.get_latest_blockhash().await.unwrap();
    transaction.sign(&[&signer], recent_blockhash);
    let result = client.send_and_confirm_transaction(&transaction).await;
    match result {
        Ok(signature) => println!("Transaction successful with signature: {:?}", signature),
        Err(e) => println!("Transaction failed: {:?}", e),
    }
}

async fn initialize_proof_account(cluster: String, keypair_filepath: String) {
    // Return early if program is initialized
    let signer = read_keypair_file(keypair_filepath).unwrap();
    let proof_address = proof_pubkey(signer.pubkey());
    let client = RpcClient::new_with_commitment(cluster, CommitmentConfig::processed());
    if client.get_account(&proof_address).await.is_ok() {
        return;
    }

    // Sign and send transaction.
    let ix = ore::instruction::register(signer.pubkey());
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
    cluster: String,
    keypair_filepath: String,
    authority: Pubkey,
) -> Pubkey {
    // Initialize client.
    let signer = read_keypair_file(keypair_filepath).unwrap();
    let client = RpcClient::new_with_commitment(cluster, CommitmentConfig::processed());

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
        &ore::MINT_ADDRESS,
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
fn proof_pubkey(authority: Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[PROOF, authority.as_ref()], &ore::ID).0
}

#[cached]
fn treasury_tokens_pubkey() -> Pubkey {
    get_associated_token_address(&TREASURY_ADDRESS, &MINT_ADDRESS)
}
