//! Hierarchical cross-contract authorization call-tree validator and mock builder.
//!
//! Provides [`assert_auth_tree!`], [`AuthTreeMockBuilder`], and tree inspection
//! utilities for verifying nested multi-contract authorization trees down to
//! arbitrary depth with side-by-side visual diffs.

use soroban_sdk::testutils::{AuthorizedFunction, AuthorizedInvocation};
use soroban_sdk::{Address, Env, IntoVal, Symbol, Val, Vec as SorobanVec};

/// A builder for constructing mock [`AuthorizedInvocation`] call trees.
///
/// Use it to assemble expected authorization structures for differential testing,
/// mocked contract calls, or complex multi-tier delegation scenarios.
///
/// ```rust,ignore
/// let invocation = AuthTreeMockBuilder::new(router_id.clone(), "swap")
///     .arg(&user, &env)
///     .arg(&1000i128, &env)
///     .sub_invoke(
///         AuthTreeMockBuilder::new(token_id.clone(), "transfer")
///             .arg(&user, &env)
///             .arg(&pool_id, &env)
///             .arg(&1000i128, &env)
///             .build(&env),
///     )
///     .build(&env);
/// ```
#[derive(Clone, Debug)]
pub struct AuthTreeMockBuilder {
    contract: Address,
    function: String,
    args: std::vec::Vec<Val>,
    sub_invocations: std::vec::Vec<AuthorizedInvocation>,
}

impl AuthTreeMockBuilder {
    /// Creates a new node builder targeting `contract` and calling `function`.
    pub fn new(contract: Address, function: impl Into<String>) -> Self {
        Self {
            contract,
            function: function.into(),
            args: std::vec::Vec::new(),
            sub_invocations: std::vec::Vec::new(),
        }
    }

    /// Appends an argument to the invocation call.
    pub fn arg(mut self, arg: impl IntoVal<Env, Val>, env: &Env) -> Self {
        self.args.push(arg.into_val(env));
        self
    }

    /// Adds a child sub-invocation to this node's authorization tree.
    pub fn sub_invoke(mut self, sub: AuthorizedInvocation) -> Self {
        self.sub_invocations.push(sub);
        self
    }

    /// Adds multiple child sub-invocations to this node.
    pub fn sub_invokes(mut self, subs: impl IntoIterator<Item = AuthorizedInvocation>) -> Self {
        self.sub_invocations.extend(subs);
        self
    }

    /// Compiles this builder into a Soroban [`AuthorizedInvocation`].
    pub fn build(self, env: &Env) -> AuthorizedInvocation {
        let function_symbol = Symbol::new(env, &self.function);
        let args_vec = SorobanVec::from_slice(env, &self.args);
        AuthorizedInvocation {
            function: AuthorizedFunction::Contract((self.contract, function_symbol, args_vec)),
            sub_invocations: self.sub_invocations,
        }
    }
}

/// Recursively computes the maximum depth of an [`AuthorizedInvocation`] tree.
pub fn tree_depth(invocation: &AuthorizedInvocation) -> usize {
    if invocation.sub_invocations.is_empty() {
        1
    } else {
        1 + invocation
            .sub_invocations
            .iter()
            .map(tree_depth)
            .max()
            .unwrap_or(0)
    }
}

