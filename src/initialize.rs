use ore::TREASURY_ADDRESS;

use solana_sdk::{signature::Signer, transaction::Transaction};

use crate::Miner;

impl Miner {
    pub async fn initialize(&self) {
        // Return early if program is already initialized
        if self.rpc_client.get_account(&TREASURY_ADDRESS).await.is_ok() {
            return;
        }

        // Submit initialize tx
        let blockhash = self.rpc_client.get_latest_blockhash().await.unwrap();
        let ix = ore::instruction::initialize(self.signer().pubkey());
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&self.signer().pubkey()),
            &[&self.signer()],
            blockhash,
        );
        // let res = self.rpc_client.send_and_confirm_transaction(&tx).await;
        // let send_cfg = RpcSendTransactionConfig {
        //     skip_preflight: true,
        //     preflight_commitment: Some(CommitmentLevel::Confirmed),
        //     encoding: Some(UiTransactionEncoding::Base64),
        //     max_retries: Some(0),
        //     min_context_slot: None,
        // };
        let res = self.rpc_client.send_and_confirm_transaction(&tx).await;
        println!("{:?}", res);
    }
}
