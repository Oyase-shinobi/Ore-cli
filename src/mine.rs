use std::{fs::File, io::Read, time::Instant};

use ore::{self, state::Proof, utils::AccountDeserialize, BUS_ADDRESSES, BUS_COUNT};
use rand::Rng;
use solana_program::pubkey::Pubkey;
use solana_sdk::{signer::Signer, transaction::Transaction};

use crate::{utils::proof_pubkey, Miner};

impl Miner {
    pub async fn mine(&self, threads: u64) {
        // Register, if needed.
        let signer = self.signer();
        self.register().await;

        // Read noise file
        let mut file = File::open("noise.txt").unwrap();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();
        let noise = buffer.as_slice();

        loop {
            // Run drillx
            let nonce = self.find_hash(signer.pubkey(), noise).await;

            // Submit most difficult hash
            // TODO Set compute budget and price
            let blockhash = self
                .rpc_client
                .get_latest_blockhash()
                .await
                .expect("failed to get blockhash");
            let reset_ix = ore::instruction::reset(signer.pubkey());
            let mine_ix = ore::instruction::mine(signer.pubkey(), find_bus(), nonce);
            let tx = Transaction::new_signed_with_payer(
                &[reset_ix, mine_ix],
                Some(&signer.pubkey()),
                &[&signer],
                blockhash,
            );
            let res = self.rpc_client.send_and_confirm_transaction(&tx).await;
            println!("{:?}", res);
        }
    }

    // TODO Parallelize search
    async fn find_hash(&self, signer: Pubkey, noise: &[u8]) -> u64 {
        let timer = Instant::now();
        let proof = self.get_proof(signer).await;
        let cutoff_time = get_cutoff(proof);
        let mut difficulty = 0;
        let mut best_nonce = 0;
        let mut nonce = 0u64;
        println!("Mining");
        loop {
            let hx = drillx::hash(&proof.challenge, &nonce.to_le_bytes(), noise);
            let d = drillx::difficulty(hx);
            if d.gt(&difficulty) {
                difficulty = d;
                best_nonce = nonce;
            }
            if timer.elapsed().as_secs().ge(&cutoff_time) {
                break;
            }
            if nonce % 10_000 == 0 {
                println!("{}", nonce);
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

fn get_cutoff(proof: Proof) -> u64 {
    const BUFFER_TIME: i64 = 10;
    proof
        .last_hash_at
        .saturating_add(60)
        .saturating_sub(BUFFER_TIME) as u64
}

fn find_bus() -> Pubkey {
    let i = rand::thread_rng().gen_range(0..BUS_COUNT);
    BUS_ADDRESSES[i]
}
