use colored::*;
use solana_sdk::{pubkey::Pubkey, signer::Signer};
use spl_token::amount_to_ui_amount;

use crate::{
    send_and_confirm::ComputeBudget,
    utils::{amount_f64_to_u64, ask_confirm},
    Miner, UpgradeArgs,
};

impl Miner {
    pub async fn upgrade(&self, args: UpgradeArgs) {
        let signer = &self.signer();
        let beneficiary = self.get_or_initialize_ata().await;
        let (sender, sender_balance) = self.get_ata_v1().await;

        let amount_f64 = match args.amount {
            Some(f64) => f64,
            None => {
                println!("Defaulting to max amount: {}", sender_balance);
                sender_balance
            }
        };
        let amount = amount_f64_to_u64(amount_f64);
        println!("amount: {}", amount);
        println!("beneficiary: {}", beneficiary);
        println!("sender: {}", sender);

        if !ask_confirm(
            format!(
                "\n You are about to upgrade {}. \n\nAre you sure you want to continue? [Y/n]",
                format!("{} ORE", amount_to_ui_amount(amount, ore::TOKEN_DECIMALS)).bold(),
            )
            .as_str(),
        ) {
            return;
        }

        // TODO: fixed compute budget
        let ix = ore::instruction::upgrade(signer.pubkey(), beneficiary, sender, amount);
        match self
            .send_and_confirm(&[ix], ComputeBudget::Dynamic, false)
            .await
        {
            Ok(tx) => {
                println!("tx: {}", tx);
            }
            Err(err) => {
                println!("error: {}", err);
            }
        }
    }

    // asserts that token account exists and gets balance
    async fn get_ata_v1(&self) -> (Pubkey, f64) {
        // Initialize client.
        let signer = self.signer();
        let client = self.rpc_client.clone();

        // Derive assoicated token address (for v1 account)
        let token_account_pubkey_v1 = spl_associated_token_account::get_associated_token_address(
            &signer.pubkey(),
            &ore::MINT_V1_ADDRESS,
        );

        // Get token account balance
        let balance = match client.get_token_account(&token_account_pubkey_v1).await {
            Ok(None) => {
                println!("v1 token account doesn't exist");
                panic!()
            }
            Ok(Some(token_account)) => match token_account.token_amount.ui_amount {
                Some(ui_amount) => ui_amount,
                None => {
                    println!(
                        "Error parsing token account UI amount: {}",
                        token_account.token_amount.amount
                    );
                    panic!()
                }
            },
            Err(err) => {
                println!("Error fetching token account: {}", err);
                panic!()
            }
        };

        // Return v1 token account address
        (token_account_pubkey_v1, balance)
    }

    async fn get_or_initialize_ata(&self) -> Pubkey {
        // Initialize client.
        let signer = self.signer();
        let client = self.rpc_client.clone();

        // Derive assoicated token address (ata)
        let token_account_pubkey = spl_associated_token_account::get_associated_token_address(
            &signer.pubkey(),
            &ore::MINT_ADDRESS,
        );

        // Check if ata already exists or init
        if let Err(_err) = client.get_token_account(&token_account_pubkey).await {
            println!("Initializing v2 token account...");
            let ix = spl_associated_token_account::instruction::create_associated_token_account(
                &signer.pubkey(),
                &signer.pubkey(),
                &ore::MINT_ADDRESS,
                &spl_token::id(),
            );
            self.send_and_confirm(&[ix], ComputeBudget::Dynamic, false)
                .await
                .ok();
        }

        // Return token account address
        token_account_pubkey
    }
}
