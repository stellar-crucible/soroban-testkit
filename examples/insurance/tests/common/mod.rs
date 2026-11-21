#![allow(dead_code)]

use soroban_sdk::token::{StellarAssetClient, TokenClient};
use soroban_sdk::{Address, Vec};
use soroban_testkit_example_insurance::{
    premium_for, Claim, InsuranceConfig, InsurancePool, InsurancePoolClient, Policy,
};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

/// One whole unit of the 7-decimal capital token.
pub const ONE: i128 = 10_000_000;
/// Seconds in a 365-day year.
pub const YEAR: u64 = 31_536_000;
/// 5% annual premium rate.
pub const RATE_BPS: u32 = 500;
/// Two of the three assessors must vote.
pub const QUORUM: u32 = 2;
/// Largest coverage one policy may buy.
pub const MAX_COVERAGE: i128 = 1_000_000 * ONE;
/// Capital token units each actor starts with.
pub const FUNDING: i128 = 10_000_000 * ONE;

/// The deployed pool and the actors a test drives it with.
///
/// `TestContextBuilder` mints eight users: two capital providers, two
/// policyholders, three assessors and an outsider who is on no panel.
pub struct Insurance {
    pub ctx: TestContext,
    pub token: Address,
    pub pool_id: Address,
    pub provider: Address,
    pub provider2: Address,
    pub holder: Address,
    pub holder2: Address,
    pub assessors: [Address; 3],
    pub outsider: Address,
}

impl Insurance {
    /// The pool with the default parameters, funded and ready to underwrite.
    pub fn new() -> Self {
        Self::with_config(RATE_BPS, QUORUM, MAX_COVERAGE)
    }

    /// The pool with explicit premium rate, quorum and coverage cap.
    pub fn with_config(rate_bps: u32, quorum: u32, max_coverage: i128) -> Self {
        let insurance = Self::deployed();
        insurance
            .pool()
            .initialize(&insurance.config(rate_bps, quorum, max_coverage));
        for user in [
            &insurance.provider,
            &insurance.provider2,
            &insurance.holder,
            &insurance.holder2,
        ] {
            insurance.fund(user, FUNDING);
        }
        insurance
    }

    /// The token and pool registered but the pool left uninitialized.
    pub fn deployed() -> Self {
        let ctx = TestContextBuilder::new().with_users(8).build();
        let env = &ctx.env;
        let token = env
            .register_stellar_asset_contract_v2(ctx.admin.clone())
            .address();
        let pool_id = env.register(InsurancePool, ());

        let provider = ctx.user(0).clone();
        let provider2 = ctx.user(1).clone();
        let holder = ctx.user(2).clone();
        let holder2 = ctx.user(3).clone();
        let assessors = [
            ctx.user(4).clone(),
            ctx.user(5).clone(),
            ctx.user(6).clone(),
        ];
        let outsider = ctx.user(7).clone();

        Self {
            ctx,
            token,
            pool_id,
            provider,
            provider2,
            holder,
            holder2,
            assessors,
            outsider,
        }
    }

    pub fn pool(&self) -> InsurancePoolClient<'_> {
        InsurancePoolClient::new(&self.ctx.env, &self.pool_id)
    }

    pub fn token(&self) -> TokenClient<'_> {
        TokenClient::new(&self.ctx.env, &self.token)
    }

    /// A configuration naming the pool's three assessors.
    pub fn config(&self, rate_bps: u32, quorum: u32, max_coverage: i128) -> InsuranceConfig {
        let mut assessors = Vec::new(&self.ctx.env);
        for assessor in &self.assessors {
            assessors.push_back(assessor.clone());
        }
        InsuranceConfig {
            admin: self.ctx.admin.clone(),
            capital_token: self.token.clone(),
            assessors,
            quorum,
            premium_rate_bps: rate_bps,
            max_coverage,
        }
    }

    pub fn fund(&self, who: &Address, amount: i128) {
        StellarAssetClient::new(&self.ctx.env, &self.token).mint(who, &amount);
    }

    /// Underwrite the pool, returning the shares minted.
    pub fn deposit(&self, provider: &Address, amount: i128) -> i128 {
        self.pool().deposit(provider, &amount)
    }

    pub fn buy(&self, holder: &Address, coverage: i128, duration: u64) -> Policy {
        self.pool().buy_policy(holder, &coverage, &duration)
    }

    pub fn file(&self, holder: &Address, policy_id: u64, loss: i128) -> Claim {
        self.pool().file_claim(holder, &policy_id, &loss)
    }

    pub fn cast(&self, assessor: &Address, claim_id: u64, approve: bool) -> Claim {
        self.pool().vote(assessor, &claim_id, &approve)
    }

    pub fn settle(&self, claim_id: u64) -> Claim {
        self.pool().settle(&claim_id)
    }

    /// Cast the two approving votes a default-quorum pool needs.
    pub fn approve(&self, claim_id: u64) -> Claim {
        self.cast(&self.assessors[0], claim_id, true);
        self.cast(&self.assessors[1], claim_id, true)
    }

    /// Buy coverage and immediately file a loss against it.
    pub fn open_claim(&self, holder: &Address, coverage: i128, loss: i128) -> (Policy, Claim) {
        let policy = self.buy(holder, coverage, YEAR);
        let claim = self.file(holder, policy.id, loss);
        (policy, claim)
    }

    /// Buy coverage and file a loss, then approve and settle it.
    pub fn paid_claim(&self, holder: &Address, coverage: i128, loss: i128) -> Claim {
        let (_, claim) = self.open_claim(holder, coverage, loss);
        self.approve(claim.id);
        self.settle(claim.id)
    }

    /// The premium the default rate charges for `coverage` over `duration`.
    pub fn premium(&self, coverage: i128, duration: u64) -> i128 {
        premium_for(coverage, RATE_BPS, duration).unwrap()
    }

    pub fn now(&self) -> u64 {
        self.ctx.env.ledger().timestamp()
    }

    pub fn advance_time(&mut self, seconds: u64) -> u64 {
        self.ctx.advance_time(seconds)
    }
}
