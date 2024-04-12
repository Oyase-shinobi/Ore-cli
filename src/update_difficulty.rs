use solana_program::keccak::Hash as KeccakHash;
use solana_sdk::signature::Signer;

use crate::Miner;

impl Miner {
    pub async fn update_difficulty(&self) {
        let signer = self.signer();
        let new_difficulty = KeccakHash::new_from_array([
            0, 0, 0, 16, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
            255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
        ]);
        let ix = ore::instruction::update_difficulty(signer.pubkey(), new_difficulty.into());
        println!("New difficulty: {:?}", new_difficulty.to_string());
        let bs58data = bs58::encode(ix.data).into_string();
        println!("Data: {:?}", bs58data);
    }
}
