use crate::events::data_label;
use soroban_sdk::testutils::{AuthorizedFunction, AuthorizedInvocation};
use soroban_sdk::xdr::ScVal;
use soroban_sdk::{Address, Env, IntoVal, TryFromVal, Val};

/// One position in an expected argument pattern.
///
/// A pattern either accepts whatever the contract recorded at that position or
/// names the value it must equal. The macro `assert_auth_matches!` writes the
/// first as `_` and the second as an ordinary expression.
pub enum AuthArg {
    /// Matches any value at this position.
    Any,
    /// Matches exactly this value.
    Exact(Val),
}

impl AuthArg {
    fn matches(&self, env: &Env, actual: &Val) -> bool {
        match self {
            AuthArg::Any => true,
            AuthArg::Exact(expected) => same_value(env, expected, actual),
        }
    }
}

/// An expected argument list, built by `assert_auth_matches!`.
///
/// The argument values are converted to ledger values with the same [`Env`] the
/// recorded authorizations came from, which is what makes the comparison work
/// for addresses and other object-backed values and not only for small numbers.
pub struct AuthArgs {
    args: Vec<AuthArg>,
}

impl Default for AuthArgs {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthArgs {
    pub fn new() -> Self {
        Self { args: Vec::new() }
    }

    /// Accepts any value at the next position.
    pub fn any(mut self) -> Self {
        self.args.push(AuthArg::Any);
        self
    }

    /// Requires the next position to equal `value`.
    pub fn value<T>(mut self, env: &Env, value: T) -> Self
    where
        T: IntoVal<Env, Val>,
    {
        self.args.push(AuthArg::Exact(value.into_val(env)));
        self
    }

    fn matches_vec(&self, env: &Env, actual: &soroban_sdk::Vec<Val>) -> bool {
        self.args.len() == actual.len() as usize
            && self
                .args
                .iter()
                .zip(actual.iter())
                .all(|(pattern, actual_val)| pattern.matches(env, &actual_val))
    }

