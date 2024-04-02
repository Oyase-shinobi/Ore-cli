use std::{
    io::{stdout, Write},
    sync::{atomic::AtomicBool, Arc, Mutex},
    time::Duration,
};

use chrono::{Duration as ChronoDuration, Utc};
use ore::{self, BUS_ADDRESSES, BUS_COUNT, EPOCH_DURATION, START_AT};
use solana_client::client_error::ClientErrorKind;
use solana_sdk::{
    keccak::{hashv, Hash as KeccakHash},
    signature::Signer,
};

use crate::{
    utils::{get_clock_account, get_proof, get_treasury},
    Miner,
};

// TODO Fetch hardware concurrency dynamically
const NUM_THREADS: u64 = 6;

impl Miner {
    pub async fn mine(&self) {
        // Register, if needed.
        let signer = self.signer();
        self.register().await;

        let mut stdout = stdout();

        // Wait for mining to begin if necessary
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let now_unix_timestamp = Utc::now().timestamp();
            let duration = START_AT - now_unix_timestamp;
            let t = format_duration(duration);
            stdout.write_all(b"\x1b[2J\x1b[3J\x1b[H").ok();
            stdout
                .write_all(format!("Waiting for mining to begin... {}\n", t).as_bytes())
                .ok();
            stdout.flush().ok();
            if START_AT.le(&now_unix_timestamp) {
                break;
            }
        }

        // Start mining loop
        loop {
            // Find a valid hash.
            let treasury = get_treasury(self.cluster.clone()).await;
            let proof = get_proof(self.cluster.clone(), signer.pubkey()).await;

            // Escape sequence that clears the screen and the scrollback buffer
            stdout.write_all(b"\x1b[2J\x1b[3J\x1b[H").ok();
            stdout
                .write_all(format!("Searching for valid hash...\n").as_bytes())
                .ok();
            let (next_hash, nonce) =
                self.find_next_hash_par(proof.hash.into(), treasury.difficulty.into());
            stdout
                .write_all(format!("\nSubmitting hash for validation... \n").as_bytes())
                .ok();
            stdout.flush().ok();

            // Submit mine tx.
            let mut bus_id = 0;
            let mut invalid_busses: Vec<u8> = vec![];
            let mut needs_reset = false;
            'submit: loop {
                // Find a valid bus.
                if invalid_busses.len().eq(&(BUS_COUNT as usize)) {
                    // All busses are drained. Wait until next epoch.
                    std::thread::sleep(std::time::Duration::from_millis(1000));
                }
                if invalid_busses.contains(&bus_id) {
                    println!("Bus {} is empty... ", bus_id);
                    bus_id += 1;
                    if bus_id.ge(&(BUS_COUNT as u8)) {
                        std::thread::sleep(Duration::from_secs(1));
                        bus_id = 0;
                    }
                }

                // Reset if epoch has ended
                let treasury = get_treasury(self.cluster.clone()).await;
                let clock = get_clock_account(self.cluster.clone()).await;
                let threshold = treasury.last_reset_at.saturating_add(EPOCH_DURATION);
                if clock.unix_timestamp.ge(&threshold) || needs_reset {
                    let reset_ix = ore::instruction::reset(signer.pubkey());
                    self.send_and_confirm(&[reset_ix])
                        .await
                        .expect("Transaction failed");
                    needs_reset = false;
                }

                // Submit request.
                let ix_mine = ore::instruction::mine(
                    signer.pubkey(),
                    BUS_ADDRESSES[bus_id as usize],
                    next_hash.into(),
                    nonce,
                );
                match self.send_and_confirm(&[ix_mine]).await {
                    Ok(sig) => {
                        stdout.write(format!("Success: {}", sig).as_bytes()).ok();
                        break;
                    }
                    Err(err) => match err.kind {
                        ClientErrorKind::Custom(msg) => {
                            if msg.contains("Bus insufficient") {
                                invalid_busses.push(bus_id);
                            } else if msg.contains("Needs reset") {
                                needs_reset = true;
                            } else if msg.contains("Hash invalid") {
                                break 'submit;
                            } else {
                                stdout
                                    .write_all(format!("\n{:?} \n", msg.to_string()).as_bytes())
                                    .ok();
                            }
                        }
                        _ => {
                            stdout
                                .write_all(format!("\nUnhandled error {:?} \n", err).as_bytes())
                                .ok();
                        }
                    },
                }
            }
        }
    }

    fn _find_next_hash(&self, hash: KeccakHash, difficulty: KeccakHash) -> (KeccakHash, u64) {
        let signer = self.signer();
        let mut next_hash: KeccakHash;
        let mut nonce = 0u64;
        loop {
            next_hash = hashv(&[
                hash.to_bytes().as_slice(),
                signer.pubkey().to_bytes().as_slice(),
                nonce.to_le_bytes().as_slice(),
            ]);
            if next_hash.le(&difficulty) {
                break;
            } else {
                println!("Invalid hash: {} Nonce: {:?}", next_hash.to_string(), nonce);
            }
            nonce += 1;
        }
        (next_hash, nonce)
    }

    fn find_next_hash_par(&self, hash: KeccakHash, difficulty: KeccakHash) -> (KeccakHash, u64) {
        let found_solution = Arc::new(AtomicBool::new(false));
        let solution = Arc::new(Mutex::<(KeccakHash, u64)>::new((
            KeccakHash::new_from_array([0; 32]),
            0,
        )));
        let signer = self.signer();
        let pubkey = signer.pubkey();
        let thread_handles: Vec<_> = (0..NUM_THREADS)
            .map(|i| {
                std::thread::spawn({
                    let found_solution = found_solution.clone();
                    let solution = solution.clone();
                    let mut stdout = stdout();
                    move || {
                        let n = u64::MAX.saturating_div(NUM_THREADS).saturating_mul(i);
                        let mut next_hash: KeccakHash;
                        let mut nonce: u64 = n;
                        loop {
                            next_hash = hashv(&[
                                hash.to_bytes().as_slice(),
                                pubkey.to_bytes().as_slice(),
                                nonce.to_le_bytes().as_slice(),
                            ]);
                            if nonce % 10_000 == 0 {
                                if found_solution.load(std::sync::atomic::Ordering::Relaxed) {
                                    return;
                                }
                                if n == 0 {
                                    stdout
                                        .write_all(
                                            format!("\r{}", next_hash.to_string()).as_bytes(),
                                        )
                                        .ok();
                                }
                            }
                            if next_hash.le(&difficulty) {
                                stdout
                                    .write_all(format!("\r{}", next_hash.to_string()).as_bytes())
                                    .ok();
                                found_solution.store(true, std::sync::atomic::Ordering::Relaxed);
                                let mut w_solution = solution.lock().expect("failed to lock mutex");
                                *w_solution = (next_hash, nonce);
                                return;
                            }
                            nonce += 1;
                        }
                    }
                })
            })
            .collect();

        for thread_handle in thread_handles {
            thread_handle.join().unwrap();
        }

        let r_solution = solution.lock().expect("Failed to get lock");
        *r_solution
    }
}

pub fn format_duration(seconds: i64) -> String {
    let duration = ChronoDuration::try_seconds(seconds).unwrap();
    let hours = duration.num_hours();
    let minutes = duration.num_minutes() % 60;
    let seconds = duration.num_seconds() % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}
