#![allow(dead_code)]

use soroban_sdk::token::{StellarAssetClient, TokenClient};
use soroban_sdk::Address;
use soroban_testkit_example_cdp::{
    CdpConfig, CdpEngine, CdpEngineClient, PriceOracle, PriceOracleClient, SyntheticToken,
    SyntheticTokenClient,
};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

/// One whole unit of a 7-decimal asset (XLM or sUSD).
pub const ONE: i128 = 10_000_000;
/// XLM at $0.10, scaled by `PRICE_SCALE`.
pub const XLM_PRICE: i128 = 1_000_000;
/// 150% minimum collateralization ratio.
pub const MCR_BPS: u32 = 15_000;
/// 10% liquidation bonus.
pub const PENALTY_BPS: u32 = 1_000;
/// 2% annual stability fee.
pub const FEE_BPS: u32 = 200;
pub const MAX_PRICE_AGE: u64 = 3_600;
/// XLM each actor starts with.
pub const FUNDING: i128 = 1_000_000 * ONE;
pub const YEAR: u64 = 31_536_000;

/// The deployed protocol: an XLM Stellar Asset Contract as collateral, a price
/// oracle, the sUSD synthetic token, and the CDP engine that ties them together.
pub struct Cdp {
    pub ctx: TestContext,
    pub collateral: Address,
    pub oracle_id: Address,
    pub synth_id: Address,
    pub engine_id: Address,
    /// The borrower most tests follow.
    pub owner: Address,
    /// A keeper that liquidates unhealthy positions.
    pub liquidator: Address,
    /// A second, independent borrower.
    pub other: Address,
}

impl Cdp {
    /// The protocol with the default parameters and a fresh $0.10 price.
    pub fn new() -> Self {
        Self::with_params(MCR_BPS, PENALTY_BPS, FEE_BPS)
    }

    pub fn with_fee(fee_bps: u32) -> Self {
        Self::with_params(MCR_BPS, PENALTY_BPS, fee_bps)
    }

    pub fn with_params(mcr_bps: u32, penalty_bps: u32, fee_bps: u32) -> Self {
        let cdp = Self::deployed();
        cdp.engine()
            .initialize(&cdp.config(mcr_bps, penalty_bps, fee_bps));
        cdp.oracle().set_price(&XLM_PRICE);
        for user in [&cdp.owner, &cdp.liquidator, &cdp.other] {
            cdp.fund(user, FUNDING);
        }
        cdp
    }

    /// Every contract registered and the oracle and token initialized, but the
    /// engine left uninitialized and no price published.
    pub fn deployed() -> Self {
        let ctx = TestContextBuilder::new().with_users(3).build();
        let env = &ctx.env;
        let collateral = env
            .register_stellar_asset_contract_v2(ctx.admin.clone())
            .address();
        let oracle_id = env.register(PriceOracle, ());
        let synth_id = env.register(SyntheticToken, ());
        let engine_id = env.register(CdpEngine, ());

        PriceOracleClient::new(env, &oracle_id).initialize(&ctx.admin);
        SyntheticTokenClient::new(env, &synth_id).initialize(&engine_id);

        let owner = ctx.user(0).clone();
        let liquidator = ctx.user(1).clone();
        let other = ctx.user(2).clone();
        Self {
            ctx,
            collateral,
            oracle_id,
            synth_id,
            engine_id,
            owner,
            liquidator,
            other,
        }
    }

    pub fn config(&self, mcr_bps: u32, penalty_bps: u32, fee_bps: u32) -> CdpConfig {
        CdpConfig {
            admin: self.ctx.admin.clone(),
            collateral_token: self.collateral.clone(),
            synth_token: self.synth_id.clone(),
            oracle: self.oracle_id.clone(),
            mcr_bps,
            liquidation_penalty_bps: penalty_bps,
            stability_fee_bps: fee_bps,
            max_price_age: MAX_PRICE_AGE,
        }
    }

    pub fn engine(&self) -> CdpEngineClient<'_> {
        CdpEngineClient::new(&self.ctx.env, &self.engine_id)
    }

    pub fn oracle(&self) -> PriceOracleClient<'_> {
        PriceOracleClient::new(&self.ctx.env, &self.oracle_id)
    }

    pub fn synth(&self) -> SyntheticTokenClient<'_> {
        SyntheticTokenClient::new(&self.ctx.env, &self.synth_id)
    }

    /// The collateral token as a plain token client.
    pub fn xlm(&self) -> TokenClient<'_> {
        TokenClient::new(&self.ctx.env, &self.collateral)
    }

    pub fn fund(&self, who: &Address, amount: i128) {
        StellarAssetClient::new(&self.ctx.env, &self.collateral).mint(who, &amount);
    }

    /// Deposit `collateral` and, when `debt > 0`, borrow `debt` against it.
    pub fn open(&self, owner: &Address, collateral: i128, debt: i128) {
        self.engine().deposit(owner, &collateral);
        if debt > 0 {
            self.engine().borrow(owner, &debt);
        }
    }

    /// Give the liquidator `amount` sUSD by opening its own deeply
    /// overcollateralized position, safe through a 95% price crash.
    pub fn arm_liquidator(&self, amount: i128) {
        let collateral = amount * 40 * PRICE_SCALE_UNITS / XLM_PRICE;
        self.open(&self.liquidator, collateral, amount);
    }

    pub fn set_price(&self, price: i128) {
        self.oracle().set_price(&price);
    }

    /// Publish a price `percent`% below the starting price.
    pub fn crash_price(&self, percent: i128) -> i128 {
        let price = XLM_PRICE * (100 - percent) / 100;
        self.set_price(price);
        price
    }

    pub fn now(&self) -> u64 {
        self.ctx.env.ledger().timestamp()
    }

    /// Move the clock forward and re-publish the current price, the way a
    /// live oracle keeps its feed fresh.
    pub fn advance_time(&mut self, seconds: u64) {
        self.ctx.advance_time(seconds);
        let price = self.oracle().price().price;
        self.set_price(price);
    }

    /// Move the clock forward without touching the oracle.
    pub fn advance_time_without_oracle(&mut self, seconds: u64) {
        self.ctx.advance_time(seconds);
    }
}

/// `PRICE_SCALE` as used to turn a USD amount into collateral units.
pub const PRICE_SCALE_UNITS: i128 = soroban_testkit_example_cdp::PRICE_SCALE;