    fn describe(&self, env: &Env) -> String {
        self.args
            .iter()
            .map(|arg| match arg {
                AuthArg::Any => "_".to_string(),
                AuthArg::Exact(value) => ScVal::try_from_val(env, value)
                    .map(|value| data_label(env, &value))
                    .unwrap_or_else(|_| format!("{value:?}")),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// One entry in a node's expected child invocation list.
///
/// Inside `sub_invocations: [..]`, `_` matches any single child invocation,
/// `..` matches zero or more arbitrary child invocations, and `{ .. }` matches
/// a nested [`AuthSubTree`].
pub enum SubInvocationPattern {
    /// Matches any single child invocation (`_`).
    Any,
    /// Matches zero or more child invocations (`..`).
    AnyMany,
    /// Matches a nested authorization sub-tree.
    Node(AuthSubTree),
}

/// An ordered pattern list over a node's `sub_invocations`, built by
/// `assert_auth_sub_tree_matches!`.
#[derive(Default)]
pub struct SubInvocations {
    patterns: Vec<SubInvocationPattern>,
}

impl SubInvocations {
    pub fn new() -> Self {
        Self::default()
    }

    /// Matches any single child invocation at the next position (`_`).
    pub fn any(mut self) -> Self {
        self.patterns.push(SubInvocationPattern::Any);
        self
    }

    /// Matches zero or more child invocations at this position (`..`).
    pub fn any_many(mut self) -> Self {
        self.patterns.push(SubInvocationPattern::AnyMany);
        self
    }

    /// Requires the next child invocation to match `node`.
    pub fn node(mut self, node: AuthSubTree) -> Self {
        self.patterns.push(SubInvocationPattern::Node(node));
        self
    }
}

/// One node in an expected authorization sub-tree.
///
/// Every filter (`contract`, `function`, `args`, `sub_invocations`) is
/// optional: leaving a filter unset accepts any value there. Leaving
/// `sub_invocations` unset (or calling [`Self::any_sub_invocations`]) accepts
/// any child invocations, whereas passing an empty [`SubInvocations`] requires
/// a leaf node with no children.
#[derive(Default)]
pub struct AuthSubTree {
    contract: Option<Address>,
    function: Option<String>,
    args: Option<AuthArgs>,
    sub_invocations: Option<SubInvocations>,
}

impl AuthSubTree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Restricts matches to invocations targeting `contract`.
    pub fn contract(mut self, contract: &Address) -> Self {
        self.contract = Some(contract.clone());
        self
    }

    /// Restricts matches to contract calls named `function`.
    pub fn function(mut self, function: impl Into<String>) -> Self {
        self.function = Some(function.into());
        self
    }

    /// Restricts matches to invocations whose arguments satisfy `args`.
    pub fn args(mut self, args: AuthArgs) -> Self {
        self.args = Some(args);
        self
    }

    /// Requires this node's child invocations to match `children`.
    pub fn sub_invocations(mut self, children: SubInvocations) -> Self {
        self.sub_invocations = Some(children);
        self
    }

    /// Accepts any child invocations under this node (zero or more).
    pub fn any_sub_invocations(mut self) -> Self {
        self.sub_invocations = None;
        self
    }

    fn matches_header(&self, env: &Env, invocation: &AuthorizedInvocation) -> bool {
        let needs_contract_fn =
            self.contract.is_some() || self.function.is_some() || self.args.is_some();
        if !needs_contract_fn {
            return true;
        }

        let AuthorizedFunction::Contract((contract, function, args)) = &invocation.function else {
            return false;
        };

        if self
            .contract
            .as_ref()
            .is_some_and(|expected| expected != contract)
        {
            return false;
        }

        if self
            .function
            .as_ref()
            .is_some_and(|expected| function.to_string() != *expected)
        {
            return false;
        }

        match &self.args {
            Some(expected) => expected.matches_vec(env, args),
            None => true,
        }
    }

    fn matches_root(&self, env: &Env, invocation: &AuthorizedInvocation) -> bool {
        if !self.matches_header(env, invocation) {
            return false;
        }
        match &self.sub_invocations {
            Some(expected) => {
                matches_children(env, &expected.patterns, &invocation.sub_invocations)
            }
            None => true,
        }
    }

    fn matches_anywhere(&self, env: &Env, invocation: &AuthorizedInvocation) -> bool {
        if self.matches_root(env, invocation) {
            return true;
        }
        invocation
            .sub_invocations
            .iter()
            .any(|child| self.matches_anywhere(env, child))
    }

    fn describe_header(&self, env: &Env) -> String {
        let mut parts = Vec::new();
        if let Some(contract) = &self.contract {
            parts.push(format!("contract {contract:?}"));
        }
        if let Some(function) = &self.function {
            parts.push(format!("function \"{function}\""));
        }
        if let Some(args) = &self.args {
            parts.push(format!("args [{}]", args.describe(env)));
        }
        if parts.is_empty() {
            "_".to_string()
        } else {
            parts.join(" ")
        }
    }
}

/// Asserts that an authorization sub-tree matching the pattern appears at any
/// depth inside the authorizations recorded by the most recent invocation.
///
/// Multi-hop routing protocols (such as `aggregator -> pool -> vault`) record
/// nested `AuthorizedInvocation` trees in `env.auths()`. This macro locates
/// and validates a sub-tree starting from any intermediate contract node,
/// without requiring the test to repeat outer wrapper calls.
///
/// All fields are optional and may appear in any order:
///
/// - `invoker: addr` (or `_` / omitted) restricts matches to authorizations
///   owned by `addr`.
/// - `contract: id` (or `_` / omitted) restricts matches to calls targeting
///   contract `id`.
/// - `function: "name"` (or `_` / omitted) restricts matches to calls named
///   `"name"`.
/// - `args: [_, val]` (or `_` / omitted) matches arguments positionally with
///   `_` wildcards.
/// - `sub_invocations: [...]` matches child invocations in order, where `_`
///   matches any single child, `..` matches zero or more children, and
///   `{ ... }` matches a nested sub-tree. Omitting `sub_invocations` or writing
///   `sub_invocations: _` accepts any children; `sub_invocations: []` requires
///   a leaf node with no children.
///
/// ```text
/// assert_auth_sub_tree_matches!(env, {
///     invoker: user,
///     contract: pool_id,
///     function: "route",
///     args: [_, 100i128],
///     sub_invocations: [
///         {
///             contract: vault_id,
///             function: "deposit",
///             args: [_, 100i128],
///         },
///         ..,
///     ],
/// });
/// ```
///
/// Failing the assertion renders a side-by-side comparison of the expected
/// sub-tree (highlighting missing branches with `[MISSING]`) next to every
/// recorded authorization tree.
#[macro_export]
macro_rules! assert_auth_sub_tree_matches {
    ($env:expr, { $($body:tt)* }) => {{
        let matcher = $crate::auth::AuthSubTreeMatcher::new(&$env);
        let env = matcher.env();
        let _ = env;
        $crate::__assert_auth_sub_tree_root!(
            env;
            matcher;
            $($body)*
        )
        .assert_matches()
    }};
}

/// Parses root-level fields of `assert_auth_sub_tree_matches!`.
#[doc(hidden)]
#[macro_export]
macro_rules! __assert_auth_sub_tree_root {
    ($env:expr; $matcher:expr;) => {
        $matcher
    };
    ($env:expr; $matcher:expr; invoker: _ $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_root!($env; $matcher; $($($rest)*)?)
    };
    ($env:expr; $matcher:expr; invoker: $invoker:expr $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_root!(
            $env;
            $matcher.invoker(&$invoker);
            $($($rest)*)?
        )
    };
    ($env:expr; $matcher:expr; contract: _ $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_root!($env; $matcher; $($($rest)*)?)
    };
    ($env:expr; $matcher:expr; contract: $contract:expr $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_root!(
            $env;
            $matcher.contract(&$contract);
            $($($rest)*)?
        )
    };
    ($env:expr; $matcher:expr; function: _ $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_root!($env; $matcher; $($($rest)*)?)
    };
    ($env:expr; $matcher:expr; function: $function:expr $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_root!(
            $env;
            $matcher.function($function);
            $($($rest)*)?
        )
    };
    ($env:expr; $matcher:expr; args: _ $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_root!($env; $matcher; $($($rest)*)?)
    };
    ($env:expr; $matcher:expr; args: [$($args:tt)*] $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_root!(
            $env;
            $matcher.args($crate::__assert_auth_args!(
                $env;
                $crate::auth::AuthArgs::new();
                $($args)*
            ));
            $($($rest)*)?
        )
    };
    ($env:expr; $matcher:expr; sub_invocations: _ $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_root!(
            $env;
            $matcher.any_sub_invocations();
            $($($rest)*)?
        )
    };
    ($env:expr; $matcher:expr; sub_invocations: [$($children:tt)*] $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_root!(
            $env;
            $matcher.sub_invocations($crate::__assert_auth_sub_tree_children!(
                $env;
                $crate::auth::SubInvocations::new();
                $($children)*
            ));
            $($($rest)*)?
        )
    };
}

/// Parses nested node fields inside `sub_invocations: [ { ... } ]`.
#[doc(hidden)]
#[macro_export]
macro_rules! __assert_auth_sub_tree_node {
    ($env:expr; $node:expr;) => {
        $node
    };
    ($env:expr; $node:expr; contract: _ $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_node!($env; $node; $($($rest)*)?)
    };
    ($env:expr; $node:expr; contract: $contract:expr $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_node!(
            $env;
            $node.contract(&$contract);
            $($($rest)*)?
        )
    };
    ($env:expr; $node:expr; function: _ $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_node!($env; $node; $($($rest)*)?)
    };
    ($env:expr; $node:expr; function: $function:expr $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_node!(
            $env;
            $node.function($function);
            $($($rest)*)?
        )
    };
    ($env:expr; $node:expr; args: _ $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_node!($env; $node; $($($rest)*)?)
    };
    ($env:expr; $node:expr; args: [$($args:tt)*] $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_node!(
            $env;
            $node.args($crate::__assert_auth_args!(
                $env;
                $crate::auth::AuthArgs::new();
                $($args)*
            ));
            $($($rest)*)?
        )
    };
    ($env:expr; $node:expr; sub_invocations: _ $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_node!(
            $env;
            $node.any_sub_invocations();
            $($($rest)*)?
        )
    };
    ($env:expr; $node:expr; sub_invocations: [$($children:tt)*] $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_node!(
            $env;
            $node.sub_invocations($crate::__assert_auth_sub_tree_children!(
                $env;
                $crate::auth::SubInvocations::new();
                $($children)*
            ));
            $($($rest)*)?
        )
    };
}

/// Builds a [`SubInvocations`] list from the token list inside
/// `sub_invocations: [..]`.
#[doc(hidden)]
#[macro_export]
macro_rules! __assert_auth_sub_tree_children {
    ($env:expr; $builder:expr;) => {
        $builder
    };
    ($env:expr; $builder:expr; _ $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_children!(
            $env;
            $builder.any();
            $($($rest)*)?
        )
    };
    ($env:expr; $builder:expr; .. $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_children!(
            $env;
            $builder.any_many();
            $($($rest)*)?
        )
    };
    ($env:expr; $builder:expr; { $($node:tt)* } $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_tree_children!(
            $env;
            $builder.node($crate::__assert_auth_sub_tree_node!(
                $env;
                $crate::auth::AuthSubTree::new();
                $($node)*
            ));
            $($($rest)*)?
        )
    };
}

/// Asserts that the invocation that just ran recorded an authorization matching
/// `{ invoker, function, args }`.
///
/// Reads `env.auths()`, which in SDK v28 reports the authorizations seen during
/// the **most recent** contract invocation — the same scope as
/// `EventMatcher::new`. Use `env.mock_all_auths()` (the default of
/// `TestContextBuilder`) when the test needs the requirements recorded rather
/// than enforced.
///
/// Both `invoker` and `function` say what to match, and `invoker` may be left out
/// when any invoker will do. `args` is optional and matched position by position;
/// `_` accepts any value there, so a test names only the arguments it cares
/// about:
///
/// ```text
/// assert_auth_matches!(env, {
///     invoker: admin,
///     function: "set_admin",
/// });
///
/// assert_auth_matches!(env, {
///     invoker: admin,
///     function: "set_admin",
///     args: [_, new_admin],
/// });
/// ```
///
/// Failing the assertion prints the expectation next to every authorization
/// that was recorded, so the mismatch is visible without a re-run.
#[macro_export]
macro_rules! assert_auth_matches {
    ($env:expr, {
        $(invoker: $invoker:expr,)?
        function: $function:expr
        $(, args: [$($args:tt)*])?
        $(,)?
    }) => {{
        let matcher = $crate::auth::AuthMatcher::new(&$env).function($function);
        $(let matcher = matcher.invoker(&$invoker);)?
        $(let matcher = matcher.args($crate::__assert_auth_args!(
            &$env;
            $crate::auth::AuthArgs::new();
            $($args)*
        ));)?
        matcher.assert_matches()
    }};
}

/// Builds an [`AuthArgs`] from the token list a user wrote inside `args: [..]`.
///
/// Both the empty list and a trailing comma end in the same rule, because the
/// comma is consumed only when another entry follows it.
#[doc(hidden)]
#[macro_export]
macro_rules! __assert_auth_args {
    ($env:expr; $builder:expr;) => { $builder };
    ($env:expr; $builder:expr; _ , $($rest:tt)*) => {
        $crate::__assert_auth_args!($env; $builder.any(); $($rest)*)
    };
    ($env:expr; $builder:expr; _) => { $builder.any() };
    ($env:expr; $builder:expr; $value:expr , $($rest:tt)*) => {
        $crate::__assert_auth_args!($env; $builder.value(&$env, $value); $($rest)*)
    };
    ($env:expr; $builder:expr; $value:expr) => { $builder.value(&$env, $value) };
}

/// Asserts that a sub-invocation matching `{ contract, function, args }` appears
/// at any depth inside recorded authorization trees.
///
/// Multi-hop routing protocols (such as `aggregator -> pool -> vault`) record
/// nested `AuthorizedInvocation` trees in `env.auths()`. Developers often only
/// care about asserting that a specific intermediate contract properly authorized
/// a sub-call, regardless of top-level invoker details.
///
/// All fields are optional and may appear in any order:
///
/// - `contract: addr` (or `_` / omitted) restricts matches to calls targeting `addr`.
/// - `function: "name"` (or `_` / omitted) restricts matches to calls named `"name"`.
/// - `args: [_, val]` (or `_` / omitted) matches arguments positionally with `_` wildcards.
/// - `invoker: addr` (or `_` / omitted) restricts matches to authorization trees
///   originating from `addr`.
///
/// ```text
/// assert_auth_sub_invocation!(env, {
///     contract: pool_id,
///     function: "route",
///     args: [_, vault_id, 500i128],
/// });
/// ```
///
/// Failing the assertion displays the expected sub-invocation and formats the full
/// recorded authorization tree(s) displaying all sub-invocations.
#[macro_export]
macro_rules! assert_auth_sub_invocation {
    ($env:expr, { $($body:tt)* } $(,)?) => {{
        let env_ref = $crate::auth::IntoEnvRef::into_env_ref(&$env);
        let matcher = $crate::auth::AuthSubInvocationMatcher::new(env_ref);
        let env = matcher.env();
        let _ = env;
        $crate::__assert_auth_sub_invocation_fields!(
            env;
            matcher;
            $($body)*
        )
        .assert_matches()
    }};
}

/// Parses fields of `assert_auth_sub_invocation!`.
#[doc(hidden)]
#[macro_export]
macro_rules! __assert_auth_sub_invocation_fields {
    ($env:expr; $matcher:expr;) => {
        $matcher
    };
    ($env:expr; $matcher:expr; invoker: _ $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_invocation_fields!($env; $matcher; $($($rest)*)?)
    };
    ($env:expr; $matcher:expr; invoker: $invoker:expr $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_invocation_fields!(
            $env;
            $matcher.invoker($crate::auth::AsAddress::as_address(&$invoker));
            $($($rest)*)?
        )
    };
    ($env:expr; $matcher:expr; contract: _ $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_invocation_fields!($env; $matcher; $($($rest)*)?)
    };
    ($env:expr; $matcher:expr; contract: $contract:expr $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_invocation_fields!(
            $env;
            $matcher.contract($crate::auth::AsAddress::as_address(&$contract));
            $($($rest)*)?
        )
    };
    ($env:expr; $matcher:expr; function: _ $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_invocation_fields!($env; $matcher; $($($rest)*)?)
    };
    ($env:expr; $matcher:expr; function: $function:expr $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_invocation_fields!(
            $env;
            $matcher.function($function);
            $($($rest)*)?
        )
    };
    ($env:expr; $matcher:expr; args: _ $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_invocation_fields!($env; $matcher; $($($rest)*)?)
    };
    ($env:expr; $matcher:expr; args: [$($args:tt)*] $(, $($rest:tt)*)?) => {
        $crate::__assert_auth_sub_invocation_fields!(
            $env;
            $matcher.args($crate::__assert_auth_args!(
                $env;
                $crate::auth::AuthArgs::new();
                $($args)*
            ));
            $($($rest)*)?
        )
    };
}

/// Assertions over the authorizations an invocation recorded.
///
/// The filters chain; [`Self::assert_matches`] is the assertion that reads them,
/// while [`Self::assert_no_auth_required`] and [`Self::assert_auth_count`] judge
/// the same recorded set without filtering.
pub struct AuthMatcher<'a> {
    env: &'a Env,
    invoker: Option<Address>,
    function: Option<String>,
    args: Option<AuthArgs>,
}

impl<'a> AuthMatcher<'a> {
    pub fn new(env: &'a Env) -> Self {
        Self {
            env,
            invoker: None,
            function: None,
            args: None,
        }
    }

    /// Restricts matches to authorizations recorded for `invoker`.
    pub fn invoker(mut self, invoker: &Address) -> Self {
        self.invoker = Some(invoker.clone());
        self
    }

    /// Restricts matches to authorizations of the contract function `function`.
    pub fn function(mut self, function: impl Into<String>) -> Self {
        self.function = Some(function.into());
        self
    }

    /// Restricts matches to authorizations whose arguments fit `args`.
    pub fn args(mut self, args: AuthArgs) -> Self {
        self.args = Some(args);
        self
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

    /// Fails unless one recorded authorization satisfies every configured
    /// filter, and prints all of them when none does.
    ///
    /// A matcher with no filters accepts any recorded authorization, which makes
    /// it the "something was authorized" assertion.
    pub fn assert_matches(&self) {
        let auths = self.env.auths();
        if auths
            .iter()
            .any(|(invoker, invocation)| self.matches(invoker, invocation))
        {
            return;
        }

        let recorded = if auths.is_empty() {
            "  (none recorded)".to_string()
        } else {
            auths
                .iter()
                .enumerate()
                .map(|(index, (invoker, invocation))| {
                    format!(
                        "  [{index}] {}",
                        describe_auth(self.env, invoker, invocation)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        panic!(
            "Expected an authorization matching {}, found {} recorded authorization(s):\n{}",
            self.describe(),
            auths.len(),
            recorded,
        );
    }

    fn matches(&self, invoker: &Address, invocation: &AuthorizedInvocation) -> bool {
        if let Some(expected) = &self.invoker {
            if expected != invoker {
                return false;
            }
        }

        // Only a contract function carries a name and arguments; a host function
        // authorization can never satisfy a `function:` filter.
        let AuthorizedFunction::Contract((_, function, args)) = &invocation.function else {
            return false;
        };

        if let Some(expected) = &self.function {
            if function.to_string() != *expected {
                return false;
            }
        }

        match &self.args {
            Some(expected) => expected.matches_vec(self.env, args),
            None => true,
        }
    }

    /// The expectation as the failure message spells it.
    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(invoker) = &self.invoker {
            parts.push(format!("invoker {invoker:?}"));
        }
        if let Some(function) = &self.function {
            parts.push(format!("function \"{function}\""));
        }
        if let Some(args) = &self.args {
            parts.push(format!("args [{}]", args.describe(self.env)));
        }
        format!("{{ {} }}", parts.join(", "))
    }
}

/// Assertions over authorization sub-trees at any depth in `env.auths()`.
///
/// Unlike [`AuthMatcher`], which inspects only top-level authorizations,
/// `AuthSubTreeMatcher` searches every recorded [`AuthorizedInvocation`] tree
/// recursively so multi-hop routing tests can validate an intermediate
/// contract hop without repeating the entire outer call stack.
pub struct AuthSubTreeMatcher<'a> {
    env: &'a Env,
    invoker: Option<Address>,
    root: AuthSubTree,
}

impl<'a> AuthSubTreeMatcher<'a> {
    pub fn new(env: &'a Env) -> Self {
        Self {
            env,
            invoker: None,
            root: AuthSubTree::new(),
        }
    }

    /// The environment this matcher inspects.
    pub fn env(&self) -> &'a Env {
        self.env
    }

    /// Restricts matches to authorization trees owned by `invoker`.
    pub fn invoker(mut self, invoker: &Address) -> Self {
        self.invoker = Some(invoker.clone());
        self
    }

    /// Replaces the expected sub-tree pattern directly.
    pub fn sub_tree(mut self, root: AuthSubTree) -> Self {
        self.root = root;
        self
    }

    /// Restricts the sub-tree root to invocations targeting `contract`.
    pub fn contract(mut self, contract: &Address) -> Self {
        self.root = self.root.contract(contract);
        self
    }

    /// Restricts the sub-tree root to calls named `function`.
    pub fn function(mut self, function: impl Into<String>) -> Self {
        self.root = self.root.function(function);
        self
    }

    /// Restricts the sub-tree root to calls whose arguments fit `args`.
    pub fn args(mut self, args: AuthArgs) -> Self {
        self.root = self.root.args(args);
        self
    }

    /// Requires the sub-tree root's child invocations to match `children`.
    pub fn sub_invocations(mut self, children: SubInvocations) -> Self {
        self.root = self.root.sub_invocations(children);
        self
    }

    /// Accepts any child invocations under the sub-tree root (zero or more).
    pub fn any_sub_invocations(mut self) -> Self {
        self.root = self.root.any_sub_invocations();
        self
    }

    /// Fails unless at least one recorded authorization tree contains a node
    /// matching this sub-tree at any depth, printing a side-by-side tree
    /// comparison on failure.
    pub fn assert_matches(&self) {
        let auths = self.env.auths();
        let matched = auths.iter().any(|(invoker, invocation)| {
            let invoker_ok = match &self.invoker {
                Some(expected) => expected == invoker,
                None => true,
            };
            invoker_ok && self.root.matches_anywhere(self.env, invocation)
        });

        if matched {
            return;
        }

        let comparison = self.render_side_by_side(&auths);
        panic!(
            "Expected authorization sub-tree did not match any recorded authorization ({} recorded):\n\n{}",
            auths.len(),
            comparison,
        );
    }

    fn render_side_by_side(&self, auths: &[(Address, AuthorizedInvocation)]) -> String {
        let mut expected_lines = Vec::new();
        let invoker_matched = match &self.invoker {
            Some(expected) => auths.iter().any(|(invoker, _)| invoker == expected),
            None => !auths.is_empty(),
        };

        if let Some(invoker) = &self.invoker {
            let suffix = if invoker_matched { "" } else { " [MISSING]" };
            expected_lines.push(format!("invoker {invoker:?}{suffix}"));
        }

        let best_candidate = if invoker_matched {
            self.find_best_candidate(auths)
        } else {
            None
        };
        render_expected_node(
            self.env,
            &self.root,
            best_candidate,
            "",
            TreeBranch::Root,
            &mut expected_lines,
        );

        let mut recorded_lines = Vec::new();
        if auths.is_empty() {
            recorded_lines.push("(none recorded)".to_string());
        } else {
            for (index, (invoker, invocation)) in auths.iter().enumerate() {
                if index > 0 {
                    recorded_lines.push(String::new());
                }
                recorded_lines.push(format!("[{index}] invoker {invoker:?}"));
                render_recorded_node(
                    self.env,
                    invocation,
                    "",
                    TreeBranch::Root,
                    &mut recorded_lines,
                );
            }
        }

        format_side_by_side(
            "Expected sub-tree",
            &expected_lines,
            "Recorded authorization tree(s)",
            &recorded_lines,
        )
    }

    fn find_best_candidate<'b>(
        &self,
        auths: &'b [(Address, AuthorizedInvocation)],
    ) -> Option<&'b AuthorizedInvocation> {
        let mut best: Option<(usize, &'b AuthorizedInvocation)> = None;
        for (invoker, invocation) in auths {
            if self
                .invoker
                .as_ref()
                .is_some_and(|expected| expected != invoker)
            {
                continue;
            }
            collect_best_candidate(self.env, &self.root, invocation, &mut best);
        }
        best.map(|(_, node)| node)
    }
}

/// Helper trait to convert either `&Env` or `&&Env` into `&Env` preserving lifetime.
pub trait IntoEnvRef<'a> {
    fn into_env_ref(self) -> &'a Env;
}

impl<'a> IntoEnvRef<'a> for &'a Env {
    fn into_env_ref(self) -> &'a Env {
        self
    }
}

impl<'a> IntoEnvRef<'a> for &&'a Env {
    fn into_env_ref(self) -> &'a Env {
        self
    }
}

