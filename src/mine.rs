use std::{
    io::{stdout, Write},
    sync::{atomic::AtomicBool, Arc, Mutex},
};

use ore::{self, BUS_ADDRESSES, BUS_COUNT, EPOCH_DURATION};
use solana_client::{client_error::ClientErrorKind, nonblocking::rpc_client::RpcClient};
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    keccak::{hashv, Hash as KeccakHash},
    signature::Signer,
    transaction::Transaction,
};

use crate::{
    utils::{get_clock_account, get_proof, get_treasury},
    Miner,
};

const COMPUTE_BUDGET: u32 = 3230;

// TODO Fetch hardware concurrency dynamically
const NUM_THREADS: u64 = 6;

impl<'a> Miner<'a> {
    pub async fn mine(&self) {
        // Register, if needed.
        self.register().await;

        let mut stdout = stdout();
        // stdout.queue(cursor::SavePosition).unwrap();

        // Start mining loop
        loop {
            // Find a valid hash.
            let treasury = get_treasury(self.cluster.clone()).await;
            let proof = get_proof(self.cluster.clone(), self.signer.pubkey()).await;

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
            let client =
                RpcClient::new_with_commitment(self.cluster.clone(), CommitmentConfig::processed());
            let mut bus_id = 0;
            let mut invalid_busses: Vec<u8> = vec![];
            let recent_blockhash = client.get_latest_blockhash().await.unwrap();
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
                        bus_id = 0;
                    }
                }

                // Reset if epoch has ended
                let treasury = get_treasury(self.cluster.clone()).await;
                let clock = get_clock_account(self.cluster.clone()).await;
                let threshold = treasury.last_reset_at.saturating_add(EPOCH_DURATION);
                if clock.unix_timestamp.ge(&threshold) {
                    let reset_ix = ore::instruction::reset(self.signer.pubkey());
                    let tx = Transaction::new_signed_with_payer(
                        &[reset_ix],
                        Some(&self.signer.pubkey()),
                        &[self.signer],
                        recent_blockhash,
                    );
                    client.send_and_confirm_transaction(&tx).await.ok();
                }

                // Submit request.
                let ix_cu_budget = ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_BUDGET);
                let ix_mine = ore::instruction::mine(
                    self.signer.pubkey(),
                    BUS_ADDRESSES[bus_id as usize],
                    next_hash.into(),
                    nonce,
                );
                let tx = Transaction::new_signed_with_payer(
                    &[ix_cu_budget, ix_mine],
                    Some(&self.signer.pubkey()),
                    &[self.signer],
                    recent_blockhash,
                );
                let result = client.send_and_confirm_transaction(&tx).await;
                match result {
                    Ok(sig) => {
                        stdout.write(format!("Success: {}", sig).as_bytes()).ok();
                        // println!("Sig: {}", sig);
                        break;
                    }
                    Err(err) => {
                        match err.kind {
                            ClientErrorKind::RpcError(err) => {
                                // TODO Why is BusInsufficientFunds an RpcError but EpochNeedsReset is a TransactionError ?
                                //      Unhandled error Error { request: None, kind: TransactionError(InstructionError(0, Custom(6003))) }
                                //      thread 'main' panicked at 'Failed to submit transaction: SolanaClientError(Error { request: None, kind: TransactionError(InstructionError(0, Custom(6000))) })', src/main.rs:193:26
                                if err.to_string().contains("custom program error: 0x5") {
                                    // Bus has no remaining funds. Use a different one.
                                    invalid_busses.push(bus_id);
                                } else if err
                                    .to_string()
                                    .contains("This transaction has already been processed")
                                {
                                    break 'submit;
                                } else {
                                    stdout
                                        .write_all(format!("\n{:?} \n", err.to_string()).as_bytes())
                                        .ok();
                                }
                            }
                            _ => {
                                stdout
                                    .write_all(format!("\nUnhandled error {:?} \n", err).as_bytes())
                                    .ok();
                            }
                        }
                    }
                }
            }
        }
    }

    fn _find_next_hash(&self, hash: KeccakHash, difficulty: KeccakHash) -> (KeccakHash, u64) {
        let mut next_hash: KeccakHash;
        let mut nonce = 0u64;
        loop {
            next_hash = hashv(&[
                hash.to_bytes().as_slice(),
                self.signer.pubkey().to_bytes().as_slice(),
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
        let pubkey = self.signer.pubkey();
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
