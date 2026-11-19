//! One crate per file: this suite drives the registry through
//! `soroban-testkit-generators`, whose job is handing a test inputs no one wrote
//! by hand — names the registry has to judge, terms of every allowed length, and
//! addresses in both formats Soroban accepts.
//!
//! Each case builds its own `Env`, so a case cannot inherit another case's
//! storage or ledger clock.

use proptest::prelude::*;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, Env, String};
use soroban_testkit_example_domain_service::{
    DomainService, DomainServiceClient, RecordKind, SECONDS_PER_YEAR,
};
use soroban_testkit_generators::strategies::{
    account_address, contract_address, ledger_sequence, timestamp,
};

const GRACE: u64 = 604_800;
const BASE_FEE: i128 = 100;

/// A registry with mocked auth and an operator that is also the treasury, so a
/// case only has to think about the owner it generates.
fn deployed(env: &Env) -> DomainServiceClient<'_> {
    env.mock_all_auths();
    let admin = Address::generate(env);
    let contract = env.register(DomainService, ());
    let client = DomainServiceClient::new(env, &contract);
    client.initialize(&admin, &admin, &BASE_FEE, &GRACE);
    client
}

fn name(env: &Env, text: &str) -> String {
    String::from_str(env, text)
}

/// A name that satisfies every rule the registry enforces: three to twelve
/// characters, lower-case letters and digits, hyphen allowed inside, and an
/// alphanumeric at each edge.
fn valid_name() -> impl Strategy<Value = std::string::String> {
    "[a-z][a-z0-9-]{1,10}[a-z0-9]"
}

