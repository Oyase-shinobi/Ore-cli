use std::str::FromStr;

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_program::pubkey::Pubkey;
use solana_sdk::{
    commitment_config::CommitmentConfig, signature::Signer, transaction::Transaction,
};

use crate::Miner;

impl<'a> Miner<'a> {
    pub async fn update_admin(&self, new_admin: String) {
        let new_admin = Pubkey::from_str(new_admin.as_str()).unwrap();
        let client =
            RpcClient::new_with_commitment(self.cluster.clone(), CommitmentConfig::processed());

        // Sign and send transaction.
        let ix = ore::instruction::update_admin(self.signer.pubkey(), new_admin);
        let mut tx = Transaction::new_with_payer(&[ix], Some(&self.signer.pubkey()));
        let recent_blockhash = client.get_latest_blockhash().await.unwrap();
        tx.sign(&[&self.signer], recent_blockhash);
        match client.send_and_confirm_transaction(&tx).await {
            Ok(sig) => println!("{:?}", sig),
            Err(e) => println!("Transaction failed: {:?}", e),
        }
    }
}
