use std::{sync::Arc, time::Instant};

use ore::{self, state::Proof, BUS_ADDRESSES, BUS_COUNT, EPOCH_DURATION};
use rand::Rng;
use solana_program::pubkey::Pubkey;
use solana_rpc_client::spinner;
use solana_sdk::signer::Signer;

#[cfg(feature = "gpu")]
use drillx::{
    gpu::{drill_hash, gpu_init, set_noise},
    noise::NOISE,
};

use crate::{
    send_and_confirm::ComputeBudget,
    utils::{amount_u64_to_string, get_clock, get_config, get_proof},
    Miner,
};

impl Miner {
    pub async fn mine(&self, threads: u64, buffer_time: u64) {
        // Register, if needed.
        let signer = self.signer();
        self.register().await;

        loop {
            // Fetch proof
            let proof = get_proof(&self.rpc_client, signer.pubkey()).await;
            println!(
                "\nStake balance: {} ORE",
                amount_u64_to_string(proof.balance)
            );

            // Run drillx (gpu)
            #[cfg(feature = "gpu")]
            let nonce = self.find_hash_gpu(proof, buffer_time).await;

            // Run drillx
            #[cfg(not(feature = "gpu"))]
            let nonce = self.find_hash_par(proof, buffer_time, threads).await;

            // Submit most difficult hash
            let mut ixs = vec![];
            if self.needs_reset().await {
                ixs.push(ore::instruction::reset(signer.pubkey()));
            }
            ixs.push(ore::instruction::mine(signer.pubkey(), find_bus(), nonce));
            self.send_and_confirm(&ixs, ComputeBudget::Fixed(250_000), false)
                .await
                .ok();
        }
    }

    #[cfg(feature = "gpu")]
    async fn find_hash_gpu(&self, proof: Proof, buffer_time: u64) -> u64 {
        let progress_bar = Arc::new(spinner::new_progress_bar());
        progress_bar.set_message("Mining (gpu)...");

        // TODO Add buffer time to gpu init interface to set cycle time
        // TODO Other args for gpu init
        unsafe {
            gpu_init();
            set_noise(NOISE.as_usize_slice().as_ptr());
        }

        // Hash on gpu
        let challenge = proof.challenge;
        let mut gpu_nonce = [0; 8];
        unsafe {
            drill_hash(challenge.as_ptr(), gpu_nonce.as_mut_ptr());
        }

        // Return nonce
        u64::from_le_bytes(gpu_nonce)
    }

    async fn find_hash_par(&self, proof: Proof, buffer_time: u64, threads: u64) -> u64 {
        // Check num threads
        self.check_num_cores(threads);

        // Calc cutoff time
        let cutoff_time = self.get_cutoff(proof, buffer_time).await;

        // Dispatch job to each thread
        let progress_bar = Arc::new(spinner::new_progress_bar());
        progress_bar.set_message("Mining...");
        let handles: Vec<_> = (0..threads)
            .map(|i| {
                std::thread::spawn({
                    let proof = proof.clone();
                    let progress_bar = progress_bar.clone();
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
                                } else if i == 0 {
                                    progress_bar.set_message(format!(
                                        "Mining... ({} sec remaining)",
                                        cutoff_time
                                            .saturating_sub(timer.elapsed().as_secs() as i64),
                                    ));
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

        // Update log
        progress_bar.finish_with_message(format!(
            "Best hash: {} (difficulty: {})",
            bs58::encode(best_hash).into_string(),
            best_difficulty
        ));

        best_nonce
    }

    pub fn check_num_cores(&self, threads: u64) {
        // Check num threads
        let num_cores = num_cpus::get() as u64;
        if threads.gt(&num_cores) {
            println!(
                "WARNING: Number of threads ({}) exceeds available cores ({})",
                threads, num_cores
            );
        }
    }

    async fn needs_reset(&self) -> bool {
        let clock = get_clock(&self.rpc_client).await;
        let config = get_config(&self.rpc_client).await;
        config
            .last_reset_at
            .saturating_add(EPOCH_DURATION)
            .le(&clock.unix_timestamp)
    }

    async fn get_cutoff(&self, proof: Proof, buffer_time: u64) -> i64 {
        let clock = get_clock(&self.rpc_client).await;
        proof
            .last_hash_at
            .saturating_add(60)
            .saturating_sub(buffer_time as i64)
            .saturating_sub(clock.unix_timestamp)
    }
}

// TODO Pick a better strategy (avoid draining bus)
fn find_bus() -> Pubkey {
    let i = rand::thread_rng().gen_range(0..BUS_COUNT);
    BUS_ADDRESSES[i]
}
