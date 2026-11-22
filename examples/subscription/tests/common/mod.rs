#![allow(dead_code)]

use soroban_sdk::token::{Client as TokenClient, StellarAssetClient};
use soroban_sdk::{Address, Env};
use soroban_testkit_example_subscription::{
    Subscription, SubscriptionContract, SubscriptionContractClient, SubscriptionError,
};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;
use soroban_testkit_generators::strategies::AddressBytes;

pub const FEE: i128 = 500;
pub const INTERVAL: u64 = 86_400;
pub const START: u64 = 1_700_000_000;
pub const NEXT_BILLING: u64 = START + INTERVAL;
pub const APPROVAL_EXPIRATION: u32 = 1_000;

/// A subscription, a funded subscriber, a merchant and a token in one
/// environment.
///
/// The ledger clock starts at [`START`] so billing boundaries are absolute and
/// reproducible, and `advance_time` moves it through the testkit context
/// rather than by hand.
pub struct Fixture {
    pub ctx: TestContext,
    pub contract_id: Address,
    pub token_id: Address,
    pub subscriber: Address,
    pub merchant: Address,
    pub outsider: Address,
    pub fee: i128,
    pub interval: u64,
    pub next_billing: u64,
}

impl Fixture {
    pub fn new() -> Self {
        Self::with_terms(FEE, INTERVAL)
    }

    /// A subscription for `fee` every `interval`, funded and approved for four
    /// cycles.
    pub fn with_terms(fee: i128, interval: u64) -> Self {
        let funding = fee.saturating_mul(4).max(fee);
        Self::with_limits(fee, interval, funding, funding, None)
    }

    /// A subscription with explicit funding and allowance, which is how the
    /// insufficient-balance and insufficient-allowance cases are built.
    pub fn with_limits(
        fee: i128,
        interval: u64,
        funding: i128,
        allowance: i128,
        parties: Option<(AddressBytes, AddressBytes)>,
    ) -> Self {
        Self::build(fee, interval, Some((funding, allowance)), parties)
    }

    /// A subscription with generated parties and no token setup.
    ///
    /// `soroban_testkit_generators` addresses are arbitrary 32-byte principals
    /// rather than ledger accounts, so the Stellar asset contract cannot mint to
    /// them. This constructor is for the parts of the flow that never move
    /// tokens: recording, reading and canceling.
    pub fn without_funding(
        fee: i128,
        interval: u64,
        parties: (AddressBytes, AddressBytes),
    ) -> Self {
        Self::build(fee, interval, None, Some(parties))
    }

    fn build(
        fee: i128,
        interval: u64,
        limits: Option<(i128, i128)>,
        parties: Option<(AddressBytes, AddressBytes)>,
    ) -> Self {
        let mut ctx = TestContextBuilder::new().with_users(3).build();
        ctx.set_timestamp(START);

        let (subscriber, merchant) = match parties {
            Some((subscriber, merchant)) => (
                subscriber.to_address(&ctx.env),
                merchant.to_address(&ctx.env),
            ),
            None => (ctx.user(0).clone(), ctx.user(1).clone()),
        };
        let outsider = ctx.user(2).clone();

        let sac = ctx
            .env
            .register_stellar_asset_contract_v2(ctx.admin.clone());
        let token_id = sac.address();
        let contract_id = ctx.env.register(SubscriptionContract, ());

        if let Some((funding, allowance)) = limits {
            StellarAssetClient::new(&ctx.env, &token_id).mint(&subscriber, &funding);
            TokenClient::new(&ctx.env, &token_id).approve(
                &subscriber,
                &contract_id,
                &allowance,
                &APPROVAL_EXPIRATION,
            );
        }

        let next_billing = START + interval;
        Self {
            ctx,
            contract_id,
            token_id,
            subscriber,
            merchant,
            outsider,
            fee,
            interval,
            next_billing,
        }
    }

    pub fn client(&self) -> SubscriptionContractClient<'_> {
        SubscriptionContractClient::new(&self.ctx.env, &self.contract_id)
    }

    pub fn token(&self) -> TokenClient<'_> {
        TokenClient::new(&self.ctx.env, &self.token_id)
    }

    pub fn subscribe(&self) {
        self.subscribe_at(self.next_billing);
    }

    pub fn subscribe_at(&self, next_billing: u64) {
        settle(self.client().try_subscribe(
            &self.subscriber,
            &self.merchant,
            &self.token_id,
            &self.fee,
            &self.interval,
            &next_billing,
        ))
        .expect("subscription should be created");
    }

    pub fn try_subscribe(
        &self,
        subscriber: &Address,
        fee: i128,
        interval: u64,
        next_billing: u64,
    ) -> Result<(), SubscriptionError> {
        settle(self.client().try_subscribe(
            subscriber,
            &self.merchant,
            &self.token_id,
            &fee,
            &interval,
            &next_billing,
        ))
    }

    pub fn bill(&self) -> u32 {
        self.try_bill().expect("a due billing cycle should succeed")
    }

    pub fn try_bill(&self) -> Result<u32, SubscriptionError> {
        settle(self.client().try_bill(&self.merchant, &self.subscriber))
    }

    pub fn try_bill_as(&self, caller: &Address) -> Result<u32, SubscriptionError> {
        settle(self.client().try_bill(caller, &self.subscriber))
    }

    pub fn cancel(&self) {
        settle(self.client().try_cancel(&self.subscriber, &self.subscriber))
            .expect("subscriber should be able to cancel");
    }

    pub fn try_cancel(&self) -> Result<(), SubscriptionError> {
        settle(self.client().try_cancel(&self.subscriber, &self.subscriber))
    }

    pub fn try_cancel_as(&self, caller: &Address) -> Result<(), SubscriptionError> {
        settle(self.client().try_cancel(caller, &self.subscriber))
    }

    pub fn subscription(&self) -> Option<Subscription> {
        self.client().subscription(&self.subscriber)
    }

    pub fn balance(&self, who: &Address) -> i128 {
        self.token().balance(who)
    }

    pub fn merchant_balance(&self) -> i128 {
        self.balance(&self.merchant)
    }

    pub fn allowance_left(&self) -> i128 {
        self.token().allowance(&self.subscriber, &self.contract_id)
    }

    pub fn now(&self) -> u64 {
        self.ctx.timestamp()
    }

    pub fn set_time(&mut self, timestamp: u64) {
        self.ctx.set_timestamp(timestamp);
    }

    pub fn advance_time(&mut self, seconds: u64) -> u64 {
        self.ctx.advance_time(seconds)
    }

    pub fn env(&self) -> &Env {
        &self.ctx.env
    }
}

/// Flattens the nested result the generated `try_*` client returns.
///
/// The outer layer reports whether the return value decoded, and the inner one
/// carries either the contract's typed error or a host failure.
fn settle<T, E>(
    result: Result<Result<T, soroban_sdk::ConversionError>, Result<E, soroban_sdk::InvokeError>>,
) -> Result<T, E> {
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) => panic!("the contract returned a value that could not be decoded"),
        Err(Ok(error)) => Err(error),
        Err(Err(invoke_error)) => {
            panic!("the host refused the invocation: {invoke_error:?}")
        }
    }
}
