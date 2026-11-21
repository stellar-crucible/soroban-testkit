//! One crate per file: this suite drives `examples/vesting_escrow` through
//! `soroban-testkit-assert`, verifying event payloads and authorizations.

use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::Address;
use soroban_testkit_assert::assert_auth_matches;
use soroban_testkit_assert::events::{EventLog, EventMatcher};
use soroban_testkit_example_vesting_escrow::{VestingEscrow, VestingEscrowClient};
use soroban_testkit_fixtures::builder::TestContextBuilder;

struct Deployed<'a> {
    contract_id: Address,
    client: VestingEscrowClient<'a>,
    token: Address,
    admin: Address,
    beneficiary: Address,
}

fn deploy(ctx: &soroban_testkit_fixtures::TestContext) -> Deployed<'_> {
    let contract_id = ctx.env.register(VestingEscrow, ());
    let client = VestingEscrowClient::new(&ctx.env, &contract_id);

    let admin = ctx.admin.clone();
    let beneficiary = ctx.user(0).clone();

    let token_admin = Address::generate(&ctx.env);
    let token_id = ctx
        .env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    let asset = StellarAssetClient::new(&ctx.env, &token_id);
    asset.mint(&admin, &1_000_000);

    Deployed {
        contract_id,
        client,
        token: token_id,
        admin,
        beneficiary,
    }
}

#[test]
fn create_event_carries_schedule_parameters() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let d = deploy(&ctx);
    ctx.env.ledger().set_timestamp(1_700_000_000);

    d.client.create_schedule(
        &d.admin,
        &d.beneficiary,
        &d.token,
        &100_000,
        &1_700_000_000,
        &10,
        &100,
        &true,
    );

    EventMatcher::new(&ctx.env)
        .from_contract(&d.contract_id)
        .with_topic("created")
        .assert_data_matches(|data| {
            data.field("total_amount")
                .and_then(|v| v.deserialize::<i128>())
                == Some(100_000)
                && data
                    .field("cliff_duration")
                    .and_then(|v| v.deserialize::<u64>())
                    == Some(10)
                && data.field("duration").and_then(|v| v.deserialize::<u64>()) == Some(100)
                && data
                    .field("revocable")
                    .and_then(|v| v.deserialize::<bool>())
                    == Some(true)
        });
}

#[test]
fn claim_event_records_amount_and_running_total() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let d = deploy(&ctx);
    ctx.env.ledger().set_timestamp(1_700_000_000);

    d.client.create_schedule(
        &d.admin,
        &d.beneficiary,
        &d.token,
        &100_000,
        &1_700_000_000,
        &0,
        &100,
        &true,
    );

    ctx.env.ledger().set_timestamp(1_700_000_025);
    d.client.claim(&0);

    EventMatcher::new(&ctx.env)
        .from_contract(&d.contract_id)
        .with_topic("claimed")
        .assert_data_matches(|data| {
            data.field("amount").and_then(|v| v.deserialize::<i128>()) == Some(25_000)
                && data
                    .field("total_claimed")
                    .and_then(|v| v.deserialize::<i128>())
                    == Some(25_000)
        });
}

#[test]
fn revoke_event_records_returned_amount() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let d = deploy(&ctx);
    ctx.env.ledger().set_timestamp(1_700_000_000);

    d.client.create_schedule(
        &d.admin,
        &d.beneficiary,
        &d.token,
        &100_000,
        &1_700_000_000,
        &0,
        &100,
        &true,
    );

    ctx.env.ledger().set_timestamp(1_700_000_040);
    d.client.claim(&0);
    d.client.revoke(&0);

    EventMatcher::new(&ctx.env)
        .from_contract(&d.contract_id)
        .with_topic("revoked")
        .assert_data_matches(|data| {
            data.field("returned").and_then(|v| v.deserialize::<i128>()) == Some(60_000)
                && data
                    .field("vested_at_revocation")
                    .and_then(|v| v.deserialize::<i128>())
                    == Some(40_000)
        });
}

#[test]
fn lifecycle_events_collect_in_order_with_event_log() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let d = deploy(&ctx);
    ctx.env.ledger().set_timestamp(1_700_000_000);

    d.client.create_schedule(
        &d.admin,
        &d.beneficiary,
        &d.token,
        &100_000,
        &1_700_000_000,
        &0,
        &100,
        &true,
    );

    let mut log = EventLog::new(&ctx.env);
    ctx.env.ledger().set_timestamp(1_700_000_025);
    d.client.claim(&0);
    log.collect();
    ctx.env.ledger().set_timestamp(1_700_000_040);
    d.client.revoke(&0);
    log.collect();

    log.matcher()
        .from_contract(&d.contract_id)
        .with_topic("claimed")
        .assert_count(1);
    log.matcher()
        .from_contract(&d.contract_id)
        .with_topic("revoked")
        .assert_count(1);
}

#[test]
fn create_requires_admin_authorization() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let d = deploy(&ctx);
    ctx.env.ledger().set_timestamp(1_700_000_000);

    d.client.create_schedule(
        &d.admin,
        &d.beneficiary,
        &d.token,
        &100_000,
        &1_700_000_000,
        &0,
        &100,
        &true,
    );

    assert_auth_matches!(ctx.env, {
        invoker: d.admin.clone(),
        function: "create_schedule",
    });
}

#[test]
fn claim_requires_beneficiary_authorization() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let d = deploy(&ctx);
    ctx.env.ledger().set_timestamp(1_700_000_000);

    d.client.create_schedule(
        &d.admin,
        &d.beneficiary,
        &d.token,
        &100_000,
        &1_700_000_000,
        &0,
        &100,
        &true,
    );

    ctx.env.ledger().set_timestamp(1_700_000_025);
    d.client.claim(&0);

    assert_auth_matches!(ctx.env, {
        invoker: d.beneficiary.clone(),
        function: "claim",
    });
}

#[test]
fn revoke_requires_admin_authorization() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let d = deploy(&ctx);
    ctx.env.ledger().set_timestamp(1_700_000_000);

    d.client.create_schedule(
        &d.admin,
        &d.beneficiary,
        &d.token,
        &100_000,
        &1_700_000_000,
        &0,
        &100,
        &true,
    );

    ctx.env.ledger().set_timestamp(1_700_000_040);
    d.client.claim(&0);
    d.client.revoke(&0);

    assert_auth_matches!(ctx.env, {
        invoker: d.admin.clone(),
        function: "revoke",
    });
}

#[test]
fn read_only_queries_emit_no_events() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let d = deploy(&ctx);
    ctx.env.ledger().set_timestamp(1_700_000_000);

    d.client.create_schedule(
        &d.admin,
        &d.beneficiary,
        &d.token,
        &100_000,
        &1_700_000_000,
        &0,
        &100,
        &true,
    );

    d.client.vested(&0);
    d.client.claimable(&0);
    d.client.unvested(&0);
    d.client.schedule(&0);

    EventMatcher::new(&ctx.env).assert_not_emitted();
}
