pub mod builder;

use soroban_sdk::{Address, BytesN, Env};

pub struct TestContext {
    pub env: Env,
    pub admin: Address,
    pub users: Vec<Address>,
}

impl TestContext {
    pub fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
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
