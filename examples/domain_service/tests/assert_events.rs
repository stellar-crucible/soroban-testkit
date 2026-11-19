// One crate per file, which is the point of splitting this suite up: this file
// is `soroban-testkit-assert`. It asserts on the events the registry publishes
// and on the authorizations it requires — nothing about budgets or generated
// inputs.
use soroban_sdk::{Address, String};
use soroban_testkit_assert::assert_auth_matches;
use soroban_testkit_assert::auth::AuthMatcher;
use soroban_testkit_assert::events::{EventLog, EventMatcher};
use soroban_testkit_example_domain_service::{
    DomainService, DomainServiceClient, RecordKind, SECONDS_PER_YEAR,
};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

fn registry(ctx: &TestContext) -> (Address, DomainServiceClient<'_>) {
    let contract_id = ctx.env.register(DomainService, ());
    let client = DomainServiceClient::new(&ctx.env, &contract_id);
    client.initialize(&ctx.admin, &ctx.admin, &100, &604_800);
    for user in ctx.users.iter() {
        client.deposit(user, &1_000);
    }
    (contract_id, client)
}

fn name(ctx: &TestContext, text: &str) -> String {
    String::from_str(&ctx.env, text)
}

#[test]
fn a_registration_event_carries_the_name_the_owner_and_the_expiry() {
    let mut ctx = TestContextBuilder::new().with_users(1).build();
    ctx.set_timestamp(1_700_000_000);
    let (contract_id, client) = registry(&ctx);
    let owner = ctx.user(0).clone();
    let label = name(&ctx, "alice");

    client.register(&owner, &label, &1);

    // Three fields, each worth asserting on its own: a registration that names
    // the right owner for the wrong term is still a bug.
    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("registered")
        .assert_data_matches(|data| {
            data.field("name")
                .and_then(|value| value.deserialize::<String>())
                == Some(label.clone())
        });
    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("registered")
        .assert_data_matches(|data| {
            data.field("owner")
                .and_then(|value| value.deserialize::<Address>())
                == Some(owner.clone())
        });
    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("registered")
        .assert_data_matches(|data| {
            data.field("expires_at")
                .and_then(|value| value.deserialize::<u64>())
                == Some(1_700_000_000 + SECONDS_PER_YEAR)
        });
}

#[test]
fn a_record_event_carries_the_kind_and_the_value_it_stored() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (_id, client) = registry(&ctx);
    let owner = ctx.user(0).clone();
    let label = name(&ctx, "alice");
    let value = name(&ctx, "ipfs://bafybeigdyrzt");
    client.register(&owner, &label, &1);

    client.set_record(&owner, &label, &RecordKind::Ipfs, &value);

    EventMatcher::new(&ctx.env)
        .with_topic("record")
        .assert_data_matches(|data| {
            data.field("kind")
                .and_then(|field| field.deserialize::<RecordKind>())
                == Some(RecordKind::Ipfs)
        });
    EventMatcher::new(&ctx.env)
        .with_topic("record")
        .assert_data_matches(|data| {
            data.field("value")
                .and_then(|field| field.deserialize::<String>())
                == Some(value.clone())
        });
}

#[test]
fn a_transfer_event_names_both_parties() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let (_id, client) = registry(&ctx);
    let (sender, recipient) = (ctx.user(0).clone(), ctx.user(1).clone());
    let label = name(&ctx, "alice");
    client.register(&sender, &label, &1);

    client.transfer(&sender, &label, &recipient);

    EventMatcher::new(&ctx.env)
        .with_topic("transferred")
        .assert_data_matches(|data| {
            data.field("from")
                .and_then(|field| field.deserialize::<Address>())
                == Some(sender.clone())
        });
    EventMatcher::new(&ctx.env)
        .with_topic("transferred")
        .assert_data_matches(|data| {
            data.field("to")
                .and_then(|field| field.deserialize::<Address>())
                == Some(recipient.clone())
        });
}

