//! One crate per file: this suite drives `examples/vesting_escrow` through
//! `soroban-testkit-fixtures`, verifying schedule creation, vesting curves,
//! claims, and revocation.

use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::Address;
use soroban_testkit_example_vesting_escrow::{VestingEscrow, VestingEscrowClient, VestingSchedule};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

struct Deployed<'a> {
    contract_id: Address,
    client: VestingEscrowClient<'a>,
    token: Address,
    admin: Address,
    beneficiary: Address,
}

fn deploy(ctx: &TestContext) -> Deployed<'_> {
    let contract_id = ctx.env.register(VestingEscrow, ());
    let client = VestingEscrowClient::new(&ctx.env, &contract_id);

    let admin = ctx.admin.clone();
    let beneficiary = ctx.user(0).clone();

    let token_admin = Address::generate(&ctx.env);
    let token_id = ctx
        .env
        .register_stellar_asset_contract_v2(token_admin.clone())
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
fn create_schedule_locks_tokens_and_returns_sequential_ids() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let d = deploy(&ctx);
    ctx.env.ledger().set_timestamp(1_700_000_000);

    let id0 = d.client.create_schedule(
        &d.admin,
        &d.beneficiary,
        &d.token,
        &100_000,
        &1_700_000_000,
        &0,
        &100,
        &true,
    );
    let id1 = d.client.create_schedule(
        &d.admin,
        &d.beneficiary,
        &d.token,
        &50_000,
        &1_700_000_000,
        &0,
        &50,
        &false,
    );

    assert_eq!(id0, 0);
    assert_eq!(id1, 1);
    assert_eq!(d.client.schedule_count(), 2);

    let asset = soroban_sdk::token::Client::new(&ctx.env, &d.token);
    assert_eq!(asset.balance(&d.contract_id), 150_000);
    assert_eq!(asset.balance(&d.admin), 850_000);
}

#[test]
fn nothing_is_vested_before_the_cliff() {
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

    ctx.env.ledger().set_timestamp(1_700_000_005);
    assert_eq!(d.client.vested(&0), 0);
    assert_eq!(d.client.claimable(&0), 0);
    assert_eq!(d.client.unvested(&0), 100_000);
}

#[test]
fn cliff_boundary_unlocks_exactly_the_linear_amount() {
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

    // one second before the cliff: nothing
    ctx.env.ledger().set_timestamp(1_700_000_009);
    assert_eq!(d.client.vested(&0), 0);

    // exactly at the cliff: 10% of the grant
    ctx.env.ledger().set_timestamp(1_700_000_010);
    assert_eq!(d.client.vested(&0), 10_000);
    assert_eq!(d.client.claimable(&0), 10_000);
}

#[test]
fn linear_unlock_progresses_through_quarters() {
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

    for (elapsed, expected) in [
        (25u64, 25_000i128),
        (50, 50_000),
        (75, 75_000),
        (100, 100_000),
    ] {
        ctx.env.ledger().set_timestamp(1_700_000_000 + elapsed);
        assert_eq!(d.client.vested(&0), expected, "at elapsed {elapsed}");
    }

    // beyond duration clamps to the full grant
    ctx.env.ledger().set_timestamp(1_700_000_000 + 500);
    assert_eq!(d.client.vested(&0), 100_000);
}

#[test]
fn claim_pays_only_the_vested_portion() {
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
    let paid = d.client.claim(&0);
    assert_eq!(paid, 25_000);
    assert_eq!(d.client.claimed(&0), 25_000);
    assert_eq!(d.client.claimable(&0), 0);

    let asset = soroban_sdk::token::Client::new(&ctx.env, &d.token);
    assert_eq!(asset.balance(&d.beneficiary), 25_000);
    assert_eq!(asset.balance(&d.contract_id), 75_000);
}

#[test]
fn multiple_claims_accrue_without_double_paying() {
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
    assert_eq!(d.client.claim(&0), 25_000);
    ctx.env.ledger().set_timestamp(1_700_000_060);
    assert_eq!(d.client.claim(&0), 35_000);
    assert_eq!(d.client.claimed(&0), 60_000);
    assert_eq!(d.client.claimable(&0), 0);
}

#[test]
fn revocation_returns_only_unvested_tokens_and_freezes_the_claimable_cap() {
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
    assert_eq!(d.client.claim(&0), 40_000);

    ctx.env.ledger().set_timestamp(1_700_000_040);
    let returned = d.client.revoke(&0);
    assert_eq!(returned, 60_000);

    let asset = soroban_sdk::token::Client::new(&ctx.env, &d.token);
    assert_eq!(asset.balance(&d.admin), 1_000_000 - 100_000 + 60_000);
    assert_eq!(asset.balance(&d.contract_id), 0);

    // Cap frozen at 40_000 vested: nothing more vests even as time advances.
    ctx.env.ledger().set_timestamp(1_700_000_099);
    assert_eq!(d.client.vested(&0), 40_000);
    assert_eq!(d.client.claimable(&0), 0);
    assert_eq!(d.client.unvested(&0), 0);
    assert_eq!(d.client.returned(&0), 60_000);
}

#[test]
fn non_revocable_schedule_cannot_be_revoked() {
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
        &false,
    );

    ctx.env.ledger().set_timestamp(1_700_000_010);
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        d.client.revoke(&0);
    }));
    assert!(res.is_err());
}

#[test]
fn fully_vested_schedule_has_nothing_to_revoke() {
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

    ctx.env.ledger().set_timestamp(1_700_000_000 + 200);
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        d.client.revoke(&0);
    }));
    assert!(res.is_err());
}

#[test]
fn schedule_getter_round_trips_the_stored_record() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let d = deploy(&ctx);
    ctx.env.ledger().set_timestamp(1_700_000_000);

    d.client.create_schedule(
        &d.admin,
        &d.beneficiary,
        &d.token,
        &100_000,
        &1_700_000_000,
        &5,
        &100,
        &true,
    );

    let sched: VestingSchedule = d.client.schedule(&0);
    assert_eq!(sched.admin, d.admin);
    assert_eq!(sched.beneficiary, d.beneficiary);
    assert_eq!(sched.token, d.token);
    assert_eq!(sched.total_amount, 100_000);
    assert_eq!(sched.cliff_duration, 5);
    assert_eq!(sched.duration, 100);
    assert!(sched.revocable);
}
