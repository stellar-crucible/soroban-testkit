#![allow(dead_code)]

use soroban_sdk::{testutils::Address as _, token, Address, Env, Vec};
use soroban_testkit_example_portfolio_vault::{AssetConfig, PortfolioVault, PortfolioVaultClient};

pub const ONE: i128 = 10_000_000;

pub struct Fixture {
    pub env: Env,
    pub admin: Address,
    pub user: Address,
    pub trader: Address,
    pub token_a: Address,
    pub token_b: Address,
    pub token_c: Address,
    pub vault: Address,
}

impl Fixture {
    pub fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let trader = Address::generate(&env);

        let token_a = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let token_b = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let token_c = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let vault = env.register(PortfolioVault, ());

        let fx = Self {
            env,
            admin,
            user,
            trader,
            token_a,
            token_b,
            token_c,
            vault,
        };
        fx.initialize_default();
        fx
    }

    pub fn client(&self) -> PortfolioVaultClient<'_> {
        PortfolioVaultClient::new(&self.env, &self.vault)
    }

    pub fn token_admin_a(&self) -> token::StellarAssetClient<'_> {
        token::StellarAssetClient::new(&self.env, &self.token_a)
    }
    pub fn token_admin_b(&self) -> token::StellarAssetClient<'_> {
        token::StellarAssetClient::new(&self.env, &self.token_b)
    }
    pub fn token_admin_c(&self) -> token::StellarAssetClient<'_> {
        token::StellarAssetClient::new(&self.env, &self.token_c)
    }
    pub fn token_a_client(&self) -> token::Client<'_> {
        token::Client::new(&self.env, &self.token_a)
    }
    pub fn token_b_client(&self) -> token::Client<'_> {
        token::Client::new(&self.env, &self.token_b)
    }
    pub fn token_c_client(&self) -> token::Client<'_> {
        token::Client::new(&self.env, &self.token_c)
    }

    pub fn initialize_default(&self) {
        let mut assets = Vec::new(&self.env);
        assets.push_back(AssetConfig {
            token: self.token_a.clone(),
            target_bps: 5000,
        });
        assets.push_back(AssetConfig {
            token: self.token_b.clone(),
            target_bps: 3000,
        });
        assets.push_back(AssetConfig {
            token: self.token_c.clone(),
            target_bps: 2000,
        });
        self.client().initialize(&self.admin, &assets);
        self.client().set_price(&self.admin, &self.token_a, &2i128);
        self.client().set_price(&self.admin, &self.token_b, &1i128);
        self.client().set_price(&self.admin, &self.token_c, &5i128);
    }

    pub fn fund_user(&self, amount_a: i128, amount_b: i128, amount_c: i128) {
        self.token_admin_a().mint(&self.user, &amount_a);
        self.token_admin_b().mint(&self.user, &amount_b);
        self.token_admin_c().mint(&self.user, &amount_c);
    }

    pub fn fund_trader(&self, amount_a: i128, amount_b: i128, amount_c: i128) {
        self.token_admin_a().mint(&self.trader, &amount_a);
        self.token_admin_b().mint(&self.trader, &amount_b);
        self.token_admin_c().mint(&self.trader, &amount_c);
    }

    pub fn deposit_balanced(&self) {
        self.fund_user(100 * ONE, 100 * ONE, 100 * ONE);
        self.client()
            .deposit(&self.user, &self.token_a, &(50 * ONE));
        self.client()
            .deposit(&self.user, &self.token_b, &(60 * ONE));
        self.client().deposit(&self.user, &self.token_c, &(8 * ONE));
    }
}