proptest! {
    // A case registers a contract and runs several calls, which is far more
    // work than a case over plain numbers. Fewer cases keeps the file quick
    // enough to run on every `cargo test`.
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn an_expired_name_can_be_claimed_by_a_new_owner_after_the_grace_period(
        years in 1u32..=5,
        suffix in "[a-z]{2,6}",
    ) {
        let env = Env::default();
        let client = deployed(&env);
        let previous = Address::generate(&env);
        let claimant = Address::generate(&env);
        let label = name(&env, &format!("domain-{suffix}"));
        client.deposit(&previous, &10_000);
        client.deposit(&claimant, &10_000);

        client.register(&previous, &label, &years);
        let expires_at = client.expires_at(&label).expect("a registered name has an expiry");

        // One second past the term plus the grace period is the first moment
        // the name belongs to nobody, and the claim has to work from there.
        env.ledger().set_timestamp(expires_at + GRACE + 1);
        prop_assert!(client.is_available(&label));

        let claimed = client.claim(&claimant, &label);
        prop_assert_eq!(claimed.owner.clone(), claimant.clone());
        prop_assert_eq!(claimed.expires_at, env.ledger().timestamp() + SECONDS_PER_YEAR);
        prop_assert_eq!(client.owner_of(&label), Some(claimant));
    }

    #[test]
    fn a_name_inside_its_grace_period_keeps_its_owner(
        years in 1u32..=5,
        held in 0u64..GRACE,
    ) {
        let env = Env::default();
        let client = deployed(&env);
        let previous = Address::generate(&env);
        let label = name(&env, "alice");
        client.deposit(&previous, &10_000);
        client.register(&previous, &label, &years);

        let expires_at = client.expires_at(&label).unwrap();
        env.ledger().set_timestamp(expires_at + held);

        // The term has run out, but the previous owner still holds the name and
        // nobody else can take it.
        prop_assert!(!client.is_available(&label));
        prop_assert_eq!(client.owner_of(&label), Some(previous));
    }

    #[test]
    fn a_renewal_extends_from_the_end_of_the_running_term(
        years in 1u32..=5,
        extra in 1u32..=5,
        when in timestamp(),
    ) {
        let env = Env::default();
        env.ledger().set_timestamp(when);
        let client = deployed(&env);
        let owner = Address::generate(&env);
        let label = name(&env, "alice");
        client.deposit(&owner, &10_000);
        client.register(&owner, &label, &years);

        client.renew(&owner, &label, &extra);

        // Renewing early must not shorten the term: the new end is the old end
        // plus the years just bought, not now plus them.
        prop_assert_eq!(
            client.expires_at(&label),
            Some(when + (years + extra) as u64 * SECONDS_PER_YEAR)
        );
    }

    #[test]
    fn a_transfer_moves_ownership_and_keeps_the_term(
        owner in account_address(),
        recipient in contract_address(),
    ) {
        let env = Env::default();
        let client = deployed(&env);
        let owner = owner.to_address(&env);
        let recipient = recipient.to_address(&env);
        let label = name(&env, "alice");
        client.deposit(&owner, &10_000);
        client.register(&owner, &label, &2);

        let expires_at = client.expires_at(&label).unwrap();
        client.transfer(&owner, &label, &recipient);

        // The recipient is generated as a contract address and the owner as an
        // account: the registry treats both as a holder, and the term does not
        // move with the name.
        prop_assert_eq!(client.owner_of(&label), Some(recipient));
        prop_assert_eq!(client.expires_at(&label), Some(expires_at));
    }

    #[test]
    fn records_stop_resolving_when_the_term_expires(
        kind in prop_oneof![
            Just(RecordKind::Stellar),
            Just(RecordKind::Ipfs),
            Just(RecordKind::Text),
        ],
        value in "[a-z0-9/:.]{3,32}",
    ) {
        let env = Env::default();
        let client = deployed(&env);
        let owner = Address::generate(&env);
        let label = name(&env, "alice");
        let stored = name(&env, &value);
        client.deposit(&owner, &10_000);
        client.register(&owner, &label, &1);
        client.set_record(&owner, &label, &kind, &stored);

        // While the term runs, the record answers exactly what was written.
        prop_assert_eq!(client.resolve(&label, &kind), Some(stored.clone()));

        let expires_at = client.expires_at(&label).unwrap();
        env.ledger().set_timestamp(expires_at + 1);

        // After it, the record is still in storage but resolution stops: a name
        // that no longer belongs to anyone must not keep pointing at its old
        // owner's payment address.
        prop_assert_eq!(client.resolve(&label, &kind), None);
    }

    #[test]
    fn every_generated_valid_name_is_accepted(text in valid_name()) {
        let env = Env::default();
        let client = deployed(&env);
        let owner = Address::generate(&env);
        client.deposit(&owner, &10_000);
        let label = name(&env, &text);

        prop_assert!(client.is_valid_name(&label));

        // A name the view accepts is a name `register` has to accept too: a
        // validator that disagrees with the writer is the bug this covers.
        let domain = client.register(&owner, &label, &1);
        prop_assert_eq!(domain.owner, owner.clone());
        prop_assert_eq!(client.owner_of(&label), Some(owner));
    }

    #[test]
    fn generated_names_outside_the_rules_are_refused(text in prop_oneof![
        "[A-Z][a-z0-9]{2,8}",
        "[a-z]{0,2}",
        "[a-z0-9]{2,8}[!@#]",
        "-[a-z0-9]{2,8}",
    ]) {
        let env = Env::default();
        let client = deployed(&env);
        let label = name(&env, &text);

        prop_assert!(!client.is_valid_name(&label));
    }

    #[test]
    fn the_price_scales_with_the_term(years in 1u32..=10) {
        let env = Env::default();
        let client = deployed(&env);

        prop_assert_eq!(client.price(&years), BASE_FEE * years as i128);
    }

    #[test]
    fn the_ledger_clock_does_not_reassign_ownership(
        sequence in ledger_sequence(),
        when in timestamp(),
    ) {
        let env = Env::default();
        env.ledger().set_sequence_number(sequence);
        env.ledger().set_timestamp(when);
        let client = deployed(&env);
        let owner = Address::generate(&env);
        let label = name(&env, "alice");
        client.deposit(&owner, &10_000);
        client.register(&owner, &label, &1);

        // A ledger close is not a term change: the sequence number moving on
        // does not expire a name, and the seconds the case advanced are far
        // short of the term it bought.
        env.ledger().set_sequence_number(sequence.saturating_add(1));
        env.ledger().set_timestamp(when.saturating_add(60));

        prop_assert_eq!(client.owner_of(&label), Some(owner));
        prop_assert!(!client.is_available(&label));
    }
}