/// Helper trait to accept either `Address` or `&Address` as an address reference.
pub trait AsAddress {
    fn as_address(&self) -> &Address;
}

impl AsAddress for Address {
    fn as_address(&self) -> &Address {
        self
    }
}

impl<T: AsAddress + ?Sized> AsAddress for &T {
    fn as_address(&self) -> &Address {
        (*self).as_address()
    }
}

/// Assertions over intermediate contract sub-invocations recorded anywhere in `env.auths()`.
///
/// While [`AuthMatcher`] checks top-level authorizations and [`AuthSubTreeMatcher`]
/// validates nested child invocation structures, `AuthSubInvocationMatcher` searches
/// recursively across all recorded [`AuthorizedInvocation`] trees to find any call
/// matching `{ contract, function, args }` regardless of invocation depth or outer callers.
pub struct AuthSubInvocationMatcher<'a> {
    env: &'a Env,
    invoker: Option<Address>,
    contract: Option<Address>,
    function: Option<String>,
    args: Option<AuthArgs>,
}

impl<'a> AuthSubInvocationMatcher<'a> {
    pub fn new(env: &'a Env) -> Self {
        Self {
            env,
            invoker: None,
            contract: None,
            function: None,
            args: None,
        }
    }

    /// The environment this matcher inspects.
    pub fn env(&self) -> &'a Env {
        self.env
    }

    /// Restricts matches to authorization trees originating from `invoker`.
    pub fn invoker(mut self, invoker: impl AsAddress) -> Self {
        self.invoker = Some(invoker.as_address().clone());
        self
    }

    /// Restricts matches to invocations targeting `contract`.
    pub fn contract(mut self, contract: impl AsAddress) -> Self {
        self.contract = Some(contract.as_address().clone());
        self
    }

    /// Restricts matches to contract calls named `function`.
    pub fn function(mut self, function: impl Into<String>) -> Self {
        self.function = Some(function.into());
        self
    }

    /// Restricts matches to invocations whose arguments satisfy `args`.
    pub fn args(mut self, args: AuthArgs) -> Self {
        self.args = Some(args);
        self
    }

    /// Checks if a single invocation matches the configured filters.
    fn matches_invocation(&self, invocation: &AuthorizedInvocation) -> bool {
        let needs_contract_fn =
            self.contract.is_some() || self.function.is_some() || self.args.is_some();
        if !needs_contract_fn {
            return true;
        }

        let AuthorizedFunction::Contract((contract, function, args)) = &invocation.function else {
            return false;
        };

        if self
            .contract
            .as_ref()
            .is_some_and(|expected| expected != contract)
        {
            return false;
        }

        if self
            .function
            .as_ref()
            .is_some_and(|expected| function.to_string() != *expected)
        {
            return false;
        }

        match &self.args {
            Some(expected) => expected.matches_vec(self.env, args),
            None => true,
        }
    }

    /// Recursively traverses `invocation` and its child `sub_invocations` to
    /// find any matching call.
    fn matches_anywhere(&self, invocation: &AuthorizedInvocation) -> bool {
        if self.matches_invocation(invocation) {
            return true;
        }
        invocation
            .sub_invocations
            .iter()
            .any(|child| self.matches_anywhere(child))
    }

    /// Returns `true` if any recorded authorization tree contains a matching sub-invocation.
    pub fn matches_any(&self) -> bool {
        let auths = self.env.auths();
        auths.iter().any(|(invoker, invocation)| {
            let invoker_ok = match &self.invoker {
                Some(expected) => expected == invoker,
                None => true,
            };
            invoker_ok && self.matches_anywhere(invocation)
        })
    }

    /// Formats the full authorization tree(s) as diagnostic output.
    pub fn render_diagnostic(&self, auths: &[(Address, AuthorizedInvocation)]) -> String {
        if auths.is_empty() {
            return "  (none recorded)".to_string();
        }

        let mut recorded_lines = Vec::new();
        for (index, (invoker, invocation)) in auths.iter().enumerate() {
            if index > 0 {
                recorded_lines.push(String::new());
            }
            recorded_lines.push(format!("[{index}] invoker {invoker:?}"));
            render_recorded_node(
                self.env,
                invocation,
                "",
                TreeBranch::Root,
                &mut recorded_lines,
            );
        }
        recorded_lines.join("\n")
    }

    /// Formats the expected sub-invocation description.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(invoker) = &self.invoker {
            parts.push(format!("invoker {invoker:?}"));
        }
        if let Some(contract) = &self.contract {
            parts.push(format!("contract {contract:?}"));
        }
        if let Some(function) = &self.function {
            parts.push(format!("function \"{function}\""));
        }
        if let Some(args) = &self.args {
            parts.push(format!("args [{}]", args.describe(self.env)));
        }
        if parts.is_empty() {
            "{ _ }".to_string()
        } else {
            format!("{{ {} }}", parts.join(", "))
        }
    }

    /// Fails unless at least one recorded authorization tree contains a matching sub-invocation,
    /// printing the full authorization tree(s) on failure.
    pub fn assert_matches(&self) {
        if self.matches_any() {
            return;
        }

        let auths = self.env.auths();
        let diagnostic = self.render_diagnostic(&auths);
        panic!(
            "Expected authorization sub-invocation matching {}, found no match in {} recorded authorization tree(s):\n{}",
            self.describe(),
            auths.len(),
            diagnostic,
        );
    }
}

