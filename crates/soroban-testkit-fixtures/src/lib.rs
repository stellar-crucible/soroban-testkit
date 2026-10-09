pub mod builder;

use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, Env};

pub struct TestContext {
    pub env: Env,
    pub admin: Address,
    pub users: Vec<Address>,
}

impl Default for TestContext {
    fn default() -> Self {
        Self::new()
    }
}

impl TestContext {
    pub fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        Self::with_env(env)
    }

    /// Builds a context around an environment the caller already configured,
    /// so every address in the context belongs to that environment.
    pub fn with_env(env: Env) -> Self {
        let admin = Address::generate(&env);
        Self {
            env,
            admin,
            users: Vec::new(),
        }
    }

    pub fn add_user(&mut self) -> &Address {
        let user = Address::generate(&self.env);
        self.users.push(user);
        self.users.last().unwrap()
    }

    /// The timestamp the ledger currently reports.
    pub fn timestamp(&self) -> u64 {
        self.env.ledger().timestamp()
    }

    /// The sequence number the ledger currently reports.
    pub fn sequence(&self) -> u32 {
        self.env.ledger().sequence()
    }

    /// Move the ledger clock to an absolute unix timestamp.
    ///
    /// Prefer [`TestContext::advance_time`] when the interesting property is
    /// the elapsed span; an absolute timestamp pins the test to a moment in
    /// history and breaks the day the fixture is read next year.
    pub fn set_timestamp(&mut self, timestamp: u64) {
        self.env.ledger().set_timestamp(timestamp);
    }

    /// Move the ledger clock forward by `seconds`, returning the new timestamp.
    ///
    /// The sequence number does not move with it: a contract that reads
    /// `env.ledger().sequence()` sees no ledger close at all. Call
    /// [`TestContext::advance_ledger`] as well when the test depends on both,
    /// which is what a real chain does — every close advances both.
    pub fn advance_time(&mut self, seconds: u64) -> u64 {
        let next = self.timestamp().saturating_add(seconds);
        self.env.ledger().set_timestamp(next);
        next
    }

    /// Jump `count` ledgers forward, returning the new sequence number.
    ///
    /// The timestamp stays where it was, so the clock does not silently advance
    /// by the network's five-second cadence when a test only wanted entries to
    /// expire.
    pub fn advance_ledger(&mut self, count: u32) -> u32 {
        let next = self.sequence().saturating_add(count);
        self.env.ledger().set_sequence_number(next);
        next
    }
}

#[cfg(test)]
mod tests {
    use super::TestContext;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{contract, contractimpl, Address, Env};

    #[test]
    fn new_context_starts_without_users() {
        let ctx = TestContext::new();
        assert!(ctx.users.is_empty());
    }

    #[test]
    fn add_user_appends_and_returns_the_new_address() {
        let mut ctx = TestContext::new();

        let first = ctx.add_user().clone();
        let second = ctx.add_user().clone();

        assert_eq!(ctx.users.len(), 2);
        assert_eq!(ctx.users[0], first);
        assert_eq!(ctx.users[1], second);
        assert_ne!(first, second);
    }

    #[test]
    fn admin_is_distinct_from_generated_users() {
        let mut ctx = TestContext::new();
        let user = ctx.add_user().clone();

        assert_ne!(ctx.admin, user);
    }

    #[test]
    fn addresses_generated_in_the_same_env_are_unique() {
        let ctx = TestContext::new();
        let a = Address::generate(&ctx.env);
        let b = Address::generate(&ctx.env);

        assert_ne!(a, b);
    }

    #[test]
    fn default_and_new_build_the_same_shape_of_context() {
        let mut from_default = TestContext::default();
        let mut from_new = TestContext::new();

        assert!(from_default.users.is_empty());
        assert!(from_new.users.is_empty());

        from_default.add_user();
        from_new.add_user();

        assert_eq!(from_default.users.len(), from_new.users.len());
    }

    #[contract]
    pub struct Clock;

    #[contractimpl]
    impl Clock {
        pub fn now(env: Env) -> u64 {
            env.ledger().timestamp()
        }

