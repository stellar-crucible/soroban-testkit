//! Authorization distinctness assertions.

use soroban_sdk::Address;

/// Verifies that an invocation was authorized by *only* the expected address
/// and no other unauthorized addresses or co-signers.
pub fn assert_auth_invoker_distinct_internal(
    expected_addr: &Address,
    authorizations: &[Address],
) {
    for auth in authorizations {
        if auth != expected_addr {
            panic!(
                "Security diagnostic: Unauthorized co-signer or unintended delegation detected. Expected only authorization from {:?}, but found authorization from {:?}",
                expected_addr, auth
            );
        }
    }
}

/// Assert that a function invocation or block was authorized exclusively by
    /// the declared authorized address and no others.
///
/// # Examples
///
/// ```ignore
/// assert_auth_invoker_distinct!(&admin_address, &[client.auth_list()]);
/// ```
#[macro_export]
macro_rules! assert_auth_invoker_distinct {
    ($expected:expr, $auths:expr $(,)?) => {{
        $crate::auth::assert_auth_invoker_distinct_internal($expected, $auths);
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::Env;

    #[test]
    fn test_distinct_auth_matching() {
        let env = Env::default();
        let addr1 = Address::generate(&env);

        // Should not panic
        assert_auth_invoker_distinct_internal(&addr1, &[addr1.clone()]);
        assert_auth_invoker_distinct_internal(&addr1, &[]);
    }

    #[test]
    #[should_panic(expected = "Security diagnostic: Unauthorized co-signer")]
    fn test_distinct_auth_breach() {
        let env = Env::default();
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);

        assert_auth_invoker_distinct_internal(&addr1, &[addr1.clone(), addr2]);
    }
}
