use std::{
    fs::File,
    io::Read,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use ore::{self, state::Proof, utils::AccountDeserialize, BUS_ADDRESSES, BUS_COUNT};
use rand::Rng;
use solana_program::pubkey::Pubkey;
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction, signer::Signer, transaction::Transaction,
};

use crate::{utils::proof_pubkey, Miner};

impl Miner {
    pub async fn mine(&self, threads: u64, buffer_time: u64) {
        // Register, if needed.
        let signer = self.signer();
        self.register().await;

        loop {
            // Run drillx
            let nonce = self.find_hash(signer.pubkey(), buffer_time).await;

            // Submit most difficult hash
            // TODO Set compute budget and price
            let blockhash = self
                .rpc_client
                .get_latest_blockhash()
                .await
                .expect("failed to get blockhash");
            let cu_budget_ix = ComputeBudgetInstruction::set_compute_unit_limit(500_000);
            let reset_ix = ore::instruction::reset(signer.pubkey());
            let mine_ix = ore::instruction::mine(signer.pubkey(), find_bus(), nonce);
            let tx = Transaction::new_signed_with_payer(
                &[cu_budget_ix, reset_ix, mine_ix],
                Some(&signer.pubkey()),
                &[&signer],
                blockhash,
            );
            let res = self.rpc_client.send_and_confirm_transaction(&tx).await;
            println!("{:?}", res);
        }
    }

    // TODO Parallelize search
    async fn find_hash(&self, signer: Pubkey, buffer_time: u64) -> u64 {
        let timer = Instant::now();
        let proof = self.get_proof(signer).await;
        let cutoff_time = get_cutoff(proof, buffer_time);
        let mut best_difficulty = 0;
        let mut best_nonce = 0;
        let mut nonce = 0u64;
        println!("Mining {} sec", cutoff_time);
        loop {
            let hx = drillx::hash(&proof.challenge, &nonce.to_le_bytes());
            let difficulty = drillx::difficulty(hx);
            if difficulty.gt(&best_difficulty) {
                best_difficulty = difficulty;
                best_nonce = nonce;
            }
            if (timer.elapsed().as_secs() as i64).ge(&cutoff_time) {
                if best_difficulty.gt(&ore::MIN_DIFFICULTY) {
                    // Min difficulty requirement
                    break;
                }
            }
            if nonce % 10_000 == 0 {
                println!(
                    "Time: {} sec – Nonce: {} – Difficulty: {}",
                    timer.elapsed().as_secs(),
                    nonce,
                    best_difficulty
                );
            }
            nonce += 1;
        }
        best_nonce
    }

    async fn get_proof(&self, signer: Pubkey) -> Proof {
        let proof_address = proof_pubkey(signer);
        let client = self.rpc_client.clone();
        let data = client
            .get_account_data(&proof_address)
            .await
            .expect("failed to get account");
        *Proof::try_from_bytes(&data).expect("failed to parse")
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