        pub fn height(env: Env) -> u32 {
            env.ledger().sequence()
        }
    }

    fn clock(env: &Env) -> ClockClient<'_> {
        let id = env.register(Clock, ());
        ClockClient::new(env, &id)
    }

    #[test]
    fn getters_read_the_ledger_the_env_started_with() {
        let ctx = TestContext::new();

        assert_eq!(ctx.timestamp(), ctx.env.ledger().timestamp());
        assert_eq!(ctx.sequence(), ctx.env.ledger().sequence());
    }

    #[test]
    fn a_fresh_env_opens_at_the_zero_ledger_so_set_before_advance() {
        let mut ctx = TestContext::new();
        assert_eq!(ctx.timestamp(), 0);
        assert_eq!(ctx.sequence(), 0);

        // `advance_time` from a zero clock lands on the offset, not on wall
        // time, so a test that cares about an absolute moment sets it first.
        ctx.set_timestamp(1_700_000_000);
        assert_eq!(ctx.advance_time(1), 1_700_000_001);
    }

    #[test]
    fn advance_time_adds_to_the_clock_rather_than_replacing_it() {
        let mut ctx = TestContext::new();
        ctx.set_timestamp(1_700_000_000);

        let after_a_day = ctx.advance_time(86_400);
        let after_two_days = ctx.advance_time(86_400);

        assert_eq!(after_a_day, 1_700_086_400);
        assert_eq!(after_two_days, 1_700_172_800);
        assert_eq!(ctx.timestamp(), after_two_days);
    }

    #[test]
    fn a_contract_sees_the_advanced_clock() {
        let mut ctx = TestContext::new();
        let env = ctx.env.clone();
        let client = clock(&env);
        let before = client.now();

        ctx.advance_time(3_600);

        assert_eq!(client.now(), before + 3_600);
    }

    #[test]
    fn advance_time_leaves_the_sequence_where_it_was() {
        let mut ctx = TestContext::new();
        let height = ctx.sequence();

        ctx.advance_time(86_400 * 30);

        assert_eq!(ctx.sequence(), height);
    }

    #[test]
    fn advance_ledger_moves_the_height_and_not_the_clock() {
        let mut ctx = TestContext::new();
        ctx.set_timestamp(1_700_000_000);
        let height = ctx.sequence();

        let new_height = ctx.advance_ledger(10);

        assert_eq!(new_height, height + 10);
        assert_eq!(ctx.sequence(), new_height);
        assert_eq!(ctx.timestamp(), 1_700_000_000);
    }

    #[test]
    fn advancing_the_clock_and_the_ledger_together_moves_both() {
        let mut ctx = TestContext::new();
        ctx.set_timestamp(1_700_000_000);
        let height = ctx.sequence();

        ctx.advance_time(5 * 10);
        ctx.advance_ledger(10);

        assert_eq!(ctx.timestamp(), 1_700_000_050);
        assert_eq!(ctx.sequence(), height + 10);
    }

    #[test]
    fn a_contract_sees_the_advanced_height() {
        let mut ctx = TestContext::new();
        let env = ctx.env.clone();
        let client = clock(&env);
        let before = client.height();

        ctx.advance_ledger(5);

        assert_eq!(client.height(), before + 5);
    }

    #[test]
    fn set_timestamp_replaces_the_clock() {
        let mut ctx = TestContext::new();

        ctx.set_timestamp(1_000);
        ctx.set_timestamp(2_000);

        assert_eq!(ctx.timestamp(), 2_000);
    }

    #[test]
    fn advancing_past_the_end_of_the_u64_clock_saturates() {
        let mut ctx = TestContext::new();
        ctx.set_timestamp(u64::MAX);

        assert_eq!(ctx.advance_time(1), u64::MAX);
    }

    #[test]
    fn advancing_past_the_top_of_u32_saturates_the_height() {
        let mut ctx = TestContext::new();
        let near_top = ctx.advance_ledger(u32::MAX - ctx.sequence());

        assert_eq!(near_top, u32::MAX);
        assert_eq!(ctx.advance_ledger(10), u32::MAX);
    }
}
