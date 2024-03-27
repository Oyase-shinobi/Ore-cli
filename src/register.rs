use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig, signature::Signer, transaction::Transaction,
};

use crate::{utils::proof_pubkey, Miner};

impl<'a> Miner<'a> {
    pub async fn register(&self) {
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
}
