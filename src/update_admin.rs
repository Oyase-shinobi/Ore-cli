use std::str::FromStr;

use solana_program::pubkey::Pubkey;
use solana_sdk::signature::Signer;

use crate::{args::UpdateAdminArgs, Miner};

impl Miner {
    pub async fn update_admin(&self, args: UpdateAdminArgs) {
        let signer = self.signer();
        let new_admin = Pubkey::from_str(args.new_admin.as_str()).unwrap();
        let ix = ore::instruction::update_admin(signer.pubkey(), new_admin);
        let bs58data = bs58::encode(ix.clone().data).into_string();
        println!("{:?}", ix);
        println!("{:?}", bs58data);
    }
}
