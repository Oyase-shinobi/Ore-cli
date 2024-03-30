use ore::{state::Bus, utils::AccountDeserialize, BUS_ADDRESSES};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;

use crate::Miner;

impl<'a> Miner<'a> {
    pub async fn busses(&self) {
        let client =
            RpcClient::new_with_commitment(self.cluster.clone(), CommitmentConfig::processed());
        for address in BUS_ADDRESSES.iter() {
            let data = client.get_account_data(address).await.unwrap();
            match Bus::try_from_bytes(&data) {
                Ok(bus) => {
                    println!("Bus {}: {:} ORE", bus.id, bus.rewards);
                }
                Err(_) => {}
            }
        }
    }
}
