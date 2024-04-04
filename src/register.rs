use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig, compute_budget::ComputeBudgetInstruction,
    signature::Signer,
};

use crate::{cu_limits::CU_LIMIT_REGISTER, utils::proof_pubkey, Miner};

impl Miner {
    pub async fn register(&self) {
        // Return early if miner is already registered
        let signer = self.signer();
        let proof_address = proof_pubkey(signer.pubkey());
        let client =
            RpcClient::new_with_commitment(self.cluster.clone(), CommitmentConfig::confirmed());
        if client.get_account(&proof_address).await.is_ok() {
            return;
        }

        // Sign and send transaction.
        println!("Generating challenge...");
        let cu_limit_ix = ComputeBudgetInstruction::set_compute_unit_limit(CU_LIMIT_REGISTER);
        let cu_price_ix = ComputeBudgetInstruction::set_compute_unit_price(self.priority_fee);
        let ix = ore::instruction::register(signer.pubkey());
        self.send_and_confirm(&[cu_limit_ix, cu_price_ix, ix], false)
            .await
            .expect("Transaction failed");
    }
}
