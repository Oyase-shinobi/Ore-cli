use std::str::FromStr;

use ore::{self, state::Proof, utils::AccountDeserialize};
use solana_program::pubkey::Pubkey;
use solana_sdk::signature::Signer;

use crate::{
    args::ClaimArgs,
    cu_limits::CU_LIMIT_CLAIM,
    send_and_confirm::ComputeBudget,
    utils::{amount_f64_to_u64, proof_pubkey},
    Miner,
};

// TODO Burn warning

impl Miner {
    pub async fn claim(&self, args: ClaimArgs) {
        let signer = self.signer();
        let pubkey = signer.pubkey();
        let client = self.rpc_client.clone();
        let beneficiary = match args.beneficiary {
            Some(beneficiary) => {
                Pubkey::from_str(&beneficiary).expect("Failed to parse beneficiary address")
            }
            None => self.initialize_ata().await,
        };
        let amount = if let Some(amount) = args.amount {
            amount_f64_to_u64(amount)
        } else {
            match client.get_account(&proof_pubkey(pubkey)).await {
                Ok(proof_account) => {
                    let proof = Proof::try_from_bytes(&proof_account.data).unwrap();
                    proof.balance
                }
                Err(err) => {
                    println!("Error looking up claimable rewards: {:?}", err);
                    return;
                }
            }
        };
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
