use soroban_sdk::Env;

pub struct AuthMatcher<'a> {
    env: &'a Env,
}

impl<'a> AuthMatcher<'a> {
    pub fn new(env: &'a Env) -> Self {
        Self { env }
    }

    pub fn assert_no_auth_required(&self) {
        let auths = self.env.auths();
        assert!(
            auths.is_empty(),
            "Expected no authorizations required, but found {} auth entries",
            auths.len()
        );
    }

    pub fn assert_auth_count(&self, expected: usize) {
        let auths = self.env.auths();
        assert_eq!(
            auths.len(),
            expected,
            "Expected {} authorizations, found {}",
            expected,
            auths.len()
        );
    }
}
