//! Deployment shared by the four test files: two asset tokens, a share token
//! owned by the pool, the pool, and a router.
#![allow(dead_code)]

use soroban_sdk::{Address, Env, String};
use soroban_testkit_example_amm::{
    AmmToken, AmmTokenClient, ConstantProductPool, ConstantProductPoolClient, Router, RouterClient,
};

/// One whole token at seven decimals.
pub const ONE: i128 = 10_000_000;

pub struct Amm {
    pub env: Env,
    pub token_a: Address,
    pub token_b: Address,
    pub share: Address,
    pub pool: Address,
    pub router: Address,
}

impl Amm {
    /// Deploy every contract into `env`, with `admin` minting the two assets.
    pub fn deploy(env: &Env, admin: &Address, fee_bps: u32) -> Self {
        let token = |name: &str, symbol: &str, owner: &Address| {
            env.register(
                AmmToken,
                (
                    owner.clone(),
                    7u32,
                    String::from_str(env, name),
                    String::from_str(env, symbol),
                ),
            )
        };
        let token_a = token("Token A", "TKA", admin);
        let token_b = token("Token B", "TKB", admin);
        let pool = env.register(ConstantProductPool, ());
        // The pool is the share token's admin, so only the pool mints shares.
        let share = token("Pool Share", "LP", &pool);
        let router = env.register(Router, ());

        ConstantProductPoolClient::new(env, &pool).initialize(&token_a, &token_b, &share, &fee_bps);

        Self {
            env: env.clone(),
            token_a,
            token_b,
            share,
            pool,
            router,
        }
    }

    pub fn pool(&self) -> ConstantProductPoolClient<'_> {
        ConstantProductPoolClient::new(&self.env, &self.pool)
    }

    pub fn router(&self) -> RouterClient<'_> {
        RouterClient::new(&self.env, &self.router)
    }

    pub fn token_a(&self) -> AmmTokenClient<'_> {
        AmmTokenClient::new(&self.env, &self.token_a)
    }

    pub fn token_b(&self) -> AmmTokenClient<'_> {
        AmmTokenClient::new(&self.env, &self.token_b)
    }

    pub fn share(&self) -> AmmTokenClient<'_> {
        AmmTokenClient::new(&self.env, &self.share)
    }

    /// Mint `amount` of both assets to `user`. Needs mocked auths.
    pub fn fund(&self, user: &Address, amount: i128) {
        self.token_a().mint(user, &amount);
        self.token_b().mint(user, &amount);
    }

    /// `reserve_a * reserve_b`.
    pub fn k(&self) -> i128 {
        let (a, b) = self.pool().get_reserves();
        a * b
    }
}
