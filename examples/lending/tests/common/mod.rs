#![allow(dead_code)]

use soroban_sdk::testutils::Ledger as _;
use soroban_sdk::token::{StellarAssetClient, TokenClient};
use soroban_sdk::Address;
use soroban_testkit_example_lending::{
    BorrowingPool, BorrowingPoolClient, CollateralVault, CollateralVaultClient, DebtToken,
    DebtTokenClient, LendingOracle, LendingOracleClient, PoolConfig, VaultConfig,
};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

pub const ONE: i128 = 10_000_000;
pub const START: u64 = 1_700_000_000;
pub const PRICE: i128 = 10_000_000;
pub const MAX_LTV_BPS: u32 = 8_000;
pub const THRESHOLD_BPS: u32 = 11_000;
pub const PENALTY_BPS: u32 = 500;
pub const INTEREST_BPS: u32 = 500;
pub const MAX_PRICE_AGE: u64 = 63_072_000;
pub const FUNDING: i128 = 1_000_000 * ONE;
pub const YEAR: u64 = 31_536_000;

pub struct Lending {
    pub ctx: TestContext,
    pub collateral: Address,
    pub oracle_id: Address,
    pub debt_id: Address,
    pub vault_id: Address,
    pub pool_id: Address,
    pub owner: Address,
    pub liquidator: Address,
    pub other: Address,
}

impl Lending {
    pub fn new() -> Self {
        Self::with_params(INTEREST_BPS)
    }

    pub fn with_params(interest_bps: u32) -> Self {
        let lending = Self::deployed();
        lending.vault().initialize(&VaultConfig {
            pool: lending.pool_id.clone(),
            collateral_token: lending.collateral.clone(),
        });
        lending.pool().initialize(&lending.config(interest_bps));
        lending.oracle().set_price(&PRICE);
        for user in [&lending.owner, &lending.liquidator, &lending.other] {
            lending.fund(user, FUNDING);
        }
        lending
    }

    pub fn deployed() -> Self {
        let ctx = TestContextBuilder::new().with_users(3).build();
        ctx.env.ledger().set_timestamp(START);
        ctx.env.ledger().set_sequence_number(1_000);
        let env = &ctx.env;
        let collateral = env
            .register_stellar_asset_contract_v2(ctx.admin.clone())
            .address();
        let oracle_id = env.register(LendingOracle, ());
        let debt_id = env.register(DebtToken, ());
        let vault_id = env.register(CollateralVault, ());
        let pool_id = env.register(BorrowingPool, ());
        let owner = ctx.user(0).clone();
        let liquidator = ctx.user(1).clone();
        let other = ctx.user(2).clone();

        LendingOracleClient::new(env, &oracle_id).initialize(&ctx.admin);
        DebtTokenClient::new(env, &debt_id).initialize(&pool_id);

        Self {
            ctx,
            collateral,
            oracle_id,
            debt_id,
            vault_id,
            pool_id,
            owner,
            liquidator,
            other,
        }
    }

    pub fn config(&self, interest_bps: u32) -> PoolConfig {
        PoolConfig {
            admin: self.ctx.admin.clone(),
            vault: self.vault_id.clone(),
            oracle: self.oracle_id.clone(),
            debt_token: self.debt_id.clone(),
            max_ltv_bps: MAX_LTV_BPS,
            liquidation_threshold_bps: THRESHOLD_BPS,
            liquidation_penalty_bps: PENALTY_BPS,
            interest_rate_bps: interest_bps,
            max_price_age: MAX_PRICE_AGE,
        }
    }

    pub fn vault(&self) -> CollateralVaultClient<'_> {
        CollateralVaultClient::new(&self.ctx.env, &self.vault_id)
    }

    pub fn pool(&self) -> BorrowingPoolClient<'_> {
        BorrowingPoolClient::new(&self.ctx.env, &self.pool_id)
    }

    pub fn oracle(&self) -> LendingOracleClient<'_> {
        LendingOracleClient::new(&self.ctx.env, &self.oracle_id)
    }

    pub fn debt(&self) -> DebtTokenClient<'_> {
        DebtTokenClient::new(&self.ctx.env, &self.debt_id)
    }

    pub fn collateral_token(&self) -> TokenClient<'_> {
        TokenClient::new(&self.ctx.env, &self.collateral)
    }

    pub fn fund(&self, who: &Address, amount: i128) {
        StellarAssetClient::new(&self.ctx.env, &self.collateral).mint(who, &amount);
    }

    pub fn deposit(&self, who: &Address, amount: i128) {
        self.vault().deposit(who, &amount);
    }

    pub fn borrow(&self, who: &Address, amount: i128) {
        self.pool().borrow(who, &amount);
    }

    pub fn open(&self, who: &Address, collateral: i128, debt: i128) {
        self.deposit(who, collateral);
        if debt > 0 {
            self.borrow(who, debt);
        }
    }

    pub fn set_price(&self, price: i128) {
        self.oracle().set_price(&price);
    }

    pub fn crash_price(&self, percent: i128) -> i128 {
        let price = PRICE * (100 - percent) / 100;
        self.set_price(price);
        price
    }

    pub fn advance_time(&mut self, seconds: u64) {
        self.ctx.advance_time(seconds);
        let price = self.oracle().price().price;
        self.set_price(price);
    }

    pub fn advance_time_without_oracle(&mut self, seconds: u64) {
        self.ctx.advance_time(seconds);
    }

    pub fn now(&self) -> u64 {
        self.ctx.env.ledger().timestamp()
    }

    pub fn arm_liquidator(&self, amount: i128) {
        self.open(&self.liquidator, amount * 20, amount);
    }

    pub fn collateral_usd(&self, collateral: i128, price: i128) -> i128 {
        collateral * price / soroban_testkit_example_lending::PRICE_SCALE
    }
}
