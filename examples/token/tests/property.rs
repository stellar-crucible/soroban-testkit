//! One crate per file: this suite drives the token example through
//! `soroban-testkit-generators`, whose job is handing a test inputs no one wrote
//! by hand — amounts from the range a token really trades in, and addresses in
//! both formats Soroban accepts.
//!
//! Each case builds its own `Env`, so a case cannot inherit another case's
//! storage, and every generated address carries the crate's process-wide nonce,
//! so two cases never hold the same holder.

use proptest::prelude::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    xdr::ScAddress,
    Address, Env,
};
use soroban_testkit_example_token::{TokenContract, TokenContractClient};
use soroban_testkit_generators::strategies::{
    account_address, address, contract_address, ledger_sequence, timestamp, token_amount,
};

/// An initialized token in an environment of its own.
fn deploy(env: &Env) -> TokenContractClient<'_> {
    env.mock_all_auths();
    let admin = Address::generate(env);
    let contract = env.register(TokenContract, ());
    let client = TokenContractClient::new(env, &contract);
    client.initialize(&admin);
    client
}

proptest! {
    // A case registers a contract and runs several calls, which is far more
    // work than a case over plain numbers. Fewer cases keeps the file quick
    // enough to run on every `cargo test`.
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn a_mint_adds_its_amount_to_the_supply(amount in token_amount()) {
        let env = Env::default();
        let client = deploy(&env);
        let holder = Address::generate(&env);

        let supply = client.mint(&holder, &amount);

        prop_assert_eq!(supply, amount);
        prop_assert_eq!(client.total_supply(), amount);
        prop_assert_eq!(client.balance_of(&holder), amount);
    }

    #[test]
    fn a_transfer_moves_exactly_what_it_reports(
        held in token_amount(),
        moved in token_amount(),
        sender in account_address(),
        recipient in contract_address(),
    ) {
        prop_assume!(moved <= held);

        let env = Env::default();
        let client = deploy(&env);
        let sender = sender.to_address(&env);
        let recipient = recipient.to_address(&env);

        client.mint(&sender, &held);
        client.transfer(&sender, &recipient, &moved);

        // The recipient is generated as a contract address and the sender as an
        // account: the token treats both as an owner, and the balances say so.
        prop_assert_eq!(client.balance_of(&recipient), moved);
        prop_assert_eq!(client.balance_of(&sender), held - moved);
        prop_assert_eq!(client.total_supply(), held);
    }

    #[test]
    fn a_burn_takes_the_amount_it_reports(
        held in token_amount(),
        burnt in token_amount(),
        holder in account_address(),
    ) {
        prop_assume!(burnt <= held);

        let env = Env::default();
        let client = deploy(&env);
        let holder = holder.to_address(&env);

        client.mint(&holder, &held);
        let supply = client.burn(&holder, &burnt);

        prop_assert_eq!(supply, held - burnt);
        prop_assert_eq!(client.total_supply(), held - burnt);
        prop_assert_eq!(client.balance_of(&holder), held - burnt);
    }

    #[test]
    fn an_allowance_is_drawn_down_by_what_was_spent(
        funded in token_amount(),
        approved in token_amount(),
        spent in token_amount(),
        owner in account_address(),
        spender in contract_address(),
    ) {
        // Two conditions, because a spender needs both permission and funds:
        // the allowance has to cover the amount and the owner's balance has to
        // cover it too.
        prop_assume!(approved >= spent && funded >= spent);

        let env = Env::default();
        let client = deploy(&env);
        let owner = owner.to_address(&env);
        let spender = spender.to_address(&env);
        let destination = Address::generate(&env);

        client.mint(&owner, &funded);
        client.approve(&owner, &spender, &approved);
        client.transfer_from(&spender, &owner, &destination, &spent);

        prop_assert_eq!(client.allowance(&owner, &spender), approved - spent);
        prop_assert_eq!(client.balance_of(&destination), spent);
        prop_assert_eq!(client.balance_of(&owner), funded - spent);
        prop_assert_eq!(client.total_supply(), funded);
    }

    #[test]
    fn the_ledger_clock_changes_no_balance(
        amount in token_amount(),
        sequence in ledger_sequence(),
        when in timestamp(),
    ) {
        // This token has no schedule, expiry or fee that reads the clock, so
        // where the ledger is must not change what a holder owns. A contract
        // that did read time would fail this, which is the point of generating
        // the ledger values rather than pinning them.
        let env = Env::default();
        env.ledger().set_sequence_number(sequence);
        env.ledger().set_timestamp(when);
        let client = deploy(&env);
        let holder = Address::generate(&env);

        client.mint(&holder, &amount);
        env.ledger().set_timestamp(when.saturating_add(1_000));
        env.ledger().set_sequence_number(sequence.saturating_add(1));

        prop_assert_eq!(client.balance_of(&holder), amount);
        prop_assert_eq!(client.total_supply(), amount);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn generated_addresses_never_collide(
        first in address(),
        second in address(),
        third in address(),
    ) {
        // The strategies write a process-wide counter into the last eight bytes,
        // so a collision would take the whole run down rather than shrink to one
        // case.
        prop_assert_ne!(first.as_bytes(), second.as_bytes());
        prop_assert_ne!(second.as_bytes(), third.as_bytes());
        prop_assert_ne!(first.as_bytes(), third.as_bytes());
    }

    #[test]
    fn the_requested_format_is_the_shape_that_arrives(
        account in account_address(),
        contract in contract_address(),
    ) {
        // Both formats are opaque 32 bytes on the wire, so the tag is the only
        // thing separating them, and `to_address` has to honour it.
        prop_assert!(matches!(ScAddress::from(account), ScAddress::Account(_)));
        prop_assert!(matches!(ScAddress::from(contract), ScAddress::Contract(_)));

        let env = Env::default();
        let account = account.to_address(&env);
        let contract = contract.to_address(&env);
        prop_assert_ne!(account, contract);
    }

    #[test]
    fn token_amounts_fit_the_range_a_token_can_hold(amount in token_amount()) {
        // The cap is what lets a property test mint, transfer and burn the same
        // generated number without an i128 overflow.
        prop_assert!(amount >= 0);
        prop_assert!(amount <= 1_000_000_000_000_000);
    }
}
