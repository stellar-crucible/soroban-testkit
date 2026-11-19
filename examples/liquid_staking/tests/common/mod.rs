use soroban_sdk::{testutils::Address as _, token::StellarAssetClient, Address, Env};
use soroban_testkit_example_liquid_staking::{LiquidStaking, LiquidStakingClient};

#[allow(dead_code)]
pub struct Fixture {
    pub env: Env,
    pub id: Address,
    pub asset: Address,
    pub admin: Address,
    pub user: Address,
}
#[allow(dead_code)]
impl Fixture {
    pub fn new(funding: i128) -> Self {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register(LiquidStaking, ());
        let admin = Address::generate(&env);
        let asset = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let user = Address::generate(&env);
        let mint = StellarAssetClient::new(&env, &asset);
        mint.mint(&user, &funding);
        mint.mint(&admin, &1_000_000_000_000_000);
        let client = LiquidStakingClient::new(&env, &id);
        client.initialize(&admin, &asset);
        Self {
            env,
            id,
            asset,
            admin,
            user,
        }
    }
    pub fn client(&self) -> LiquidStakingClient<'_> {
        LiquidStakingClient::new(&self.env, &self.id)
    }
    pub fn underlying(&self, owner: &Address) -> i128 {
        soroban_sdk::token::Client::new(&self.env, &self.asset).balance(owner)
    }
}
