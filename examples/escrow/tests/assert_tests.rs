extern crate std;

use soroban_sdk::{
    token::{Client as TokenClient, StellarAssetClient},
    Address, Symbol,
};
use soroban_testkit_assert::events::EventMatcher;
use soroban_testkit_example_escrow::{EscrowContract, EscrowContractClient, EscrowStatus};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

struct AssertFixture {
    ctx: TestContext,
    contract_id: Address,
    client: EscrowContractClient<'static>,
    token: Address,
    buyer: Address,
    seller: Address,
    arbiter: Address,
}

impl AssertFixture {
    fn new(total_amount: i128) -> Self {
        let ctx = TestContextBuilder::new().with_users(3).build();
        let buyer = ctx.user(0).clone();
        let seller = ctx.user(1).clone();
        let arbiter = ctx.user(2).clone();

        let token = ctx
            .env
            .register_stellar_asset_contract_v2(ctx.admin.clone())
            .address();
        StellarAssetClient::new(&ctx.env, &token).mint(&buyer, &(total_amount * 3));

        let contract_id = ctx.env.register(EscrowContract, ());
        let client = EscrowContractClient::new(&ctx.env, &contract_id);

        Self {
            ctx,
            contract_id,
            client,
            token,
            buyer,
            seller,
            arbiter,
        }
    }
}

#[test]
fn test_assert_initialization_event() {
    let fix = AssertFixture::new(5_000);
    fix.client.initialize(
        &fix.buyer,
        &fix.seller,
        &fix.arbiter,
        &fix.token,
        &5_000,
        &200,
    );

    EventMatcher::new(&fix.ctx.env)
        .from_contract(&fix.contract_id)
        .with_topic("initialized")
        .assert_emitted();
}

#[test]
fn test_assert_deposit_event() {
    let fix = AssertFixture::new(5_000);
    fix.client.initialize(
        &fix.buyer,
        &fix.seller,
        &fix.arbiter,
        &fix.token,
        &5_000,
        &200,
    );
    fix.client.deposit(&fix.buyer, &2_500);

    EventMatcher::new(&fix.ctx.env)
        .from_contract(&fix.contract_id)
        .with_topic("deposited")
        .assert_emitted();
}

#[test]
fn test_assert_milestone_approval_event() {
    let fix = AssertFixture::new(5_000);
    fix.client.initialize(
        &fix.buyer,
        &fix.seller,
        &fix.arbiter,
        &fix.token,
        &5_000,
        &200,
    );
    fix.client.add_milestone(
        &fix.buyer,
        &1,
        &Symbol::new(&fix.ctx.env, "phase_1"),
        &2_500,
    );
    fix.client.deposit(&fix.buyer, &5_000);

    fix.client.approve_milestone(&fix.buyer, &1);

    EventMatcher::new(&fix.ctx.env)
        .from_contract(&fix.contract_id)
        .with_topic("milestone_approved")
        .assert_emitted();
}

#[test]
fn test_assert_dispute_event() {
    let fix = AssertFixture::new(5_000);
    fix.client.initialize(
        &fix.buyer,
        &fix.seller,
        &fix.arbiter,
        &fix.token,
        &5_000,
        &200,
    );
    fix.client.deposit(&fix.buyer, &5_000);

    fix.client
        .raise_dispute(&fix.buyer, &Symbol::new(&fix.ctx.env, "defect"));

    EventMatcher::new(&fix.ctx.env)
        .from_contract(&fix.contract_id)
        .with_topic("disputed")
        .assert_emitted();
    assert_eq!(fix.client.get_status(), EscrowStatus::Disputed);
}

#[test]
fn test_assert_dispute_resolved_event() {
    let fix = AssertFixture::new(5_000);
    fix.client.initialize(
        &fix.buyer,
        &fix.seller,
        &fix.arbiter,
        &fix.token,
        &5_000,
        &200,
    );
    fix.client.deposit(&fix.buyer, &5_000);
    fix.client
        .raise_dispute(&fix.buyer, &Symbol::new(&fix.ctx.env, "defect"));

    fix.client.resolve_dispute(&fix.arbiter, &2_000, &3_000);

    EventMatcher::new(&fix.ctx.env)
        .from_contract(&fix.contract_id)
        .with_topic("resolved")
        .assert_emitted();
    assert_eq!(fix.client.get_status(), EscrowStatus::Resolved);
}

#[test]
fn test_assert_balance_conservation_after_payout() {
    let fix = AssertFixture::new(10_000);
    let token_client = TokenClient::new(&fix.ctx.env, &fix.token);

    let initial_buyer = token_client.balance(&fix.buyer);
    let initial_seller = token_client.balance(&fix.seller);

    fix.client.initialize(
        &fix.buyer,
        &fix.seller,
        &fix.arbiter,
        &fix.token,
        &10_000,
        &500,
    );
    fix.client.deposit(&fix.buyer, &10_000);

    fix.client
        .add_milestone(&fix.buyer, &1, &Symbol::new(&fix.ctx.env, "m1"), &4_000);
    fix.client.approve_milestone(&fix.buyer, &1);

    fix.client
        .raise_dispute(&fix.buyer, &Symbol::new(&fix.ctx.env, "dispute"));
    fix.client.resolve_dispute(&fix.arbiter, &3_000, &3_000);

    let final_buyer = token_client.balance(&fix.buyer);
    let final_seller = token_client.balance(&fix.seller);
    let contract_balance = token_client.balance(&fix.contract_id);

    // Initial total money must equal final total money
    assert_eq!(
        initial_buyer + initial_seller,
        final_buyer + final_seller + contract_balance
    );
    assert_eq!(contract_balance, 0);
    assert_eq!(final_seller, 7_000); // 4_000 from milestone + 3_000 dispute
    assert_eq!(final_buyer, initial_buyer - 7_000);
}

#[test]
fn test_assert_refund_conservation_on_timeout() {
    let mut fix = AssertFixture::new(10_000);
    let token_client = TokenClient::new(&fix.ctx.env, &fix.token);
    let initial_buyer = token_client.balance(&fix.buyer);

    fix.client.initialize(
        &fix.buyer,
        &fix.seller,
        &fix.arbiter,
        &fix.token,
        &10_000,
        &100,
    );
    fix.client.deposit(&fix.buyer, &10_000);

    fix.client
        .add_milestone(&fix.buyer, &1, &Symbol::new(&fix.ctx.env, "m1"), &3_000);
    fix.client.approve_milestone(&fix.buyer, &1);

    fix.ctx.advance_ledger(150);

    let refunded = fix.client.claim_refund(&fix.buyer);
    assert_eq!(refunded, 7_000);

    let final_buyer = token_client.balance(&fix.buyer);
    let final_seller = token_client.balance(&fix.seller);
    assert_eq!(final_seller, 3_000);
    assert_eq!(final_buyer, initial_buyer - 3_000);
    assert_eq!(token_client.balance(&fix.contract_id), 0);
}
