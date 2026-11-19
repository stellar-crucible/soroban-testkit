use soroban_sdk::{testutils::Address as _, token::StellarAssetClient, Address, Env};
use soroban_testkit_example_crowdfunding::{Crowdfunding, CrowdfundingClient};

pub struct Fixture { pub env: Env, pub id: Address, pub token: Address, pub creator: Address, pub backer: Address }
impl Fixture {
    pub fn new(funding: i128) -> Self {
        let env = Env::default(); env.mock_all_auths();
        let id = env.register(Crowdfunding, ());
        let token = env.register_stellar_asset_contract_v2(Address::generate(&env)).address();
        let creator = Address::generate(&env); let backer = Address::generate(&env);
        StellarAssetClient::new(&env, &token).mint(&backer, &funding);
        Self { env, id, token, creator, backer }
    }
    pub fn client(&self) -> CrowdfundingClient<'_> { CrowdfundingClient::new(&self.env, &self.id) }
    pub fn open(&self, goal: i128, deadline: u64) { self.client().create(&1, &self.creator, &self.token, &goal, &deadline, &2); }
    pub fn backer_balance(&self) -> i128 { soroban_sdk::token::Client::new(&self.env, &self.token).balance(&self.backer) }
    pub fn escrow_balance(&self) -> i128 { soroban_sdk::token::Client::new(&self.env, &self.token).balance(&self.id) }
}