enum TreeBranch {
    Root,
    Middle,
    Last,
}

impl TreeBranch {
    fn connector(&self) -> &'static str {
        match self {
            TreeBranch::Root => "",
            TreeBranch::Middle => "├── ",
            TreeBranch::Last => "└── ",
        }
    }

    fn child_indent(&self) -> &'static str {
        match self {
            TreeBranch::Root => "",
            TreeBranch::Middle => "│   ",
            TreeBranch::Last => "    ",
        }
    }
}

fn matches_children(
    env: &Env,
    patterns: &[SubInvocationPattern],
    actuals: &[AuthorizedInvocation],
) -> bool {
    match patterns.split_first() {
        None => actuals.is_empty(),
        Some((SubInvocationPattern::AnyMany, rest_patterns)) => {
            for skip in 0..=actuals.len() {
                if matches_children(env, rest_patterns, &actuals[skip..]) {
                    return true;
                }
            }
            false
        }
        Some((SubInvocationPattern::Any, rest_patterns)) => {
            let Some((_, rest_actuals)) = actuals.split_first() else {
                return false;
            };
            matches_children(env, rest_patterns, rest_actuals)
        }
        Some((SubInvocationPattern::Node(node), rest_patterns)) => {
            let Some((first_actual, rest_actuals)) = actuals.split_first() else {
                return false;
            };
            node.matches_root(env, first_actual)
                && matches_children(env, rest_patterns, rest_actuals)
        }
    }
}

