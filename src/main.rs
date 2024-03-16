use std::{
    borrow::BorrowMut,
    io::{stdout, Stdout, Write},
    str::FromStr,
    sync::{atomic::AtomicBool, Arc, Mutex},
};

use cached::proc_macro::cached;
use clap::{command, Parser, Subcommand};
use crossterm::{
    cursor, execute,
    terminal::{self, ClearType},
    QueueableCommand,
};
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

// TODO Fetch hardware concurrency dynamically
const NUM_THREADS: u64 = 6;

struct Miner<'a> {
    pub signer: &'a Keypair,
    pub cluster: String,
}

#[derive(Parser, Debug)]
#[command(about, version)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    #[command(about = "Fetch the Ore balance of an account")]
    Balance(Balance),

    #[command(about = "Mine Ore using local compute")]
    Mine(Mine),

    #[command(about = "Claim available mining rewards")]
    Claim(Claim),

    #[cfg(feature = "admin")]
    #[command(about = "Initialize the program")]
    Initialize(Initialize),

    #[cfg(feature = "admin")]
    #[command(about = "Update the program admin authority")]
    UpdateAdmin(UpdateAdmin),

    #[cfg(feature = "admin")]
    #[command(about = "Update the mining difficulty")]
    UpdateDifficulty(UpdateDifficulty),
}

#[derive(Parser, Debug)]
struct Balance {
    #[arg(
        long,
        value_name = "ADDRESS",
        help = "The address of the account to fetch the balance of"
    )]
    pub address: Option<String>,
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

// TODO Address
// TODO Busses
// TODO Epoch
// TODO Treasury

#[cfg(feature = "admin")]
#[derive(Parser, Debug)]
struct Initialize {}

#[cfg(feature = "admin")]
#[derive(Parser, Debug)]
struct UpdateAdmin {
    new_admin: String,
}

#[cfg(feature = "admin")]
#[derive(Parser, Debug)]
struct UpdateDifficulty {
    new_difficulty: String,
}

#[tokio::main]
async fn main() {
    // Initialize miner.
    let args = Args::parse();
    let solana_config_file = solana_cli_config::CONFIG_FILE.as_ref().unwrap().as_str();
    let solana_config = match solana_cli_config::Config::load(solana_config_file) {
        Ok(cfg) => cfg,
        Err(_err) => {
            println!("Failed fetching solana keypair. Please install the Solana CLI: https://docs.solanalabs.com/cli/install");
            return;
        }
    };
    let cluster = solana_config.json_rpc_url;
    let signer = read_keypair_file(solana_config.keypair_path).unwrap();
    let miner = Arc::new(Miner::new(cluster.clone(), &signer));

    // Execute user command.
    match args.command {
        Commands::Balance(cmd) => {
            miner.balance(cmd.address).await;
        }
        Commands::Mine(_cmd) => {
            miner.mine().await;
        }
        Commands::Claim(cmd) => {
            let beneficiary = match cmd.beneficiary {
                Some(beneficiary) => {
                    Pubkey::from_str(&beneficiary).expect("Failed to parse beneficiary address")
                }
                None => miner.initialize_token_account().await,
            };
            miner.claim(cluster, beneficiary, cmd.amount).await;
        }
        #[cfg(feature = "admin")]
        Commands::Initialize(_) => {
            miner.initialize().await;
        }
        #[cfg(feature = "admin")]
        Commands::UpdateAdmin(cmd) => {
            // TODO
        }
        #[cfg(feature = "admin")]
        Commands::UpdateDifficulty(cmd) => {
            // TODO
        }
    }
}

impl<'a> Miner<'a> {
    pub fn new(cluster: String, signer: &'a Keypair) -> Self {
        Self { signer, cluster }
    }

    pub async fn balance(&self, address: Option<String>) {
        let address = if let Some(address) = address {
            if let Ok(address) = Pubkey::from_str(&address) {
                address
            } else {
                println!("Invalid address: {:?}", address);
                return;
            }
        } else {
            self.signer.pubkey()
        };
        let client =
            RpcClient::new_with_commitment(self.cluster.clone(), CommitmentConfig::processed());
        let token_account_address = spl_associated_token_account::get_associated_token_address(
            &address,
            &ore::MINT_ADDRESS,
        );
        match client.get_token_account(&token_account_address).await {
            Ok(token_account) => {
                if let Some(token_account) = token_account {
                    println!("{:} ORE", token_account.token_amount.ui_amount_string);
                } else {
                    println!("Account not found");
                }
            }
            Err(err) => {
                println!("{:?}", err);
            }
        }
    }

