use solana_program::keccak::Hash as KeccakHash;
use solana_sdk::signature::Signer;

use crate::Miner;

impl Miner {
    pub async fn update_difficulty(&self) {
        let signer = self.signer();
        let new_difficulty = KeccakHash::new_from_array([
            0, 0, 0, 32, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
            255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
        ]);
        let ix = ore::instruction::update_difficulty(signer.pubkey(), new_difficulty.into());
        self.send_and_confirm(&[ix])
            .await
            .expect("Transaction failed");
    }
}
