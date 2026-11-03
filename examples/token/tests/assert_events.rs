// One crate per file, which is the point of splitting this suite up: this file
// is `soroban-testkit-assert`. It asserts on the events the token publishes and
// on the authorizations it requires — nothing about budgets or generated inputs.
use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};
use soroban_sdk::{Address, IntoVal};
use soroban_testkit_assert::assert_auth_matches;
use soroban_testkit_assert::auth::AuthMatcher;
use soroban_testkit_assert::events::{EventLog, EventMatcher};
use soroban_testkit_example_token::{TokenContract, TokenContractClient};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

fn token(ctx: &TestContext) -> (Address, TokenContractClient<'_>) {
    let contract_id = ctx.env.register(TokenContract, ());
    let client = TokenContractClient::new(&ctx.env, &contract_id);
    client.initialize(&ctx.admin);
    (contract_id, client)
}

#[test]
fn a_transfer_event_carries_the_amount_and_both_parties_in_its_data() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let (contract_id, client) = token(&ctx);
    let (sender, recipient) = (ctx.user(0).clone(), ctx.user(1).clone());
    client.mint(&sender, &1_000);

    client.transfer(&sender, &recipient, &250);

    // The counter's event holds one number; a token's holds three, and each is
    // worth asserting on its own — a transfer that moves the right amount to the
    // wrong address is still a bug.
    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("transfer")
        .assert_data_matches(|data| {
            data.field("amount")
                .and_then(|value| value.deserialize::<i128>())
                == Some(250)
        });
    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("transfer")
        .assert_data_matches(|data| {
            data.field("from")
                .and_then(|value| value.deserialize::<Address>())
                == Some(sender.clone())
        });
    EventMatcher::new(&ctx.env)
        .with_topic("transfer")
        .assert_data_matches(|data| {
            data.field("to")
                .and_then(|value| value.deserialize::<Address>())
                == Some(recipient.clone())
        });
}

#[test]
fn the_transfer_payload_names_every_field_it_carries() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let client = token(&ctx).1;
    client.mint(ctx.user(0), &1_000);
    client.transfer(ctx.user(0), ctx.user(1), &250);

    // `fields()` enumerates what is actually in the map, so a test can pin the
    // shape of an event as well as its values. Ledger order is the XDR
    // canonicalisation of the keys, so the comparison sorts before it checks.
    EventMatcher::new(&ctx.env)
        .with_topic("transfer")
        .assert_data_matches(|data| {
            let mut names = data.fields();
            names.sort();
            names == ["amount", "from", "to"]
        });
}

#[test]
fn a_mint_payload_omits_the_field_that_has_no_value() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let client = token(&ctx).1;

    client.mint(ctx.user(0), &500);

    // `Minted` has no `from`, and the SDK publishes event maps sparse: the
    // absent field is missing from the map rather than stored as void, so
    // `field("from")` reads None here and Some in the transfer above.
    EventMatcher::new(&ctx.env)
        .with_topic("mint")
        .assert_data_matches(|data| {
            data.field("from").is_none()
                && data
                    .field("amount")
                    .and_then(|value| value.deserialize::<i128>())
                    == Some(500)
        });
}

#[test]
fn reading_the_token_publishes_nothing() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let client = token(&ctx).1;

    client.total_supply();
    client.balance_of(ctx.user(0));
    client.allowance(ctx.user(0), ctx.user(0));

    EventMatcher::new(&ctx.env).assert_not_emitted();
}

#[test]
fn two_tokens_in_one_environment_keep_their_events_apart() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (first_id, first) = token(&ctx);
    let (second_id, _second) = token(&ctx);

    first.mint(ctx.user(0), &10);

    // The second registration is the point of this test: a matcher scoped to a
    // contract that published nothing stays silent while the one that did
    // publish answers, and nothing about the fixture changed between them.
    EventMatcher::new(&ctx.env)
        .from_contract(&first_id)
        .with_topic("mint")
        .assert_emitted();
    EventMatcher::new(&ctx.env)
        .from_contract(&second_id)
        .with_topic("mint")
        .assert_not_emitted();
}

