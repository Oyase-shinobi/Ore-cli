use solana_sdk::{signature::Signer, transaction::Transaction};

use crate::Miner;

impl Miner {
    pub async fn pause(&self) {
        let blockhash = self.rpc_client.get_latest_blockhash().await.unwrap();
        let ix = ore::instruction::pause(self.signer().pubkey(), false);
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&self.signer().pubkey()),
            &[&self.signer()],
            blockhash,
        );
        let res = self.rpc_client.send_and_confirm_transaction(&tx).await;
        println!("{:?}", res);
    }
}
