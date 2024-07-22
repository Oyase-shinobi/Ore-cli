use colored::Colorize;

use crate::{utils::get_config, Miner};

impl Miner {
    pub async fn config(&self) {
        let config = get_config(&self.rpc_client).await;
        println!("{}: {}", "Last reset".bold(), config.last_reset_at);
        println!("{}: {}", "Min difficulty".bold(), config.min_difficulty);
        println!("{}: {}", "Base reward rate".bold(), config.base_reward_rate);
    }
}