    async fn register(&self) {
        // Return early if miner is already registered
        let proof_address = proof_pubkey(self.signer.pubkey());
        let client =
            RpcClient::new_with_commitment(self.cluster.clone(), CommitmentConfig::processed());
        if client.get_account(&proof_address).await.is_ok() {
            return;
        }

        // Sign and send transaction.
        let ix = ore::instruction::register(self.signer.pubkey());
        let mut tx = Transaction::new_with_payer(&[ix], Some(&self.signer.pubkey()));
        let recent_blockhash = client.get_latest_blockhash().await.unwrap();
        tx.sign(&[&self.signer], recent_blockhash);
        match client.send_and_confirm_transaction(&tx).await {
            Ok(sig) => println!("Transaction successful with signature: {:?}", sig),
            Err(e) => println!("Transaction failed: {:?}", e),
        }
    }

    pub async fn mine(&self) {
        // Register, if needed.
        self.register().await;

        let mut stdout = stdout();
        stdout.queue(cursor::SavePosition).unwrap();

        // Start mining loop
        loop {
            // Find a valid hash.
            let treasury = get_treasury(self.cluster.clone()).await;
            let proof = get_proof(self.cluster.clone(), self.signer.pubkey()).await;
            execute!(
                stdout,
                cursor::MoveTo(0, 0),
                terminal::Clear(ClearType::All)
            )
            .ok();
            stdout
                .write_all(format!("Searching for valid hash...\n").as_bytes())
                .ok();
            let (next_hash, nonce) =
                self.find_next_hash_par(proof.hash.into(), treasury.difficulty.into());
            stdout
                .write_all(format!("\nSubmitting hash for validation... \n").as_bytes())
                .ok();
            stdout.flush().ok();

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

                // Reset if epoch has ended
                let treasury = get_treasury(self.cluster.clone()).await;
                let clock = get_clock_account(self.cluster.clone()).await;
                let threshold = treasury.last_reset_at.saturating_add(EPOCH_DURATION);
                if clock.unix_timestamp.ge(&threshold) {
                    let reset_ix = ore::instruction::reset(self.signer.pubkey());
                    let tx = Transaction::new_signed_with_payer(
                        &[reset_ix],
                        Some(&self.signer.pubkey()),
                        &[self.signer],
                        recent_blockhash,
                    );
                    client.send_and_confirm_transaction(&tx).await.ok();
                }

                // Submit request.
                const COMPUTE_BUDGET: u32 = 3000; // Determined from on local testing
                let ix_cu_budget = ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_BUDGET);
                let ix_mine = ore::instruction::mine(
                    self.signer.pubkey(),
                    BUS_ADDRESSES[bus_id as usize],
                    next_hash.into(),
                    nonce,
                );
                let tx = Transaction::new_signed_with_payer(
                    &[ix_cu_budget, ix_mine],
                    Some(&self.signer.pubkey()),
                    &[self.signer],
                    recent_blockhash,
                );
                let result = client.send_and_confirm_transaction(&tx).await;
                match result {
                    Ok(sig) => {
                        stdout.write(format!("Success: {}", sig).as_bytes()).ok();
                        // println!("Sig: {}", sig);
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
                                    // println!("Bus {} is drained. Finding another one.", bus_id);
                                    invalid_busses.push(bus_id);
                                } else {
                                    // log::error!("{:?}", err.to_string());
                                }
                            }
                            _ => {
                                // println!("Unhandled error {:?}", err);
                            }
                        }
                    }
                }
            }
        }
    }

    async fn claim(&self, cluster: String, beneficiary: Pubkey, amount: Option<u64>) {
        let pubkey = self.signer.pubkey();
        let client = RpcClient::new_with_commitment(cluster, CommitmentConfig::processed());
        let amount = if let Some(amount) = amount {
            amount
        } else {
            match client.get_account(&proof_pubkey(pubkey)).await {
                Ok(proof_account) => {
                    let proof = Proof::try_from_bytes(&proof_account.data).unwrap();
                    proof.claimable_rewards
                }
                Err(err) => {
                    println!("Error looking up claimable rewards: {:?}", err);
                    return;
                }
            }
        };
        let ix = ore::instruction::claim(pubkey, beneficiary, amount);
        let recent_blockhash = client.get_latest_blockhash().await.unwrap();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&pubkey),
            &[self.signer],
            recent_blockhash,
        );
        let sig = client.send_and_confirm_transaction(&tx).await.ok();
        let amountf = (amount as f64) / (10f64.powf(ore::TOKEN_DECIMALS as f64));
        println!(
            "Successfully claimed {:} ORE to account {:}",
            amountf, beneficiary
        );
        println!("Transaction: {:?}", sig);
    }

    fn _find_next_hash(&self, hash: KeccakHash, difficulty: KeccakHash) -> (KeccakHash, u64) {
        let mut next_hash: KeccakHash;
        let mut nonce = 0u64;
        loop {
            next_hash = hashv(&[
                hash.to_bytes().as_slice(),
                self.signer.pubkey().to_bytes().as_slice(),
                nonce.to_le_bytes().as_slice(),
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
        let pubkey = self.signer.pubkey();
        let thread_handles: Vec<_> = (0..NUM_THREADS)
            .map(|i| {
                std::thread::spawn({
                    let found_solution = found_solution.clone();
                    let solution = solution.clone();
                    let mut stdout = stdout();
                    move || {
                        let n = u64::MAX.saturating_div(NUM_THREADS).saturating_mul(i);
                        let mut next_hash: KeccakHash;
                        let mut nonce: u64 = n;
                        loop {
                            next_hash = hashv(&[
                                hash.to_bytes().as_slice(),
                                pubkey.to_bytes().as_slice(),
                                nonce.to_le_bytes().as_slice(),
                            ]);
                            if nonce % 10_000 == 0 {
                                if found_solution.load(std::sync::atomic::Ordering::Relaxed) {
                                    return;
                                }
                                if n == 0 {
                                    stdout
                                        .write_all(
                                            format!("\r{}", next_hash.to_string()).as_bytes(),
                                        )
                                        .ok();
                                }
                            }
                            if next_hash.le(&difficulty) {
                                stdout
                                    .write_all(format!("\r{}", next_hash.to_string()).as_bytes())
                                    .ok();
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

    #[cfg(feature = "admin")]
    async fn initialize(&self) {
        // Return early if program is initialized
        let client =
            RpcClient::new_with_commitment(self.cluster.clone(), CommitmentConfig::processed());
        if client.get_account(&TREASURY_ADDRESS).await.is_ok() {
            return;
        }

        // Sign and send transaction.
        let ix = ore::instruction::initialize(self.signer.pubkey());
        let mut tx = Transaction::new_with_payer(&[ix], Some(&self.signer.pubkey()));
        let recent_blockhash = client.get_latest_blockhash().await.unwrap();
        tx.sign(&[&self.signer], recent_blockhash);
        match client.send_and_confirm_transaction(&tx).await {
            Ok(sig) => println!("Transaction successful with signature: {:?}", sig),
            Err(e) => println!("Transaction failed: {:?}", e),
        }
    }

    async fn initialize_token_account(&self) -> Pubkey {
        // Initialize client.
        let authority = self.signer.pubkey();
        let client =
            RpcClient::new_with_commitment(self.cluster.clone(), CommitmentConfig::processed());

        // Build instructions.
        let token_account_keypair = Keypair::new();
        let token_account_pubkey = token_account_keypair.pubkey();
        let rent = client
            .get_minimum_balance_for_rent_exemption(TokenAccount::LEN)
            .await
            .unwrap();
        let create_account_instruction = system_instruction::create_account(
            &self.signer.pubkey(),
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
        let mut tx = Transaction::new_with_payer(
            &[create_account_instruction, initialize_account_instruction],
            Some(&self.signer.pubkey()),
        );
        let recent_blockhash = client.get_latest_blockhash().await.unwrap();
        tx.sign(&[&self.signer, &token_account_keypair], recent_blockhash);
        let result = client.send_and_confirm_transaction(&tx).await;
        match result {
            Ok(sig) => println!("Transaction successful with signature: {:?}", sig),
            Err(e) => println!("Transaction failed: {:?}", e),
        }

        // Return token account address
        token_account_pubkey
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

#[cached]
fn proof_pubkey(authority: Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[PROOF, authority.as_ref()], &ore::ID).0
}

#[cached]
fn treasury_tokens_pubkey() -> Pubkey {
    get_associated_token_address(&TREASURY_ADDRESS, &MINT_ADDRESS)
}
