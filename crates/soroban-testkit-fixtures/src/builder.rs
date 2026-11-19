use soroban_sdk::Env;

use crate::profiles::NetworkProfile;

pub struct TestContextBuilder {
    num_users: usize,
    mock_auths: bool,
    network_profile: NetworkProfile,
}

impl Default for TestContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TestContextBuilder {
    pub fn new() -> Self {
        Self {
            num_users: 0,
            mock_auths: true,
            network_profile: NetworkProfile::Local,
        }
    }

    pub fn with_users(mut self, count: usize) -> Self {
        self.num_users = count;
        self
    }

    pub fn without_mock_auths(mut self) -> Self {
        self.mock_auths = false;
        self
    }

    /// Apply a network environment profile to the built context.
    ///
    /// [`NetworkProfile::Local`] is the default: unmetered, so a test only
    /// pays for the work it explicitly measures. `Testnet` and `Mainnet`
    /// install that network's budget ceilings and ledger parameters, so a call
    /// that would exceed them fails here instead of on a live network.
    pub fn with_network_profile(mut self, profile: NetworkProfile) -> Self {
        self.network_profile = profile;
        self
    }

    pub fn build(self) -> super::TestContext {
        let env = Env::default();
        if self.mock_auths {
            env.mock_all_auths();
        }

        let mut ctx = super::TestContext::with_env(env);
        ctx.mock_auths = self.mock_auths;
        for _ in 0..self.num_users {
            ctx.add_user();
        }
        self.network_profile.apply(&ctx.env);
        ctx.network_profile = self.network_profile;
        ctx
    }
}

#[cfg(test)]
mod tests {
    use super::TestContextBuilder;
    use crate::profiles::{NetworkProfile, TESTNET_CPU_INSTRUCTIONS};
    use soroban_sdk::testutils::Events as _;

    #[test]
    fn new_builder_produces_an_empty_context() {
        let ctx = TestContextBuilder::new().build();
        assert!(ctx.users.is_empty());
    }

    #[test]
    fn default_builder_and_new_builder_agree_on_their_defaults() {
        let from_default = TestContextBuilder::default().build();
        let from_new = TestContextBuilder::new().build();

        assert!(from_default.users.is_empty());
        assert!(from_new.users.is_empty());
    }

    #[test]
    fn with_users_pre_generates_distinct_addresses() {
        let ctx = TestContextBuilder::new().with_users(4).build();

        assert_eq!(ctx.users.len(), 4);
        for (index, user) in ctx.users.iter().enumerate() {
            assert!(
                ctx.users[..index].iter().all(|other| other != user),
                "user {} duplicates an earlier address",
                index
            );
        }
    }

    #[test]
    fn users_are_generated_before_the_context_is_handed_over() {
        let ctx = TestContextBuilder::new().with_users(25).build();
        assert_eq!(ctx.users.len(), 25);
    }

    #[test]
    fn without_mock_auths_still_builds_a_usable_env() {
        let ctx = TestContextBuilder::new()
            .with_users(1)
            .without_mock_auths()
            .build();

        assert_eq!(ctx.users.len(), 1);
        assert!(
            ctx.env.events().all().events().is_empty(),
            "a fresh environment has no events to inspect"
        );
    }

    #[test]
    fn with_network_profile_installs_the_profile_budget() {
        let ctx = TestContextBuilder::new()
            .with_network_profile(NetworkProfile::Testnet)
            .build();

        assert_eq!(ctx.network_profile, NetworkProfile::Testnet);
        assert_eq!(
            ctx.env
                .host()
                .budget_cloned()
                .get_cpu_insns_remaining()
                .unwrap(),
            TESTNET_CPU_INSTRUCTIONS as u64
        );
    }

    #[test]
    fn builder_chaining_preserves_every_option() {
        let ctx = TestContextBuilder::new()
            .with_users(3)
            .without_mock_auths()
            .build();
        assert_eq!(ctx.users.len(), 3);
        assert_ne!(ctx.admin, ctx.users[0]);
    }
}
