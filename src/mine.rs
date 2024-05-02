use std::time::{Instant, SystemTime, UNIX_EPOCH};

use logfather::{info, trace};
use ore::{self, state::Proof, BUS_ADDRESSES, BUS_COUNT};
use rand::Rng;
use solana_program::pubkey::Pubkey;
use solana_sdk::signer::Signer;

use crate::{
    send_and_confirm::ComputeBudget,
    utils::{amount_u64_to_string, get_proof},
    Miner,
};

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

            // Submit most difficult hash
            let reset_ix = ore::instruction::reset(signer.pubkey());
            let mine_ix = ore::instruction::mine(signer.pubkey(), find_bus(), nonce);
            self.send_and_confirm(&[reset_ix, mine_ix], ComputeBudget::Fixed(500_000), false)
                .await
                .ok();
        }
    }

    async fn find_hash_par(&self, signer: Pubkey, buffer_time: u64, threads: u64) -> u64 {
        let proof = get_proof(&self.rpc_client, signer).await;
        info!("Stake balance: {} ORE", amount_u64_to_string(proof.balance));
        let cutoff_time = get_cutoff(proof, buffer_time);
        let handles: Vec<_> = (0..threads)
            .map(|i| {
                std::thread::spawn({
                    let proof = proof.clone();
                    move || {
                        let timer = Instant::now();
                        let first_nonce = u64::MAX.saturating_div(threads).saturating_mul(i);
                        let mut nonce = first_nonce;
                        let mut best_nonce = nonce;
                        let mut best_difficulty = 0;
                        let mut best_hash = [0; 32];
                        loop {
                            // Create hash
                            let hx = drillx::hash(&proof.challenge, &nonce.to_le_bytes());
                            let difficulty = drillx::difficulty(hx);

                            // Check difficulty
                            if difficulty.gt(&best_difficulty) {
                                best_nonce = nonce;
                                best_difficulty = difficulty;
                                best_hash = hx;
                            }

                            // Exit if time has elapsed
                            if nonce % 10_000 == 0 {
                                if (timer.elapsed().as_secs() as i64).ge(&cutoff_time) {
                                    if best_difficulty.gt(&ore::MIN_DIFFICULTY) {
                                        // Mine until min difficulty has been met
                                        break;
                                    }
                                } else {
                                    trace!(
                                        "(Core {}) Hashes searched: {} – Best difficulty: {} – Time remaining: {} sec",
                                        i,
                                        nonce.saturating_sub(first_nonce),
                                        best_difficulty,
                                        cutoff_time
                                            .saturating_sub(timer.elapsed().as_secs() as i64),
                                    );
                                }
                            }

                            // Increment nonce
                            nonce += 1;
                        }

                        // Return the best nonce
                        (best_nonce, best_difficulty, best_hash)
                    }
                })
            })
            .collect();

        // Join handles and return best nonce
        let mut best_nonce = 0;
        let mut best_difficulty = 0;
        let mut best_hash = [0; 32];
        for h in handles {
            if let Ok((nonce, difficulty, hash)) = h.join() {
                if difficulty > best_difficulty {
                    best_difficulty = difficulty;
                    best_nonce = nonce;
                    best_hash = hash;
                }
            }
        }

        info!(
            "Best hash: {} (difficulty: {})",
            bs58::encode(best_hash).into_string(),
            best_difficulty
        );

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

// TODO Pick a better strategy (avoid draining bus)
fn find_bus() -> Pubkey {
    let i = rand::thread_rng().gen_range(0..BUS_COUNT);
    BUS_ADDRESSES[i]
}
