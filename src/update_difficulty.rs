use std::str::FromStr;

use solana_program::keccak::Hash as KeccakHash;
use solana_program::pubkey::Pubkey;

use crate::Miner;

impl Miner {
    pub async fn update_difficulty(&self) {
        let signer = Pubkey::from_str("tHCCE3KWKx8i8cDjX2DQ3Z7EMJkScAVwkfxdWz8SqgP").expect("");
        let new_difficulty = KeccakHash::new_from_array([
            0, 0, 0, 16, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
            255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
        ]);
        let ix = ore::instruction::update_difficulty(signer, new_difficulty.into());
        println!("New difficulty: {:?}", new_difficulty.to_string());
        let bs58data = bs58::encode(ix.clone().data).into_string();
        println!("Data: {:?}", bs58data);
        println!("Ix: {:?}", ix);
    }
}
