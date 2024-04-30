use std::str::FromStr;

use solana_program::pubkey::Pubkey;
use solana_sdk::signature::Signer;

use crate::{
    utils::{get_config, get_proof},
    Miner,
};

impl Miner {
    pub async fn rewards(&self) {
        let config = get_config(&self.rpc_client).await;
        let base_reward_rate = config.base_reward_rate;
        let base_difficulty = ore::MIN_DIFFICULTY;

        let mut s =
            format!("{}: {} ORE", base_difficulty, format_ore(base_reward_rate)).to_string();
        for i in 1..32 {
            let reward_rate = base_reward_rate.saturating_mul(2u64.saturating_pow(i));
            s = format!(
                "{}\n{}: {} ORE",
                s,
                base_difficulty + i,
                format_ore(reward_rate)
            );
        }
        println!("{}", s);
        // let address = if let Some(address) = address {
        //     if let Ok(address) = Pubkey::from_str(&address) {
        //         address
        //     } else {
        //         println!("Invalid address: {:?}", address);
        //         return;
        //     }
        // } else {
        //     self.signer().pubkey()
        // };
        // let proof = get_proof(&self.rpc_client, address).await;
        // let amount = (proof.balance as f64) / 10f64.powf(ore::TOKEN_DECIMALS as f64);
        // println!("{:} ORE", amount);
    }
}

fn format_ore(nore: u64) -> f64 {
    (nore as f64) / 10f64.powf(ore::TOKEN_DECIMALS as f64)
}