/// Asserts that the invocation that just ran recorded an authorization tree matching
/// the declared hierarchical call tree.
///
/// Supports matching invoker address, function name, arguments, and nested `sub_invokes`
/// down to arbitrary depth. On assertion failure, renders a visual hierarchical diff
/// highlighting missing or mismatched branches with `[MISSING]`.
///
/// ```rust,ignore
/// use soroban_testkit_assert::assert_auth_tree;
///
/// assert_auth_tree!(env, {
///     invoker: user,
///     function: "deposit_and_stake",
///     sub_invokes: [
///         {
///             invoker: router,
///             function: "transfer",
///             args: [user, pool, 1000],
///             sub_invokes: [
///                 { invoker: pool, function: "mint_receipt", args: [user, 1000] }
///             ]
///         }
///     ]
/// });
/// ```
#[macro_export]
macro_rules! assert_auth_tree {
    ($env:expr, { $($body:tt)* }) => {{
        $crate::assert_auth_sub_tree_matches!($env, { $($body)* })
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{contract, contractimpl, Address, Env};

    #[contract]
    pub struct VaultContract;

    #[contractimpl]
    impl VaultContract {
        pub fn deposit(env: Env, user: Address, token: Address, amount: i128) {
            user.require_auth();
            TokenContractClient::new(&env, &token).mint(&user, &amount);
        }
    }

    #[contract]
    pub struct TokenContract;

    #[contractimpl]
    impl TokenContract {
        pub fn mint(user: Address, _amount: i128) {
            user.require_auth();
        }
    }

    #[contract]
    pub struct PoolContract;

    #[contractimpl]
    impl PoolContract {
        pub fn route(env: Env, user: Address, vault: Address, token: Address, amount: i128) {
            user.require_auth();
            VaultContractClient::new(&env, &vault).deposit(&user, &token, &amount);
        }
    }

    #[contract]
    pub struct AggregatorContract;

    #[contractimpl]
    impl AggregatorContract {
        pub fn swap(
            env: Env,
            user: Address,
            pool: Address,
            vault: Address,
            token: Address,
            amount: i128,
        ) {
            user.require_auth();
            PoolContractClient::new(&env, &pool).route(&user, &vault, &token, &amount);
        }
    }

    struct NestedFixture {
        env: Env,
        aggregator: Address,
        pool: Address,
        vault: Address,
        token: Address,
        user: Address,
    }

    fn nested_fixture() -> NestedFixture {
        let env = Env::default();
        env.mock_all_auths();
        let aggregator = env.register(AggregatorContract, ());
        let pool = env.register(PoolContract, ());
        let vault = env.register(VaultContract, ());
        let token = env.register(TokenContract, ());
        let user = Address::generate(&env);
        NestedFixture {
            env,
            aggregator,
            pool,
            vault,
            token,
            user,
        }
    }

    #[test]
    fn test_assert_auth_tree_depth_four_matching() {
        let fx = nested_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &fx.token, &1_000i128);

        // Validates 4-level deep call tree: Aggregator -> Pool -> Vault -> Token
        assert_auth_tree!(fx.env, {
            invoker: fx.user,
            contract: fx.aggregator,
            function: "swap",
            sub_invokes: [
                {
                    contract: fx.pool,
                    function: "route",
                    sub_invokes: [
                        {
                            contract: fx.vault,
                            function: "deposit",
                            args: [_, fx.token.clone(), 1_000i128],
                            sub_invokes: [
                                {
                                    contract: fx.token,
                                    function: "mint",
                                    args: [_, 1_000i128],
                                }
                            ]
                        }
                    ]
                }
            ]
        });
    }

    #[test]
    #[should_panic(expected = "[MISSING]")]
    fn test_assert_auth_tree_missing_leaf_fails() {
        let fx = nested_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &fx.token, &1_000i128);

        // Expects an extra non-existent child invocation under Token
        assert_auth_tree!(fx.env, {
            invoker: fx.user,
            contract: fx.aggregator,
            function: "swap",
            sub_invokes: [
                {
                    contract: fx.pool,
                    function: "route",
                    sub_invokes: [
                        {
                            contract: fx.vault,
                            function: "deposit",
                            sub_invokes: [
                                {
                                    contract: fx.token,
                                    function: "mint",
                                    sub_invokes: [
                                        {
                                            function: "non_existent_child",
                                        }
                                    ]
                                }
                            ]
                        }
                    ]
                }
            ]
        });
    }

    #[test]
    #[should_panic(expected = "[MISSING]")]
    fn test_assert_auth_tree_branch_mismatch_fails() {
        let fx = nested_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &fx.token, &1_000i128);

        // Expects wrong intermediate contract function name
        assert_auth_tree!(fx.env, {
            invoker: fx.user,
            contract: fx.aggregator,
            function: "swap",
            sub_invokes: [
                {
                    contract: fx.pool,
                    function: "wrong_function_name",
                }
            ]
        });
    }

    #[test]
    fn test_auth_tree_mock_builder_and_depth() {
        let env = Env::default();
        let c1 = Address::generate(&env);
        let c2 = Address::generate(&env);
        let c3 = Address::generate(&env);
        let c4 = Address::generate(&env);

        let leaf = AuthTreeMockBuilder::new(c4, "leaf")
            .arg(100u32, &env)
            .build(&env);

        let n3 = AuthTreeMockBuilder::new(c3, "tier3")
            .sub_invoke(leaf)
            .build(&env);

        let n2 = AuthTreeMockBuilder::new(c2, "tier2")
            .sub_invoke(n3)
            .build(&env);

        let root = AuthTreeMockBuilder::new(c1, "root")
            .sub_invoke(n2)
            .build(&env);

        assert_eq!(tree_depth(&root), 4);
    }
}
