#![allow(dead_code)]

use guardian_treasury::{GuardianTreasury, GuardianTreasuryClient};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, Vec};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::{NetworkProfile, TestContext};

pub const THRESHOLD: u32 = 2;
pub const TIMELOCK: u64 = 3_600;
pub const INITIAL_BALANCE: i128 = 10_000;

pub struct Fixture {
    pub ctx: TestContext,
    pub contract_id: Address,
    pub admin: Address,
    pub signers: [Address; 3],
    pub guardian: Address,
    pub outsider: Address,
    pub recipient: Address,
}

impl Fixture {
    pub fn new() -> Self {
        Self::with_profile(NetworkProfile::Local)
    }

    pub fn with_profile(profile: NetworkProfile) -> Self {
        let ctx = TestContextBuilder::new()
            .with_network_profile(profile)
            .with_users(2)
            .build();
        let admin = Address::generate(&ctx.env);
        let signers = [
            Address::generate(&ctx.env),
            Address::generate(&ctx.env),
            Address::generate(&ctx.env),
        ];
        let guardian = Address::generate(&ctx.env);
        let outsider = Address::generate(&ctx.env);
        let recipient = Address::generate(&ctx.env);
        let contract_id = ctx.env.register(GuardianTreasury, ());
        Self {
            ctx,
            contract_id,
            admin,
            signers,
            guardian,
            outsider,
            recipient,
        }
    }

    pub fn initialized() -> Self {
        let fixture = Self::new();
        fixture.initialize();
        fixture
    }

    pub fn funded() -> Self {
        let fixture = Self::initialized();
        fixture.fund(INITIAL_BALANCE);
        fixture
    }

    pub fn client(&self) -> GuardianTreasuryClient<'_> {
        GuardianTreasuryClient::new(&self.ctx.env, &self.contract_id)
    }

    pub fn signers_vec(&self) -> Vec<Address> {
        Vec::from_array(&self.ctx.env, self.signers.clone())
    }

    pub fn initialize(&self) {
        self.client().initialize(
            &self.admin,
            &self.signers_vec(),
            &THRESHOLD,
            &TIMELOCK,
            &self.guardian,
        );
    }

    pub fn fund(&self, amount: i128) {
        self.client().deposit(&self.admin, &amount);
    }

    pub fn propose(&self, amount: i128) -> u64 {
        self.client()
            .propose(&self.signers[0], &self.recipient, &amount)
    }

    pub fn approve_threshold(&self, proposal_id: u64) {
        let client = self.client();
        client.approve(&self.signers[0], &proposal_id);
        client.approve(&self.signers[1], &proposal_id);
    }

    pub fn ready_proposal(&self, amount: i128) -> u64 {
        let proposal_id = self.propose(amount);
        self.approve_threshold(proposal_id);
        proposal_id
    }

    pub fn now(&self) -> u64 {
        self.ctx.env.ledger().get().timestamp
    }

    pub fn advance_time(&self, seconds: u64) {
        self.ctx.env.ledger().set_timestamp(self.now() + seconds);
    }
}