#[test]
fn a_sequence_of_calls_can_be_asserted_as_one_set() {
    let ctx = TestContextBuilder::new().with_users(3).build();
    let client = token(&ctx).1;
    let (sender, recipient, spender) = (ctx.user(0), ctx.user(1), ctx.user(2));

    let mut log = EventLog::new(&ctx.env);
    client.mint(sender, &1_000);
    log.collect();
    client.transfer(sender, recipient, &250);
    log.collect();
    client.approve(sender, spender, &100);
    log.collect();

    // The log keeps every collected call, so a count is over the sequence and
    // not only over the last invocation the env still holds.
    log.matcher().with_topic("transfer").assert_count(1);
    log.matcher().with_topic("mint").assert_count(1);
    assert_eq!(log.len(), 3);

    let names: Vec<String> = log
        .topics()
        .iter()
        .map(|topics| topics[1].clone())
        .collect();
    assert_eq!(names, ["mint", "transfer", "approve"]);
}

#[test]
fn transfer_from_draws_the_allowance_it_says_it_did() {
    let ctx = TestContextBuilder::new().with_users(3).build();
    let client = token(&ctx).1;
    let (owner, spender, recipient) = (
        ctx.user(0).clone(),
        ctx.user(1).clone(),
        ctx.user(2).clone(),
    );

    client.mint(&owner, &1_000);
    client.approve(&owner, &spender, &400);
    client.transfer_from(&spender, &owner, &recipient, &400);

    // The spend itself is a transfer, so the same topic carries it, and the
    // `Approval` two calls earlier is out of the matcher's scope: only the
    // latest invocation is in view unless the test collects a log.
    EventMatcher::new(&ctx.env)
        .with_topic("transfer")
        .assert_data_matches(|data| {
            data.field("from")
                .and_then(|value| value.deserialize::<Address>())
                == Some(owner.clone())
        });
    EventMatcher::new(&ctx.env)
        .with_topic("approve")
        .assert_not_emitted();

    // Read back after the assertions: one more call is one more invocation, and
    // the invocation the matcher was looking at is gone with it.
    assert_eq!(client.allowance(&owner, &spender), 0);
    assert_eq!(client.balance_of(&recipient), 400);
}

#[test]
fn a_transfer_records_the_holders_authorization_with_its_arguments() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let client = token(&ctx).1;
    let (sender, recipient) = (ctx.user(0).clone(), ctx.user(1).clone());

    client.mint(&sender, &1_000);
    client.transfer(&sender, &recipient, &250);

    // Under the builder's default policy the requirement is recorded rather
    // than enforced, so the matcher can read back who authorized which function
    // with which arguments.
    assert_auth_matches!(ctx.env, {
        invoker: sender,
        function: "transfer",
        args: [_, recipient, 250i128],
    });
}

#[test]
fn the_matcher_counts_one_calls_requirement_and_a_reads_none() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let client = token(&ctx).1;
    let (sender, recipient) = (ctx.user(0).clone(), ctx.user(1).clone());

    client.mint(&sender, &1_000);
    client.transfer(&sender, &recipient, &250);

    // The matcher also answers questions the macro does not ask: how many
    // authorizations this one call needed, with no filter applied to the count.
    AuthMatcher::new(&ctx.env).assert_auth_count(1);

    client.balance_of(&sender);

    // A read requires nothing, so the same environment now records nothing at
    // all — the count is per invocation, not per test.
    AuthMatcher::new(&ctx.env).assert_no_auth_required();
}

#[test]
fn an_authorization_supplied_by_the_test_instead_of_the_harness_still_matches() {
    let ctx = TestContextBuilder::new()
        .with_users(1)
        .without_mock_auths()
        .build();
    let contract_id = ctx.env.register(TokenContract, ());
    let client = TokenContractClient::new(&ctx.env, &contract_id);
    let admin = ctx.admin.clone();

    ctx.env.mock_auths(&[MockAuth {
        address: &admin,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "initialize",
            args: (admin.clone(),).into_val(&ctx.env),
            sub_invokes: &[],
        },
    }]);
    client.initialize(&admin);

    // The authorization is asserted before the state is read back, for the same
    // reason as above: `env.auths()` belongs to the latest invocation, and
    // `client.admin()` would become the latest one.
    assert_auth_matches!(ctx.env, {
        invoker: admin,
        function: "initialize",
    });
    assert_eq!(client.admin(), admin.clone());
}

#[test]
#[should_panic]
fn a_call_the_owner_never_authorized_is_refused() {
    let ctx = TestContextBuilder::new()
        .with_users(2)
        .without_mock_auths()
        .build();
    let contract_id = ctx.env.register(TokenContract, ());
    let client = TokenContractClient::new(&ctx.env, &contract_id);

    // No `initialize` here: `approve` needs only the owner's authorization, so
    // this isolates the refusal of one unapproved call.
    client.approve(ctx.user(0), ctx.user(1), &1);
}