fn collect_best_candidate<'b>(
    env: &Env,
    expected: &AuthSubTree,
    actual: &'b AuthorizedInvocation,
    best: &mut Option<(usize, &'b AuthorizedInvocation)>,
) {
    let score = candidate_score(env, expected, actual);
    if score > 0 && best.is_none_or(|(best_score, _)| score > best_score) {
        *best = Some((score, actual));
    }
    for child in &actual.sub_invocations {
        collect_best_candidate(env, expected, child, best);
    }
}

fn candidate_score(env: &Env, expected: &AuthSubTree, actual: &AuthorizedInvocation) -> usize {
    let AuthorizedFunction::Contract((contract, function, args)) = &actual.function else {
        return usize::from(
            expected.contract.is_none() && expected.function.is_none() && expected.args.is_none(),
        );
    };

    if expected
        .contract
        .as_ref()
        .is_some_and(|wanted| wanted != contract)
    {
        return 0;
    }

    if expected
        .function
        .as_ref()
        .is_some_and(|wanted| function.to_string() != *wanted)
    {
        return 0;
    }

    let mut score = 1;
    if expected
        .args
        .as_ref()
        .is_none_or(|wanted| wanted.matches_vec(env, args))
    {
        score += 2;
    }

    if let Some(children) = &expected.sub_invocations {
        for pattern in &children.patterns {
            if let SubInvocationPattern::Node(child_expected) = pattern {
                let matched = actual
                    .sub_invocations
                    .iter()
                    .any(|child| child_expected.matches_header(env, child));
                score += usize::from(matched);
            }
        }
    }

    score
}

fn render_expected_node(
    env: &Env,
    node: &AuthSubTree,
    candidate: Option<&AuthorizedInvocation>,
    prefix: &str,
    branch: TreeBranch,
    out: &mut Vec<String>,
) {
    let header_matches = candidate.is_some_and(|actual| node.matches_header(env, actual));
    let status = if header_matches { "" } else { " [MISSING]" };
    let connector = branch.connector();
    let header = node.describe_header(env);
    out.push(format!("{prefix}{connector}{header}{status}"));

    let child_prefix = format!("{prefix}{}", branch.child_indent());
    let Some(children) = &node.sub_invocations else {
        return;
    };

    if children.patterns.is_empty() {
        let mismatch = candidate.is_some_and(|actual| !actual.sub_invocations.is_empty());
        let suffix = if mismatch { " [MISMATCH]" } else { "" };
        out.push(format!("{child_prefix}└── (no sub-invocations){suffix}"));
        return;
    }

    let actual_children = candidate
        .filter(|_| header_matches)
        .map(|actual| actual.sub_invocations.as_slice())
        .unwrap_or(&[]);

    let last_index = children.patterns.len() - 1;
    for (index, pattern) in children.patterns.iter().enumerate() {
        let child_branch = if index == last_index {
            TreeBranch::Last
        } else {
            TreeBranch::Middle
        };
        match pattern {
            SubInvocationPattern::Any => {
                let suffix = if actual_children.is_empty() {
                    " [MISSING]"
                } else {
                    ""
                };
                let connector = child_branch.connector();
                out.push(format!("{child_prefix}{connector}_{suffix}"));
            }
            SubInvocationPattern::AnyMany => {
                out.push(format!("{child_prefix}{}..", child_branch.connector()));
            }
            SubInvocationPattern::Node(child_node) => {
                let child_candidate = actual_children
                    .iter()
                    .find(|actual| child_node.matches_root(env, actual))
                    .or_else(|| {
                        actual_children
                            .iter()
                            .filter(|actual| candidate_score(env, child_node, actual) > 0)
                            .max_by_key(|actual| candidate_score(env, child_node, actual))
                    });
                render_expected_node(
                    env,
                    child_node,
                    child_candidate,
                    &child_prefix,
                    child_branch,
                    out,
                );
            }
        }
    }
}

