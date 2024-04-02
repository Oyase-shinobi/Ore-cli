use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig, compute_budget::ComputeBudgetInstruction,
    signature::Signer,
};

use crate::{utils::proof_pubkey, Miner};

const CU_BUDGET: u32 = 20_000;

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
        let cu_budget_ix = ComputeBudgetInstruction::set_compute_unit_limit(CU_BUDGET);
        let cu_price_ix = ComputeBudgetInstruction::set_compute_unit_price(self.priority_fee);
        let ix = ore::instruction::register(signer.pubkey());
        self.send_and_confirm(&[cu_budget_ix, cu_price_ix, ix])
            .await
            .expect("Transaction failed");
    }
}
