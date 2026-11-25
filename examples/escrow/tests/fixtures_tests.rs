use soroban_sdk::{
    token::{Client as TokenClient, StellarAssetClient},
    Address, Symbol,
};
use soroban_testkit_example_escrow::{
    EscrowContract, EscrowContractClient, EscrowError, EscrowStatus,
};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

struct EscrowFixture {
    ctx: TestContext,
    client: EscrowContractClient<'static>,
    token: Address,
    buyer: Address,
    seller: Address,
    arbiter: Address,
    attacker: Address,
}

impl EscrowFixture {
    fn new(total_amount: i128, timeout_ledger: u32) -> Self {
        let ctx = TestContextBuilder::new().with_users(4).build();
        let buyer = ctx.user(0).clone();
        let seller = ctx.user(1).clone();
        let arbiter = ctx.user(2).clone();
        let attacker = ctx.user(3).clone();

        let token = ctx
            .env
            .register_stellar_asset_contract_v2(ctx.admin.clone())
            .address();
        StellarAssetClient::new(&ctx.env, &token).mint(&buyer, &(total_amount * 2));

        let contract_id = ctx.env.register(EscrowContract, ());
        let client = EscrowContractClient::new(&ctx.env, &contract_id);

        client.initialize(
            &buyer,
            &seller,
            &arbiter,
            &token,
            &total_amount,
            &timeout_ledger,
        );

        Self {
            ctx,
            client,
            token,
            buyer,
            seller,
            arbiter,
            attacker,
        }
    }
}

#[test]
fn test_fixture_context_users_and_setup() {
    let fix = EscrowFixture::new(10_000, 500);
    let cfg = fix.client.get_config();
    assert_eq!(cfg.buyer, fix.buyer);
    assert_eq!(cfg.seller, fix.seller);
    assert_eq!(cfg.arbiter, fix.arbiter);
    assert_eq!(cfg.total_amount, 10_000);
    assert_eq!(cfg.status, EscrowStatus::Active);
}

#[test]
fn test_buyer_can_raise_dispute() {
    let fix = EscrowFixture::new(10_000, 500);
    fix.client.deposit(&fix.buyer, &10_000);
    fix.client
        .raise_dispute(&fix.buyer, &Symbol::new(&fix.ctx.env, "bad_work"));
    assert_eq!(fix.client.get_status(), EscrowStatus::Disputed);
}

#[test]
fn test_seller_can_raise_dispute() {
    let fix = EscrowFixture::new(10_000, 500);
    fix.client.deposit(&fix.buyer, &10_000);
    fix.client
        .raise_dispute(&fix.seller, &Symbol::new(&fix.ctx.env, "unresponsive"));
    assert_eq!(fix.client.get_status(), EscrowStatus::Disputed);
}

#[test]
fn test_unauthorized_party_cannot_raise_dispute() {
    let fix = EscrowFixture::new(10_000, 500);
    fix.client.deposit(&fix.buyer, &10_000);
    let res = fix
        .client
        .try_raise_dispute(&fix.attacker, &Symbol::new(&fix.ctx.env, "grief"));
    assert_eq!(res, Err(Ok(EscrowError::Unauthorized)));
}

#[test]
fn test_arbiter_resolves_dispute_split() {
    let fix = EscrowFixture::new(10_000, 500);
    fix.client.deposit(&fix.buyer, &10_000);
    fix.client
        .raise_dispute(&fix.buyer, &Symbol::new(&fix.ctx.env, "dispute"));

    // 70% to buyer, 30% to seller
    fix.client.resolve_dispute(&fix.arbiter, &7_000, &3_000);
    assert_eq!(fix.client.get_status(), EscrowStatus::Resolved);

    let token = TokenClient::new(&fix.ctx.env, &fix.token);
    assert_eq!(token.balance(&fix.seller), 3_000);
}

#[test]
fn test_dispute_resolution_payout_mismatch_fails() {
    let fix = EscrowFixture::new(10_000, 500);
    fix.client.deposit(&fix.buyer, &10_000);
    fix.client
        .raise_dispute(&fix.buyer, &Symbol::new(&fix.ctx.env, "dispute"));

    // Sum is 9,000, expected 10,000
    let res = fix.client.try_resolve_dispute(&fix.arbiter, &5_000, &4_000);
    assert_eq!(res, Err(Ok(EscrowError::PayoutMismatch)));
}

#[test]
fn test_unauthorized_cannot_resolve_dispute() {
    let fix = EscrowFixture::new(10_000, 500);
    fix.client.deposit(&fix.buyer, &10_000);
    fix.client
        .raise_dispute(&fix.buyer, &Symbol::new(&fix.ctx.env, "dispute"));

    let res = fix
        .client
        .try_resolve_dispute(&fix.attacker, &5_000, &5_000);
    assert_eq!(res, Err(Ok(EscrowError::Unauthorized)));
}

#[test]
fn test_refund_fails_before_timeout_ledger() {
    let fix = EscrowFixture::new(10_000, 500);
    fix.client.deposit(&fix.buyer, &10_000);

    // Ledger sequence is 0, timeout is 500
    let res = fix.client.try_claim_refund(&fix.buyer);
    assert_eq!(res, Err(Ok(EscrowError::NotExpired)));
}

#[test]
fn test_refund_succeeds_after_ledger_advance() {
    let mut fix = EscrowFixture::new(10_000, 500);
    fix.client.deposit(&fix.buyer, &10_000);

    // Advance ledger past timeout
    fix.ctx.advance_ledger(501);

    let refunded = fix.client.claim_refund(&fix.buyer);
    assert_eq!(refunded, 10_000);
    assert_eq!(fix.client.get_status(), EscrowStatus::Refunded);
    assert_eq!(fix.client.get_remaining_funds(), 0);
}
