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
            Some(expected) => {
                expected.args.len() == args.len() as usize
                    && expected
                        .args
                        .iter()
                        .zip(args.iter())
                        .all(|(pattern, actual)| pattern.matches(self.env, &actual))
            }
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
            let patterns = args
                .args
                .iter()
                .map(|arg| match arg {
                    AuthArg::Any => "_".to_string(),
                    AuthArg::Exact(value) => ScVal::try_from_val(self.env, value)
                        .map(|value| data_label(self.env, &value))
                        .unwrap_or_else(|_| format!("{value:?}")),
                })
                .collect::<Vec<_>>();
            parts.push(format!("args [{}]", patterns.join(", ")));
        }
        format!("{{ {} }}", parts.join(", "))
    }
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

/// One recorded authorization as the failure message spells it.
fn describe_auth(env: &Env, invoker: &Address, invocation: &AuthorizedInvocation) -> String {
    let function = match &invocation.function {
        AuthorizedFunction::Contract((contract, name, args)) => format!(
            "contract {contract:?} function \"{}\" args {}",
            name.to_string(),
            data_label(env, &ScVal::from(args)),
        ),
        AuthorizedFunction::CreateContractHostFn(_) => "create_contract host function".to_string(),
        AuthorizedFunction::CreateContractV2HostFn(_) => {
            "create_contract v2 host function".to_string()
        }
    };
    format!("invoker {invoker:?} {function}")
}

#[cfg(test)]
mod tests {
    use super::{AuthArgs, AuthMatcher};
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
}
