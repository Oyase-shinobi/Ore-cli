use std::str::FromStr;

use ore::{self, state::Proof, utils::AccountDeserialize};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_program::pubkey::Pubkey;
use solana_sdk::{
    commitment_config::CommitmentConfig, signature::Signer, transaction::Transaction,
};

use crate::{utils::proof_pubkey, Miner};

impl<'a> Miner<'a> {
    pub async fn claim(&self, cluster: String, beneficiary: Option<String>, amount: Option<f64>) {
        let pubkey = self.signer.pubkey();
        let client = RpcClient::new_with_commitment(cluster, CommitmentConfig::processed());
        let beneficiary = match beneficiary {
            Some(beneficiary) => {
                Pubkey::from_str(&beneficiary).expect("Failed to parse beneficiary address")
            }
            None => self.initialize_ata().await,
        };
        let amount = if let Some(amount) = amount {
            (amount * 10f64.powf(ore::TOKEN_DECIMALS as f64)) as u64
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
        let amountf = (amount as f64) / (10f64.powf(ore::TOKEN_DECIMALS as f64));
        let ix = ore::instruction::claim(pubkey, beneficiary, amount);
        let recent_blockhash = client.get_latest_blockhash().await.unwrap();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&pubkey),
            &[self.signer],
            recent_blockhash,
        );
        match client.send_and_confirm_transaction(&tx).await {
            Ok(sig) => {
                println!("Claimed {:} ORE to account {:}", amountf, beneficiary);
                println!("{:?}", sig);
            }
            Err(err) => {
                println!("Error: {:?}", err);
            }
        }
    }

    async fn initialize_ata(&self) -> Pubkey {
        // Initialize client.
        let client =
            RpcClient::new_with_commitment(self.cluster.clone(), CommitmentConfig::processed());

        // Build instructions.
        let token_account_pubkey = spl_associated_token_account::get_associated_token_address(
            &self.signer.pubkey(),
            &ore::MINT_ADDRESS,
        );

        // Check if ata already exists
        if let Ok(Some(_ata)) = client.get_token_account(&token_account_pubkey).await {
            return token_account_pubkey;
        }

        // Sign and send transaction.
        let ix = spl_associated_token_account::instruction::create_associated_token_account(
            &self.signer.pubkey(),
            &self.signer.pubkey(),
            &ore::MINT_ADDRESS,
            &spl_token::id(),
        );
        let mut tx = Transaction::new_with_payer(&[ix], Some(&self.signer.pubkey()));
        let recent_blockhash = client.get_latest_blockhash().await.unwrap();
        tx.sign(&[&self.signer], recent_blockhash);
        let result = client.send_and_confirm_transaction(&tx).await;
        match result {
            Ok(_sig) => println!("Created token account {:?}", token_account_pubkey),
            Err(e) => println!("Transaction failed: {:?}", e),
        }

        // Return token account address
        token_account_pubkey
    }
}
