use std::{
    io::{stdout, Write},
    time::Duration,
};

use solana_client::{
    client_error::{ClientError, ClientErrorKind, Result as ClientResult},
    nonblocking::rpc_client::RpcClient,
};
use solana_program::instruction::Instruction;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    signature::{Signature, Signer},
    transaction::Transaction,
};

use crate::Miner;

const GATEWAY_RETRIES: usize = 10;

impl Miner {
    pub async fn send_and_confirm(&self, ixs: &[Instruction]) -> ClientResult<Signature> {
        let mut stdout = stdout();
        let signer = self.signer();
        let client =
            RpcClient::new_with_commitment(self.cluster.clone(), CommitmentConfig::confirmed());

        // Build tx
        let mut hash = client.get_latest_blockhash().await.unwrap();
        let mut tx = Transaction::new_with_payer(ixs, Some(&signer.pubkey()));
        tx.sign(&[&signer], hash);

        // Sim and prepend cu ixs
        let sim_res = client.simulate_transaction(&tx).await;
        let final_ixs = if let Ok(sim_res) = sim_res {
            let cu_budget_ix = ComputeBudgetInstruction::set_compute_unit_limit(
                sim_res.value.units_consumed.unwrap() as u32 + 1000,
            );
            let cu_price_ix = ComputeBudgetInstruction::set_compute_unit_price(self.priority_fee);
            let mut final_ixs = vec![];
            final_ixs.extend_from_slice(&[cu_budget_ix, cu_price_ix]);
            final_ixs.extend_from_slice(ixs);
            final_ixs
        } else {
            return Err(ClientError {
                request: None,
                kind: ClientErrorKind::Custom("Failed simulation".into()),
            });
        };

        // Rebuild tx with cu ixs
        tx = Transaction::new_with_payer(&final_ixs, Some(&signer.pubkey()));
        tx.sign(&[&signer], hash);

        // Loop
        let mut attempts = 0;
        loop {
            println!("Attempt: {:?}", attempts);
            match client.send_and_confirm_transaction(&tx).await {
                Ok(sig) => {
                    println!("Confirmed: {:?}", sig);
                    return Ok(sig);
                }
                Err(err) => {
                    println!("Error {:?}", err);
                }
            }
            stdout.flush().ok();

            // Retry with new hash
            std::thread::sleep(Duration::from_millis(1000));
            hash = client.get_latest_blockhash().await.unwrap();
            tx.sign(&[&signer], hash);
            attempts += 1;
            if attempts > GATEWAY_RETRIES {
                return Err(ClientError {
                    request: None,
                    kind: ClientErrorKind::Custom("Max retries".into()),
                });
            }
        }
    }
}