fn render_recorded_node(
    env: &Env,
    invocation: &AuthorizedInvocation,
    prefix: &str,
    branch: TreeBranch,
    out: &mut Vec<String>,
) {
    let connector = branch.connector();
    let label = describe_invocation(env, invocation);
    out.push(format!("{prefix}{connector}{label}"));

    let child_prefix = format!("{prefix}{}", branch.child_indent());
    let count = invocation.sub_invocations.len();
    for (index, child) in invocation.sub_invocations.iter().enumerate() {
        let child_branch = if index + 1 == count {
            TreeBranch::Last
        } else {
            TreeBranch::Middle
        };
        render_recorded_node(env, child, &child_prefix, child_branch, out);
    }
}

fn format_side_by_side(
    left_header: &str,
    left_lines: &[String],
    right_header: &str,
    right_lines: &[String],
) -> String {
    let left_width = left_lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0)
        .max(left_header.chars().count());
    let right_width = right_lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0)
        .max(right_header.chars().count());

    let mut rows = Vec::with_capacity(left_lines.len().max(right_lines.len()) + 2);
    let header_pad = " ".repeat(left_width.saturating_sub(left_header.chars().count()));
    rows.push(format!("{left_header}{header_pad} | {right_header}"));
    let left_rule = "-".repeat(left_width);
    let right_rule = "-".repeat(right_width);
    rows.push(format!("{left_rule}-+-{right_rule}"));

    let total_rows = left_lines.len().max(right_lines.len());
    for row in 0..total_rows {
        let left = left_lines.get(row).map_or("", String::as_str);
        let right = right_lines.get(row).map_or("", String::as_str);
        let pad = left_width.saturating_sub(left.chars().count());
        if right.is_empty() {
            rows.push(format!("{left}{} |", " ".repeat(pad)));
        } else {
            rows.push(format!("{left}{} | {right}", " ".repeat(pad)));
        }
    }

    rows.join("\n")
}