#[test]
fn reading_the_registry_publishes_nothing() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (_id, client) = registry(&ctx);
    let label = name(&ctx, "alice");
    client.register(ctx.user(0), &label, &1);

    client.owner_of(&label);
    client.domain(&label);
    client.is_available(&label);
    client.resolve(&label, &RecordKind::Stellar);

    // None of the reads above published anything, and the matcher's scope is
    // the invocation that just ran.
    EventMatcher::new(&ctx.env).assert_not_emitted();
}

#[test]
fn two_registries_in_one_environment_keep_their_events_apart() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (first_id, first) = registry(&ctx);
    let (second_id, _second) = registry(&ctx);

    first.register(ctx.user(0), &name(&ctx, "alice"), &1);

    EventMatcher::new(&ctx.env)
        .from_contract(&first_id)
        .with_topic("registered")
        .assert_emitted();
    EventMatcher::new(&ctx.env)
        .from_contract(&second_id)
        .with_topic("registered")
        .assert_not_emitted();
}

#[test]
fn a_sequence_of_calls_can_be_asserted_as_one_set() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let (_id, client) = registry(&ctx);
    let (owner, recipient) = (ctx.user(0).clone(), ctx.user(1).clone());
    let label = name(&ctx, "alice");

    let mut log = EventLog::new(&ctx.env);
    client.register(&owner, &label, &1);
    log.collect();
    client.set_record(&owner, &label, &RecordKind::Text, &name(&ctx, "hello"));
    log.collect();
    client.transfer(&owner, &label, &recipient);
    log.collect();

    // A log keeps every collected call, so the count is over the sequence and
    // not only over the latest invocation the env still holds.
    log.matcher().with_topic("registered").assert_count(1);
    log.matcher().with_topic("record").assert_count(1);
    log.matcher().with_topic("transferred").assert_count(1);
    assert_eq!(log.len(), 3);
}

#[test]
fn a_registration_records_the_owners_authorization_with_its_arguments() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (_id, client) = registry(&ctx);
    let owner = ctx.user(0).clone();
    let label = name(&ctx, "alice");

    client.register(&owner, &label, &3);

    // Under the builder's default policy the requirement is recorded rather
    // than enforced, so the matcher reads back who authorized which function
    // with which arguments. The name is left as `_`: a `String` comparison
    // here would need the same env-converted handle.
    assert_auth_matches!(ctx.env, {
        invoker: owner,
        function: "register",
        args: [_, _, 3u32],
    });
}

#[test]
fn the_matcher_counts_one_calls_requirement_and_a_reads_none() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (_id, client) = registry(&ctx);
    let label = name(&ctx, "alice");
    client.register(ctx.user(0), &label, &1);

    AuthMatcher::new(&ctx.env).assert_auth_count(1);

    client.owner_of(&label);

    // A read requires nothing, so the same environment now records nothing at
    // all — the count is per invocation, not per test.
    AuthMatcher::new(&ctx.env).assert_no_auth_required();
}

#[test]
fn a_renewal_event_reports_the_new_end_of_the_term() {
    let mut ctx = TestContextBuilder::new().with_users(1).build();
    ctx.set_timestamp(1_700_000_000);
    let (_id, client) = registry(&ctx);
    let owner = ctx.user(0).clone();
    let label = name(&ctx, "alice");
    client.register(&owner, &label, &1);

    client.renew(&owner, &label, &2);

    // Renewal starts from the end of the term that is still running, so one
    // year plus two more is three years from the clock the test set.
    EventMatcher::new(&ctx.env)
        .with_topic("renewed")
        .assert_data_matches(|data| {
            data.field("expires_at")
                .and_then(|field| field.deserialize::<u64>())
                == Some(1_700_000_000 + 3 * SECONDS_PER_YEAR)
        });
}

#[test]
#[should_panic]
fn a_registration_the_owner_never_authorized_is_refused() {
    let ctx = TestContextBuilder::new()
        .with_users(1)
        .without_mock_auths()
        .build();
    let contract_id = ctx.env.register(DomainService, ());
    let client = DomainServiceClient::new(&ctx.env, &contract_id);

    // No `initialize` here: `register` needs the owner's authorization, so this
    // isolates the refusal of one unapproved call.
    client.register(ctx.user(0), &name(&ctx, "alice"), &1);
}
