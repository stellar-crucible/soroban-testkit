pub mod builder;

use soroban_sdk::testutils::Address as _;
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
}

#[cfg(test)]
mod tests {
    use super::TestContext;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::Address;

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
}
