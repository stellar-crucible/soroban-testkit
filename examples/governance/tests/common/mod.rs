#![allow(dead_code)]

use governance::{Action, Governance, GovernanceClient, ProposalStatus};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Symbol};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::{NetworkProfile, TestContext};

/// 40% of the supply must vote for a proposal to be decided.
pub const QUORUM_BPS: u32 = 4_000;
/// One day of voting.
pub const VOTING_PERIOD: u64 = 86_400;
/// Two days between passing and execution.
pub const TIMELOCK: u64 = 172_800;
/// Where the ledger clock starts.
pub const GENESIS: u64 = 1_700_000_000;
/// Voting power of the three members. Total supply 1 000, quorum 400.
pub const WEIGHTS: [i128; 3] = [450, 350, 200];
pub const SUPPLY: i128 = 1_000;

pub struct Fixture {
    pub ctx: TestContext,
    pub contract_id: Address,
    pub admin: Address,
    pub voters: [Address; 3],
    pub outsider: Address,
}

impl Fixture {
    /// Registered, not yet initialized.
    pub fn new() -> Self {
        Self::with_profile(NetworkProfile::Local)
    }

    pub fn with_profile(profile: NetworkProfile) -> Self {
        let mut ctx = TestContextBuilder::new()
            .with_network_profile(profile)
            .with_users(3)
            .build();
        ctx.set_timestamp(GENESIS);
        let admin = ctx.admin.clone();
        let voters = [
            ctx.user(0).clone(),
            ctx.user(1).clone(),
            ctx.user(2).clone(),
        ];
        let outsider = Address::generate(&ctx.env);
        let contract_id = ctx.env.register(Governance, ());
        Self {
            ctx,
            contract_id,
            admin,
            voters,
            outsider,
        }
    }

    /// Initialized with the default configuration, no voting power minted.
    pub fn initialized() -> Self {
        let fixture = Self::new();
        fixture.initialize();
        fixture
    }

    /// Initialized, with [`WEIGHTS`] minted to the three voters.
    pub fn funded() -> Self {
        let fixture = Self::initialized();
        fixture.mint_weights();
        fixture
    }

    pub fn client(&self) -> GovernanceClient<'_> {
        GovernanceClient::new(&self.ctx.env, &self.contract_id)
    }

    pub fn initialize(&self) {
        self.client()
            .initialize(&self.admin, &QUORUM_BPS, &VOTING_PERIOD, &TIMELOCK);
    }

    pub fn mint_weights(&self) {
        for (voter, weight) in self.voters.iter().zip(WEIGHTS) {
            self.client().mint(voter, &weight);
        }
    }

    pub fn title(&self, text: &str) -> Symbol {
        Symbol::new(&self.ctx.env, text)
    }

    /// Voter 0 proposes `action`.
    pub fn propose(&self, action: &Action) -> u64 {
        self.client()
            .propose(&self.voters[0], &self.title("proposal"), action)
    }

    pub fn vote(&self, voter: usize, id: u64, support: bool) -> i128 {
        self.client().vote(&self.voters[voter], &id, &support)
    }

    pub fn now(&self) -> u64 {
        self.ctx.timestamp()
    }

    pub fn advance_time(&mut self, seconds: u64) -> u64 {
        self.ctx.advance_time(seconds)
    }

    pub fn end_voting(&mut self) {
        self.advance_time(VOTING_PERIOD);
    }

    /// Propose `action`, vote it through with voters 0 and 1, close voting and
    /// queue it. Returns the queued proposal id.
    pub fn queued(&mut self, action: &Action) -> u64 {
        let id = self.propose(action);
        self.vote(0, id, true);
        self.vote(1, id, true);
        self.end_voting();
        assert_eq!(self.client().queue(&id), ProposalStatus::Queued);
        id
    }

    pub fn status(&self, id: u64) -> ProposalStatus {
        self.client().proposal(&id).status
    }
}
