use soroban_sdk::{Address, Env};

pub struct TestContextBuilder {
    num_users: usize,
    mock_auths: bool,
}

impl TestContextBuilder {
    pub fn new() -> Self {
        Self {
            num_users: 0,
            mock_auths: true,
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

    pub fn build(self) -> super::TestContext {
        let mut ctx = super::TestContext::new();
        if !self.mock_auths {
            // Re-create env without mock_all_auths
            ctx.env = Env::default();
        }
        for _ in 0..self.num_users {
            ctx.add_user();
        }
        ctx
    }
}
