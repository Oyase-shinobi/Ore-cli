use std::time::{Instant, SystemTime, UNIX_EPOCH};

use logfather::info;
use ore::{self, state::Proof, BUS_ADDRESSES, BUS_COUNT};
use rand::Rng;
use solana_program::pubkey::Pubkey;
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction, signer::Signer, transaction::Transaction,
};

use crate::{utils::get_proof, Miner};

impl Miner {
    pub async fn mine(&self, threads: u64, buffer_time: u64) {
        // Register, if needed.
        let signer = self.signer();
        self.register().await;

        loop {
            // Run drillx
            let nonce = self
                .find_hash_par(signer.pubkey(), buffer_time, threads)
                .await;

            // TODO Submit through send_and_confirm flow
            // Submit most difficult hash
            let blockhash = self
                .rpc_client
                .get_latest_blockhash()
                .await
                .expect("failed to get blockhash");
            let cu_budget_ix = ComputeBudgetInstruction::set_compute_unit_limit(500_000);
            let cu_price_ix = ComputeBudgetInstruction::set_compute_unit_price(self.priority_fee);
            let reset_ix = ore::instruction::reset(signer.pubkey());
            let mine_ix = ore::instruction::mine(signer.pubkey(), find_bus(), nonce);
            let tx = Transaction::new_signed_with_payer(
                &[cu_budget_ix, cu_price_ix, reset_ix, mine_ix],
                Some(&signer.pubkey()),
                &[&signer],
                blockhash,
            );
            let res = self.rpc_client.send_and_confirm_transaction(&tx).await;
            info!("{:?}", res);
        }
    }

    async fn find_hash_par(&self, signer: Pubkey, buffer_time: u64, threads: u64) -> u64 {
        let proof = get_proof(&self.rpc_client, signer).await;
        let cutoff_time = get_cutoff(proof, buffer_time);
        let handles: Vec<_> = (0..threads)
            .map(|i| {
                std::thread::spawn({
                    let proof = proof.clone();
                    move || {
                        let timer = Instant::now();
                        let mut nonce = u64::MAX.saturating_div(threads).saturating_mul(i);
                        let mut best_nonce = nonce;
                        let mut best_difficulty = 0;
                        loop {
                            // Create hash
                            let hx = drillx::hash(&proof.challenge, &nonce.to_le_bytes());
                            let difficulty = drillx::difficulty(hx);

                            // Check difficulty
                            if difficulty.gt(&best_difficulty) {
                                best_nonce = nonce;
                                best_difficulty = difficulty;
                            }

                            // Exit if time has elapsed
                            if nonce % 1000 == 0 {
                                if (timer.elapsed().as_secs() as i64).ge(&cutoff_time) {
                                    if best_difficulty.gt(&ore::MIN_DIFFICULTY) {
                                        // Mine until min difficulty has been met
                                        break;
                                    }
                                }
                            }

                            // Increment nonce
                            nonce += 1;
                        }

                        // Return the best nonce
                        (best_nonce, best_difficulty)
                    }
                })
            })
            .collect();

        // Join handles and return best nonce
        let mut best_nonce = 0;
        let mut best_difficulty = 0;
        for h in handles {
            if let Ok((nonce, difficulty)) = h.join() {
                if difficulty > best_difficulty {
                    best_difficulty = difficulty;
                    best_nonce = nonce;
                }
            }
        }

        best_nonce
    }
}

fn get_cutoff(proof: Proof, buffer_time: u64) -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Failed to get time")
        .as_secs() as i64;
    proof
        .last_hash_at
        .saturating_add(60)
        .saturating_sub(buffer_time as i64)
        .saturating_sub(now)
}

// TODO Better strategy (avoid draining bus)
fn find_bus() -> Pubkey {
    let i = rand::thread_rng().gen_range(0..BUS_COUNT);
    BUS_ADDRESSES[i]
}
