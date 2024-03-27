mod balance;
mod claim;
#[cfg(feature = "admin")]
mod initialize;
mod mine;
mod register;
mod rewards;
mod treasury;
#[cfg(feature = "admin")]
mod update_admin;
#[cfg(feature = "admin")]
mod update_difficulty;
mod utils;

use std::sync::Arc;

use clap::{command, Parser, Subcommand};
use solana_sdk::signature::{read_keypair_file, Keypair};

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
    Balance(BalanceArgs),

    #[command(about = "Mine Ore using local compute")]
    Mine(MineArgs),

    #[command(about = "Claim available mining rewards")]
    Claim(ClaimArgs),

    #[command(about = "Fetch your balance of unclaimed mining rewards")]
    Rewards(RewardsArgs),

    #[command(about = "Fetch the treasury account and balance")]
    Treasury(TreasuryArgs),

    #[cfg(feature = "admin")]
    #[command(about = "Initialize the program")]
    Initialize(InitializeArgs),

    #[cfg(feature = "admin")]
    #[command(about = "Update the program admin authority")]
    UpdateAdmin(UpdateAdminArgs),

    #[cfg(feature = "admin")]
    #[command(about = "Update the mining difficulty")]
    UpdateDifficulty(UpdateDifficultyArgs),
}

#[derive(Parser, Debug)]
struct BalanceArgs {
    #[arg(
        // long,
        value_name = "ADDRESS",
        help = "The address of the account to fetch the balance of"
    )]
    pub address: Option<String>,
}

#[derive(Parser, Debug)]
struct RewardsArgs {
    #[arg(
        // long,
        value_name = "ADDRESS",
        help = "The address of the account to fetch the rewards balance of"
    )]
    pub address: Option<String>,
}

#[derive(Parser, Debug)]
struct MineArgs {
    // TODO Thread count
}

#[derive(Parser, Debug)]
struct TreasuryArgs {}

#[derive(Parser, Debug)]
struct ClaimArgs {
    #[arg(
        // long,
        value_name = "AMOUNT",
        help = "The amount of rewards to claim. Defaults to max."
    )]
    amount: Option<f64>,

    #[arg(
        // long,
        value_name = "TOKEN_ACCOUNT_ADDRESS",
        help = "Token account to receive mining rewards."
    )]
    beneficiary: Option<String>,
}

#[cfg(feature = "admin")]
#[derive(Parser, Debug)]
struct InitializeArgs {}

#[cfg(feature = "admin")]
#[derive(Parser, Debug)]
struct UpdateAdminArgs {
    new_admin: String,
}

#[cfg(feature = "admin")]
#[derive(Parser, Debug)]
struct UpdateDifficultyArgs {}

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
        Commands::Balance(args) => {
            miner.balance(args.address).await;
        }
        Commands::Rewards(args) => {
            miner.rewards(args.address).await;
        }
        Commands::Treasury(_) => {
            miner.treasury().await;
        }
        Commands::Mine(_args) => {
            miner.mine().await;
        }
        Commands::Claim(args) => {
            miner.claim(cluster, args.beneficiary, args.amount).await;
        }
        #[cfg(feature = "admin")]
        Commands::Initialize(_) => {
            miner.initialize().await;
        }
        #[cfg(feature = "admin")]
        Commands::UpdateAdmin(args) => {
            miner.update_admin(args.new_admin).await;
        }
        #[cfg(feature = "admin")]
        Commands::UpdateDifficulty(_) => {
            miner.update_difficulty().await;
        }
    }
}

impl<'a> Miner<'a> {
    pub fn new(cluster: String, signer: &'a Keypair) -> Self {
        Self { signer, cluster }
    }
}
