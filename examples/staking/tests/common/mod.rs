//! Fixture shared by the staking example's test files.
//!
//! Every test starts from a funded pool, two stakers and a reward balance the
//! pool owns, so a failure is about the contract rather than about setup.

#![allow(dead_code)]

use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::token::{Client as TokenClient, StellarAssetClient};
use soroban_sdk::{Address, Env};
use soroban_testkit_example_staking::{StakingPool, StakingPoolClient};

/// Stroops in one token unit.
pub const ONE: i128 = 10_000_000;

/// Reward stroops per second the fixture's pool pays by default.
pub const RATE: i128 = 1_000;

pub struct Fixture {
    pub env: Env,
    pub id: Address,
    pub stake_token: Address,
    pub reward_token: Address,
    pub admin: Address,
    pub alice: Address,
    pub bob: Address,
}

impl Fixture {
    pub fn new() -> Self {
        Self::with_rate(RATE)
    }

    pub fn with_rate(rate: i128) -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|ledger| {
            ledger.timestamp = 1_700_000_000;
            ledger.sequence_number = 1_000;
        });

        let id = env.register(StakingPool, ());
        let admin = Address::generate(&env);
        let stake_token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let reward_token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let alice = Address::generate(&env);
        let bob = Address::generate(&env);

        StellarAssetClient::new(&env, &stake_token).mint(&alice, &(1_000_000 * ONE));
        StellarAssetClient::new(&env, &stake_token).mint(&bob, &(1_000_000 * ONE));
        StellarAssetClient::new(&env, &reward_token).mint(&id, &(1_000_000_000 * ONE));

        let fixture = Self {
            env,
            id,
            stake_token,
            reward_token,
            admin,
            alice,
            bob,
        };
        fixture.client().initialize(
            &fixture.admin,
            &fixture.stake_token,
            &fixture.reward_token,
            &rate,
        );
        fixture
    }

    pub fn client(&self) -> StakingPoolClient<'_> {
        StakingPoolClient::new(&self.env, &self.id)
    }

    pub fn stake_balance(&self, owner: &Address) -> i128 {
        TokenClient::new(&self.env, &self.stake_token).balance(owner)
    }

    pub fn reward_balance(&self, owner: &Address) -> i128 {
        TokenClient::new(&self.env, &self.reward_token).balance(owner)
    }

    /// Move the ledger clock forward, in seconds.
    pub fn advance_time(&self, seconds: u64) {
        self.env.ledger().with_mut(|ledger| {
            ledger.timestamp += seconds;
            ledger.sequence_number += (1 + seconds / 5) as u32;
        });
    }

    /// Stake on behalf of `owner` and return the stake token balance after.
    pub fn stake(&self, owner: &Address, amount: i128) -> i128 {
        self.client().stake(owner, &amount);
        self.stake_balance(owner)
    }
}
