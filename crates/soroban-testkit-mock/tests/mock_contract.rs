//! End-to-end tests for `#[mock_contract]`: a contract under test calls a
//! dependency that is replaced by a generated mock.

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, testutils::Address as _, Address, Env,
    Symbol,
};
use soroban_testkit_mock::mock_contract;

// ── The issue's example shape: no `env` parameter ──────────────────────────

#[mock_contract]
pub trait TokenInterface {
    fn transfer(from: Address, to: Address, amount: i128);
    fn balance_of(account: Address) -> i128;
}

// ── The `#[contractclient]` shape: `env` first, so the mock implements the
//    trait and a contract can call it through the generated client ─────────

#[mock_contract]
#[contractclient(name = "OracleClient")]
pub trait Oracle {
    fn price(env: Env, asset: Symbol) -> i128;
    fn report(env: Env, asset: Symbol, price: i128);
    fn checked_price(env: Env, asset: Symbol) -> Result<i128, OracleError>;
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum OracleError {
    Stale = 1,
}

/// The contract under test: values a holding with an oracle price and pays
/// out through a token.
#[contract]
pub struct Vault;

#[contractimpl]
impl Vault {
    pub fn value(env: Env, oracle: Address, asset: Symbol, units: i128) -> i128 {
        OracleClient::new(&env, &oracle).price(&asset) * units
    }

    pub fn refresh(env: Env, oracle: Address, asset: Symbol, price: i128) {
        OracleClient::new(&env, &oracle).report(&asset, &price);
    }

    pub fn safe_value(env: Env, oracle: Address, asset: Symbol, units: i128) -> i128 {
        match OracleClient::new(&env, &oracle).try_checked_price(&asset) {
            Ok(Ok(price)) => price * units,
            _ => 0,
        }
    }

    pub fn pay_if_solvent(
        env: Env,
        token: Address,
        from: Address,
        to: Address,
        amount: i128,
    ) -> bool {
        let client = MockTokenInterfaceContractClient::new(&env, &token);
        if client.balance_of(&from) >= amount {
            client.transfer(&from, &to, &amount);
            true
        } else {
            false
        }
    }
}

fn vault(env: &Env) -> VaultClient<'_> {
    let id = env.register(Vault, ());
    VaultClient::new(env, &id)
}

#[test]
fn returning_configures_a_return_value_per_function() {
    let env = Env::default();
    let mock = MockTokenInterface::new(&env);
    mock.expect_balance_of().returning(|_| 1000);
    let client = MockTokenInterfaceContractClient::new(&env, &mock.address());
    assert_eq!(client.balance_of(&Address::generate(&env)), 1000);
}

#[test]
fn a_contract_calls_the_mocked_dependency() {
    let env = Env::default();
    let token = MockTokenInterface::new(&env);
    token.expect_balance_of().returns(500);
    token.expect_transfer().once();

    let (from, to) = (Address::generate(&env), Address::generate(&env));
    let paid = vault(&env).pay_if_solvent(&token.address(), &from, &to, &200);

    assert!(paid);
    assert_eq!(token.balance_of_calls(), vec![(from.clone(),)]);
    assert_eq!(token.transfer_calls(), vec![(from, to, 200)]);
    token.verify();
}

#[test]
fn an_insolvent_payer_never_reaches_transfer() {
    let env = Env::default();
    let token = MockTokenInterface::new(&env);
    token.expect_balance_of().returns(10);
    token.expect_transfer().never();

    let paid = vault(&env).pay_if_solvent(
        &token.address(),
        &Address::generate(&env),
        &Address::generate(&env),
        &200,
    );
    assert!(!paid);
    token.verify();
}

#[test]
fn call_history_is_typed_and_ordered() {
    let env = Env::default();
    let token = MockTokenInterface::new(&env);
    token.expect_balance_of().returns(1_000);
    let (from, to) = (Address::generate(&env), Address::generate(&env));
    vault(&env).pay_if_solvent(&token.address(), &from, &to, &1);

    let calls = token.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].function(), "balance_of");
    assert_eq!(
        calls[1],
        MockTokenInterfaceCall::Transfer {
            from,
            to,
            amount: 1
        }
    );
    assert_eq!(token.call_count(), 2);
}