/// Two ledger values are the same when their XDR is the same.
///
/// A recorded address or symbol and the one the test built are different host
/// objects, so their `Val`s are not interchangeable; converting both through the
/// same env is what makes this an equality of contents rather than of handles.
fn same_value(env: &Env, expected: &Val, actual: &Val) -> bool {
    match (
        ScVal::try_from_val(env, expected),
        ScVal::try_from_val(env, actual),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn describe_invocation(env: &Env, invocation: &AuthorizedInvocation) -> String {
    match &invocation.function {
        AuthorizedFunction::Contract((contract, name, args)) => format!(
            "contract {contract:?} function \"{}\" args {}",
            name.to_string(),
            data_label(env, &ScVal::from(args)),
        ),
        AuthorizedFunction::CreateContractHostFn(_) => "create_contract host function".to_string(),
        AuthorizedFunction::CreateContractV2HostFn(_) => {
            "create_contract v2 host function".to_string()
        }
    }
}

/// One recorded authorization as the failure message spells it.
fn describe_auth(env: &Env, invoker: &Address, invocation: &AuthorizedInvocation) -> String {
    format!(
        "invoker {invoker:?} {}",
        describe_invocation(env, invocation)
    )
}

#[cfg(test)]
mod tests {
    use super::{
        AuthArgs, AuthMatcher, AuthSubInvocationMatcher, AuthSubTree, AuthSubTreeMatcher,
        SubInvocations,
    };
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{contract, contractimpl, Address, Env};

    // A contract whose calls record real authorizations: one address, two
    // addresses, and a call that needs none at all.
    #[contract]
    pub struct AuthContract;

    #[contractimpl]
    impl AuthContract {
        pub fn guarded(env: Env, signer: Address, amount: u32) {
            signer.require_auth();
            let _ = (env, amount);
        }

        pub fn joint(env: Env, first: Address, second: Address) {
            first.require_auth();
            second.require_auth();
            let _ = env;
        }

        pub fn unguarded(env: Env) {
            let _ = env;
        }
    }

    /// An env whose calls record their auth requirements instead of enforcing
    /// them, plus the id of the contract that records them.
    fn signing_env() -> (Env, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register(AuthContract, ());
        (env, id)
    }

    #[test]
    fn idle_env_requires_no_authorizations() {
        let env = Env::default();
        AuthMatcher::new(&env).assert_no_auth_required();
        AuthMatcher::new(&env).assert_auth_count(0);
    }

    #[test]
    #[should_panic(expected = "Expected 2 authorizations, found 0")]
    fn assert_auth_count_reports_the_observed_total() {
        let env = Env::default();
        AuthMatcher::new(&env).assert_auth_count(2);
    }

    #[test]
    fn matches_the_recorded_invoker_and_function() {
        let (env, id) = signing_env();
        let client = AuthContractClient::new(&env, &id);
        let signer = Address::generate(&env);

        client.guarded(&signer, &5u32);

        assert_auth_matches!(env, {
            invoker: signer,
            function: "guarded",
        });
    }

    #[test]
    fn matches_the_recorded_arguments() {
        let (env, id) = signing_env();
        let client = AuthContractClient::new(&env, &id);
        let signer = Address::generate(&env);

        client.guarded(&signer, &5u32);

        // Every position named.
        assert_auth_matches!(env, {
            invoker: signer,
            function: "guarded",
            args: [signer.clone(), 5u32],
        });

        // `_` leaves the positions the test does not care about alone.
        assert_auth_matches!(env, {
            invoker: signer,
            function: "guarded",
            args: [_, 5u32],
        });
        assert_auth_matches!(env, {
            invoker: signer,
            function: "guarded",
            args: [signer.clone(), _],
        });
    }

    #[test]
    #[should_panic(expected = "Expected an authorization matching { invoker ")]
    fn fails_when_the_function_does_not_match() {
        let (env, id) = signing_env();
        let client = AuthContractClient::new(&env, &id);
        let signer = Address::generate(&env);

        client.guarded(&signer, &5u32);

        assert_auth_matches!(env, {
            invoker: signer,
            function: "unguarded",
        });
    }

    #[test]
    #[should_panic(expected = "function \"guarded\" args [")]
    fn failure_lists_the_recorded_authorization() {
        let (env, id) = signing_env();
        let client = AuthContractClient::new(&env, &id);
        let signer = Address::generate(&env);

        client.guarded(&signer, &5u32);

        assert_auth_matches!(env, {
            invoker: signer,
            function: "unguarded",
        });
    }

    #[test]
    #[should_panic(expected = "Expected an authorization matching { invoker ")]
    fn fails_when_the_invoker_does_not_match() {
        let (env, id) = signing_env();
        let client = AuthContractClient::new(&env, &id);
        let signer = Address::generate(&env);
        let other = Address::generate(&env);

        client.guarded(&signer, &5u32);

        assert_auth_matches!(env, {
            invoker: other,
            function: "guarded",
        });
    }

    #[test]
    #[should_panic(expected = "args [_, 999]")]
    fn fails_when_an_argument_does_not_match() {
        let (env, id) = signing_env();
        let client = AuthContractClient::new(&env, &id);
        let signer = Address::generate(&env);

        client.guarded(&signer, &5u32);

        assert_auth_matches!(env, {
            invoker: signer,
            function: "guarded",
            args: [_, 999u32],
        });
    }

    #[test]
    #[should_panic(expected = "found 0 recorded authorization(s):\n  (none recorded)")]
    fn failure_says_when_nothing_was_recorded() {
        let env = Env::default();
        AuthMatcher::new(&env).function("guarded").assert_matches();
    }

    #[test]
    fn matches_one_of_several_recorded_authorizations() {
        let (env, id) = signing_env();
        let client = AuthContractClient::new(&env, &id);
        let first = Address::generate(&env);
        let second = Address::generate(&env);

        client.joint(&first, &second);

        AuthMatcher::new(&env).assert_auth_count(2);
        assert_auth_matches!(env, {
            invoker: first,
            function: "joint",
            args: [_, _],
        });
        assert_auth_matches!(env, {
            invoker: second,
            function: "joint",
            args: [_, _],
        });
    }

    #[test]
    #[should_panic(expected = "found 2 recorded authorization(s):\n  [0] invoker ")]
    fn failure_lists_every_recorded_authorization() {
        let (env, id) = signing_env();
        let client = AuthContractClient::new(&env, &id);
        let first = Address::generate(&env);
        let second = Address::generate(&env);

        client.joint(&first, &second);

        assert_auth_matches!(env, {
            invoker: first,
            function: "unguarded",
        });
    }

    #[test]
    fn args_builder_accepts_values_and_wildcards() {
        let env = Env::default();
        let signer = Address::generate(&env);
        let args = AuthArgs::new().any().value(&env, signer.clone());

        let matcher = AuthMatcher::new(&env).args(args);
        assert_eq!(matcher.describe(), format!("{{ args [_, {:?}] }}", signer));
    }

    // Multi-hop routing contracts (Aggregator -> Pool -> Vault / FeeCollector)
    // that record real nested `AuthorizedInvocation` sub-trees.
    #[contract]
    pub struct VaultContract;

    #[contractimpl]
    impl VaultContract {
        pub fn deposit(env: Env, user: Address, amount: i128) {
            user.require_auth();
            let _ = (env, amount);
        }
    }

    #[contract]
    pub struct FeeContract;

    #[contractimpl]
    impl FeeContract {
        pub fn collect(env: Env, user: Address, fee: i128) {
            user.require_auth();
            let _ = (env, fee);
        }
    }

    #[contract]
    pub struct PoolContract;

    #[contractimpl]
    impl PoolContract {
        pub fn route(env: Env, user: Address, vault: Address, amount: i128) {
            user.require_auth();
            VaultContractClient::new(&env, &vault).deposit(&user, &amount);
        }

        pub fn route_with_fee(
            env: Env,
            user: Address,
            vault: Address,
            fee_contract: Address,
            amount: i128,
        ) {
            user.require_auth();
            let fee = amount / 10;
            VaultContractClient::new(&env, &vault).deposit(&user, &amount);
            FeeContractClient::new(&env, &fee_contract).collect(&user, &fee);
        }
    }

    #[contract]
    pub struct AggregatorContract;

    #[contractimpl]
    impl AggregatorContract {
        pub fn swap(env: Env, user: Address, pool: Address, vault: Address, amount: i128) {
            user.require_auth();
            PoolContractClient::new(&env, &pool).route(&user, &vault, &amount);
        }

        pub fn swap_with_fee(
            env: Env,
            user: Address,
            pool: Address,
            vault: Address,
            fee_contract: Address,
            amount: i128,
        ) {
            user.require_auth();
            PoolContractClient::new(&env, &pool).route_with_fee(
                &user,
                &vault,
                &fee_contract,
                &amount,
            );
        }
    }

    struct RoutingFixture {
        env: Env,
        aggregator: Address,
        pool: Address,
        vault: Address,
        fee_contract: Address,
        user: Address,
    }

    fn routing_fixture() -> RoutingFixture {
        let env = Env::default();
        env.mock_all_auths();
        let aggregator = env.register(AggregatorContract, ());
        let pool = env.register(PoolContract, ());
        let vault = env.register(VaultContract, ());
        let fee_contract = env.register(FeeContract, ());
        let user = Address::generate(&env);
        RoutingFixture {
            env,
            aggregator,
            pool,
            vault,
            fee_contract,
            user,
        }
    }

    #[test]
    fn sub_tree_matches_intermediate_node_at_depth_one() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &500i128);

        // Matches Pool -> Vault without mentioning the top-level Aggregator hop.
        assert_auth_sub_tree_matches!(fx.env, {
            invoker: fx.user,
            contract: fx.pool,
            function: "route",
            args: [_, fx.vault.clone(), 500i128],
            sub_invocations: [
                {
                    contract: fx.vault,
                    function: "deposit",
                    args: [fx.user.clone(), 500i128],
                    sub_invocations: [],
                },
            ],
        });
    }

    #[test]
    fn sub_tree_matches_leaf_node_at_depth_two() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &250i128);

        assert_auth_sub_tree_matches!(fx.env, {
            invoker: fx.user,
            contract: fx.vault,
            function: "deposit",
            args: [_, 250i128],
            sub_invocations: [],
        });
    }

    #[test]
    fn sub_tree_matches_full_three_hop_chain_from_root() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &100i128);

        assert_auth_sub_tree_matches!(fx.env, {
            invoker: fx.user,
            contract: fx.aggregator,
            function: "swap",
            sub_invocations: [
                {
                    contract: fx.pool,
                    function: "route",
                    sub_invocations: [
                        {
                            contract: fx.vault,
                            function: "deposit",
                            args: [_, 100i128],
                        },
                    ],
                },
            ],
        });
    }

    #[test]
    fn sub_tree_supports_wildcard_arguments_and_child_patterns() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator).swap_with_fee(
            &fx.user,
            &fx.pool,
            &fx.vault,
            &fx.fee_contract,
            &1_000i128,
        );

        // `sub_invocations: _` accepts any child invocations.
        assert_auth_sub_tree_matches!(fx.env, {
            invoker: fx.user,
            contract: fx.pool,
            function: "route_with_fee",
            args: _,
            sub_invocations: _,
        });

        // `_` matches any single child invocation.
        assert_auth_sub_tree_matches!(fx.env, {
            invoker: fx.user,
            function: "route_with_fee",
            sub_invocations: [
                {
                    contract: fx.vault,
                    function: "deposit",
                    args: [_, 1_000i128],
                },
                _,
            ],
        });

        // `..` matches zero or more child invocations before or after a node.
        assert_auth_sub_tree_matches!(fx.env, {
            invoker: fx.user,
            function: "route_with_fee",
            sub_invocations: [
                ..,
                {
                    contract: fx.fee_contract,
                    function: "collect",
                    args: [_, 100i128],
                },
            ],
        });
    }

    #[test]
    fn sub_tree_matches_when_invoker_is_omitted_or_wildcarded() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &75i128);

        assert_auth_sub_tree_matches!(fx.env, {
            function: "deposit",
            args: [_, 75i128],
        });

        assert_auth_sub_tree_matches!(fx.env, {
            invoker: _,
            contract: fx.pool,
            function: "route",
        });
    }

    #[test]
    fn sub_tree_builder_api_validates_nested_invocations() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator).swap_with_fee(
            &fx.user,
            &fx.pool,
            &fx.vault,
            &fx.fee_contract,
            &200i128,
        );

        let expected = AuthSubTree::new()
            .contract(&fx.pool)
            .function("route_with_fee")
            .sub_invocations(
                SubInvocations::new()
                    .node(
                        AuthSubTree::new()
                            .contract(&fx.vault)
                            .function("deposit")
                            .args(AuthArgs::new().any().value(&fx.env, 200i128)),
                    )
                    .any_many(),
            );

        AuthSubTreeMatcher::new(&fx.env)
            .invoker(&fx.user)
            .sub_tree(expected)
            .assert_matches();
    }

    #[test]
    #[should_panic(expected = "[MISSING]")]
    fn sub_tree_failure_highlights_missing_child_branch() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &300i128);

        // `swap` calls only `Vault::deposit`, never `FeeContract::collect`.
        assert_auth_sub_tree_matches!(fx.env, {
            invoker: fx.user,
            contract: fx.pool,
            function: "route",
            sub_invocations: [
                {
                    contract: fx.vault,
                    function: "deposit",
                },
                {
                    contract: fx.fee_contract,
                    function: "collect",
                },
            ],
        });
    }

    #[test]
    #[should_panic(expected = "Expected sub-tree")]
    fn sub_tree_failure_formats_side_by_side_comparison() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &300i128);

        assert_auth_sub_tree_matches!(fx.env, {
            invoker: fx.user,
            contract: fx.pool,
            function: "nonexistent",
        });
    }

    #[test]
    #[should_panic(expected = "Recorded authorization tree(s)")]
    fn sub_tree_failure_includes_recorded_tree_column() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &300i128);

        assert_auth_sub_tree_matches!(fx.env, {
            invoker: fx.user,
            function: "route",
            sub_invocations: [
                {
                    function: "deposit",
                    args: [_, 999i128],
                },
            ],
        });
    }

    #[test]
    #[should_panic(expected = "[MISSING]")]
    fn sub_tree_fails_when_invoker_does_not_match() {
        let fx = routing_fixture();
        let stranger = Address::generate(&fx.env);
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &100i128);

        assert_auth_sub_tree_matches!(fx.env, {
            invoker: stranger,
            function: "route",
        });
    }

    #[test]
    #[should_panic(expected = "[MISMATCH]")]
    fn sub_tree_fails_when_leaf_expected_but_children_exist() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &100i128);

        assert_auth_sub_tree_matches!(fx.env, {
            invoker: fx.user,
            contract: fx.pool,
            function: "route",
            sub_invocations: [],
        });
    }

    #[test]
    #[should_panic(expected = "(none recorded)")]
    fn sub_tree_fails_cleanly_when_no_authorizations_were_recorded() {
        let env = Env::default();
        assert_auth_sub_tree_matches!(env, {
            function: "deposit",
        });
    }

    #[test]
    fn sub_invocation_matches_direct_invocation_at_root() {
        let (env, id) = signing_env();
        let client = AuthContractClient::new(&env, &id);
        let signer = Address::generate(&env);

        client.guarded(&signer, &42u32);

        // Matches direct root invocation with contract, function, and args
        assert_auth_sub_invocation!(env, {
            contract: id,
            function: "guarded",
            args: [signer.clone(), 42u32],
        });

        // Also works passing &env by reference and &id
        assert_auth_sub_invocation!(&env, {
            contract: &id,
            function: "guarded",
            args: [_, 42u32],
        });
    }

    #[test]
    fn sub_invocation_matches_intermediate_call_at_depth_one() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &500i128);

        // Matches Pool -> Vault intermediate call without naming Aggregator or Vault
        assert_auth_sub_invocation!(fx.env, {
            contract: fx.pool,
            function: "route",
            args: [_, fx.vault.clone(), 500i128],
        });
    }

    #[test]
    fn sub_invocation_matches_deep_sub_invocation_at_depth_two() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &500i128);

        // Matches leaf Vault deposit at depth 2 without naming Aggregator or Pool
        assert_auth_sub_invocation!(fx.env, {
            contract: fx.vault,
            function: "deposit",
            args: [fx.user.clone(), 500i128],
        });
    }

    #[test]
    fn sub_invocation_matches_in_branching_sub_tree() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator).swap_with_fee(
            &fx.user,
            &fx.pool,
            &fx.vault,
            &fx.fee_contract,
            &1_000i128,
        );

        // Matches second branch in Pool route_with_fee: FeeContract collect
        assert_auth_sub_invocation!(fx.env, {
            contract: fx.fee_contract,
            function: "collect",
            args: [_, 100i128],
        });

        // Matches first branch: Vault deposit
        assert_auth_sub_invocation!(fx.env, {
            contract: fx.vault,
            function: "deposit",
            args: [fx.user.clone(), 1_000i128],
        });
    }

    #[test]
    fn sub_invocation_supports_wildcard_arguments_and_omitted_filters() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &750i128);

        // Match by function alone across any contract
        assert_auth_sub_invocation!(fx.env, {
            function: "deposit",
        });

        // Match with wildcard arguments
        assert_auth_sub_invocation!(fx.env, {
            function: "deposit",
            args: [_, 750i128],
        });

        // Explicit wildcards: contract: _, function: ..., args: _
        assert_auth_sub_invocation!(fx.env, {
            contract: _,
            function: "deposit",
            args: _,
        });

        // Match with contract and function only, omitting args
        assert_auth_sub_invocation!(fx.env, {
            contract: fx.vault,
            function: "deposit",
        });
    }

    #[test]
    fn sub_invocation_builder_api_matches_nested_call() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &321i128);

        AuthSubInvocationMatcher::new(&fx.env)
            .contract(&fx.vault)
            .function("deposit")
            .args(AuthArgs::new().any().value(&fx.env, 321i128))
            .assert_matches();
    }

    #[test]
    #[should_panic(expected = "found no match in 1 recorded authorization tree(s)")]
    fn sub_invocation_fails_when_call_is_absent() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &100i128);

        assert_auth_sub_invocation!(fx.env, {
            function: "nonexistent_call",
        });
    }

    #[test]
    #[should_panic(expected = "found no match in 1 recorded authorization tree(s)")]
    fn sub_invocation_fails_when_contract_does_not_match() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &100i128);

        // Deposit was invoked on vault, not fee_contract
        assert_auth_sub_invocation!(fx.env, {
            contract: fx.fee_contract,
            function: "deposit",
        });
    }

    #[test]
    #[should_panic(expected = "found no match in 1 recorded authorization tree(s)")]
    fn sub_invocation_fails_when_arguments_do_not_match() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &100i128);

        assert_auth_sub_invocation!(fx.env, {
            function: "deposit",
            args: [_, 9999i128],
        });
    }

    #[test]
    #[should_panic(expected = "(none recorded)")]
    fn sub_invocation_fails_cleanly_when_no_authorizations_recorded() {
        let env = Env::default();
        assert_auth_sub_invocation!(env, {
            function: "deposit",
        });
    }

    #[test]
    #[should_panic(expected = "contract")]
    fn sub_invocation_diagnostic_formats_full_authorization_tree() {
        let fx = routing_fixture();
        AggregatorContractClient::new(&fx.env, &fx.aggregator)
            .swap(&fx.user, &fx.pool, &fx.vault, &100i128);

        assert_auth_sub_invocation!(fx.env, {
            function: "absent",
        });
    }
}
