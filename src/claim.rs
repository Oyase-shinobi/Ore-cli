use std::str::FromStr;

use colored::*;
use ore::{self, ONE_DAY};
use solana_program::pubkey::Pubkey;
use solana_sdk::signature::Signer;
use spl_token::amount_to_ui_amount;

use crate::{
    args::ClaimArgs,
    cu_limits::CU_LIMIT_CLAIM,
    send_and_confirm::ComputeBudget,
    utils::{amount_f64_to_u64, ask_confirm, get_clock, get_proof},
    Miner,
};

impl Miner {
    pub async fn claim(&self, args: ClaimArgs) {
        let signer = self.signer();
        let pubkey = signer.pubkey();
        let proof = get_proof(&self.rpc_client, pubkey).await;
        let beneficiary = match args.beneficiary {
            Some(beneficiary) => {
                Pubkey::from_str(&beneficiary).expect("Failed to parse beneficiary address")
            }
            None => self.initialize_ata().await,
        };
        let amount = if let Some(amount) = args.amount {
            amount_f64_to_u64(amount)
        } else {
            proof.balance
        };

        // Burn warning
        let clock = get_clock(&self.rpc_client).await;
        let t = proof.last_claim_at.saturating_add(ONE_DAY);
        if clock.unix_timestamp.lt(&t) {
            let burn_amount = amount
                .saturating_mul(t.saturating_sub(clock.unix_timestamp) as u64)
                .saturating_div(ONE_DAY as u64);
            let hours_ago =
                (clock.unix_timestamp.saturating_sub(proof.last_claim_at) as f64) / 60f64 / 64f64;
            let mins_to_go = t.saturating_sub(clock.unix_timestamp).saturating_div(60);
            if !ask_confirm(
                format!("\n{} You are about to burn {}!\nClaiming more frequently than once per day is subject to a burn penalty.\nYour last claim was {:.2} hours ago. You must wait {} minutes to avoid this penalty.\n\nAre you sure you want to continue? [Y/n]", 
                    "WARNING".bold().yellow(),
                    format!("{} ORE", amount_to_ui_amount(burn_amount, ore::TOKEN_DECIMALS)).bold(),
                    hours_ago,
                    mins_to_go
                ).as_str()
            ) {
                return;
            }
        }

        let ix = ore::instruction::claim(pubkey, beneficiary, amount);
        self.send_and_confirm(&[ix], ComputeBudget::Fixed(CU_LIMIT_CLAIM), false)
            .await
            .ok();
    }

    async fn initialize_ata(&self) -> Pubkey {
        // Initialize client.
        let signer = self.signer();
        let client = self.rpc_client.clone();

        // Build instructions.
        let token_account_pubkey = spl_associated_token_account::get_associated_token_address(
            &signer.pubkey(),
            &ore::MINT_ADDRESS,
        );

        // Check if ata already exists
        if let Ok(Some(_ata)) = client.get_token_account(&token_account_pubkey).await {
            return token_account_pubkey;
        }
        // Sign and send transaction.
        let ix = spl_associated_token_account::instruction::create_associated_token_account(
            &signer.pubkey(),
            &signer.pubkey(),
            &ore::MINT_ADDRESS,
            &spl_token::id(),
        );
        self.send_and_confirm(&[ix], ComputeBudget::Dynamic, false)
            .await
            .ok();

        // Return token account address
        token_account_pubkey
    }
}