#[test]
fn the_mock_implements_the_client_trait() {
    let env = Env::default();
    let oracle = MockOracle::new(&env);
    let xlm = Symbol::new(&env, "XLM");
    oracle.expect_price().with(xlm.clone()).returns(12);

    assert_eq!(vault(&env).value(&oracle.address(), &xlm, &10), 120);
    assert_eq!(oracle.price_calls(), vec![(xlm,)]);
}

#[test]
fn argument_matchers_select_the_expectation() {
    let env = Env::default();
    let oracle = MockOracle::new(&env);
    let (xlm, usdc) = (Symbol::new(&env, "XLM"), Symbol::new(&env, "USDC"));
    oracle.expect_price().with(xlm.clone()).returns(12);
    oracle.expect_price().with(usdc.clone()).returns(100);

    let vault = vault(&env);
    assert_eq!(vault.value(&oracle.address(), &usdc, &2), 200);
    assert_eq!(vault.value(&oracle.address(), &xlm, &2), 24);
}

#[test]
fn withf_and_returning_see_the_arguments() {
    let env = Env::default();
    let oracle = MockOracle::new(&env);
    oracle.expect_report().withf(|_, price| *price > 0).times(2);
    oracle.expect_report().withf(|_, price| *price <= 0).never();

    let vault = vault(&env);
    let xlm = Symbol::new(&env, "XLM");
    vault.refresh(&oracle.address(), &xlm, &5);
    vault.refresh(&oracle.address(), &xlm, &6);
    oracle.verify();
    assert_eq!(oracle.report_calls().len(), 2);
}

#[test]
fn result_returning_functions_can_return_errors() {
    let env = Env::default();
    let oracle = MockOracle::new(&env);
    let xlm = Symbol::new(&env, "XLM");
    oracle
        .expect_checked_price()
        .returns(Err(OracleError::Stale));
    assert_eq!(vault(&env).safe_value(&oracle.address(), &xlm, &3), 0);

    oracle.reset();
    oracle.expect_checked_price().returns(Ok(7));
    assert_eq!(vault(&env).safe_value(&oracle.address(), &xlm, &3), 21);
}

#[test]
fn times_limits_saturate_into_the_next_expectation() {
    let env = Env::default();
    let oracle = MockOracle::new(&env);
    oracle.expect_price().times(1).returns(1);
    oracle.expect_price().returns(2);
    let vault = vault(&env);
    let xlm = Symbol::new(&env, "XLM");
    assert_eq!(vault.value(&oracle.address(), &xlm, &1), 1);
    assert_eq!(vault.value(&oracle.address(), &xlm, &1), 2);
    assert_eq!(vault.value(&oracle.address(), &xlm, &1), 2);
    oracle.verify();
}

#[test]
#[should_panic(expected = "expected 2 call(s), got 1")]
fn verify_reports_unmet_call_counts() {
    let env = Env::default();
    let oracle = MockOracle::new(&env);
    oracle.expect_price().times(2).returns(1);
    vault(&env).value(&oracle.address(), &Symbol::new(&env, "XLM"), &1);
    oracle.verify();
}

#[test]
fn a_call_without_a_matching_expectation_fails_the_invocation() {
    let env = Env::default();
    let oracle = MockOracle::new(&env);
    let result = vault(&env).try_value(&oracle.address(), &Symbol::new(&env, "XLM"), &1);
    assert!(result.is_err());
}

#[test]
fn checkpoint_verifies_then_clears() {
    let env = Env::default();
    let oracle = MockOracle::new(&env);
    oracle.expect_price().once().returns(3);
    vault(&env).value(&oracle.address(), &Symbol::new(&env, "XLM"), &1);
    oracle.checkpoint();
    assert_eq!(oracle.call_count(), 0);
}

#[test]
fn two_mocks_of_the_same_trait_are_independent() {
    let env = Env::default();
    let first = MockOracle::new(&env);
    let second = MockOracle::new(&env);
    first.expect_price().returns(1);
    second.expect_price().returns(2);
    let vault = vault(&env);
    let xlm = Symbol::new(&env, "XLM");
    assert_eq!(vault.value(&first.address(), &xlm, &1), 1);
    assert_eq!(vault.value(&second.address(), &xlm, &1), 2);
    assert_eq!(first.call_count(), 1);
    assert_eq!(second.call_count(), 1);
}
