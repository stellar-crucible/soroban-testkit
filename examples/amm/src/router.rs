use soroban_sdk::{contract, contractimpl, Address, Env};

use crate::pool::{AmmError, ConstantProductPoolClient};

/// The user-facing entry point. It holds nothing; it forwards to a pool.
#[contract]
pub struct Router;

#[contractimpl]
impl Router {
    /// Swap through `pool` on behalf of `user`.
    ///
    /// `user` authorizes this call, and the authorization covers the pool's
    /// `swap` and the token `transfer` beneath it: one signature, a
    /// three-level tree.
    pub fn swap_exact_in(
        env: Env,
        pool: Address,
        user: Address,
        buy_a: bool,
        amount_in: i128,
        min_out: i128,
    ) -> Result<i128, AmmError> {
        user.require_auth();
        // A failing pool call fails this frame with the same error, so a
        // caller sees the pool's (or a token's) code unchanged.
        let pool = ConstantProductPoolClient::new(&env, &pool);
        Ok(pool.swap(&user, &buy_a, &amount_in, &min_out))
    }
}
