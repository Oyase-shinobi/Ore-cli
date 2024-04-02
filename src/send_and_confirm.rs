use std::{
    io::{stdout, Write},
    time::Duration,
};

use solana_client::{
    client_error::{ClientError, ClientErrorKind, Result as ClientResult},
    nonblocking::rpc_client::RpcClient,
    rpc_config::RpcSendTransactionConfig,
};
use solana_program::instruction::Instruction;
use solana_sdk::{
    commitment_config::{CommitmentConfig, CommitmentLevel},
    signature::{Signature, Signer},
    transaction::Transaction,
};
use solana_transaction_status::UiTransactionEncoding;

use crate::Miner;

const RPC_RETRIES: usize = 3;
const GATEWAY_RETRIES: usize = 10;

impl Miner {
    pub async fn send_and_confirm(&self, ixs: &[Instruction]) -> ClientResult<Signature> {
        let mut stdout = stdout();
        let signer = self.signer();
        let client =
            RpcClient::new_with_commitment(self.cluster.clone(), CommitmentConfig::confirmed());
        let mut hash = client.get_latest_blockhash().await.unwrap();
        let mut cfg = RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: Some(CommitmentLevel::Confirmed),
            encoding: Some(UiTransactionEncoding::Base64),
            max_retries: Some(RPC_RETRIES),
            min_context_slot: None,
        };
        let mut tx = Transaction::new_with_payer(ixs, Some(&signer.pubkey()));
        tx.sign(&[&signer], hash);
        let mut attempts = 0;
        loop {
            println!("Attempt: {:?}", attempts);
            match client.send_transaction_with_config(&tx, cfg).await {
                Ok(sig) => {
                    match client
                        .confirm_transaction_with_commitment(&sig, CommitmentConfig::confirmed())
                        .await
                    {
                        Ok(confirmed) => {
                            if confirmed.value {
                                return Ok(sig);
                            }
                        }
                        Err(err) => {
                            println!("Error: {:?}", err);
                        }
                    }
                }
                Err(err) => {
                    println!("Error {:?}", err);
                    return Err(err);
                }
            }
            stdout.flush().ok();

            // Retry
            std::thread::sleep(Duration::from_millis(1000));
            hash = client.get_latest_blockhash().await.unwrap();
            cfg = RpcSendTransactionConfig {
                skip_preflight: true,
                preflight_commitment: Some(CommitmentLevel::Confirmed),
                encoding: Some(UiTransactionEncoding::Base64),
                max_retries: Some(RPC_RETRIES),
                min_context_slot: None,
            };
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
