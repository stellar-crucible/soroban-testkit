extern crate std;

use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::{Address, String};
use soroban_testkit_assert::events::EventMatcher;
use soroban_testkit_assert::{
    assert_auth_matches, assert_event_count_range, assert_event_topic_count,
};
use soroban_testkit_example_nft::{NftContract, NftContractClient};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

const ONE: i128 = 10_000_000;

struct NftAssertFixture {
    ctx: TestContext,
    nft: Address,
    currency: Address,
    _creator: Address,
    user_1: Address,
    user_2: Address,
    operator: Address,
}

fn setup() -> NftAssertFixture {
    let ctx = TestContextBuilder::new().with_users(4).build();

    let nft = ctx.env.register(NftContract, ());
    let currency = ctx
        .env
        .register_stellar_asset_contract_v2(ctx.admin.clone())
        .address();

    let creator = ctx.user(0).clone();
    let user_1 = ctx.user(1).clone();
    let user_2 = ctx.user(2).clone();
    let operator = ctx.user(3).clone();

    for u in [&creator, &user_1, &user_2, &operator] {
        StellarAssetClient::new(&ctx.env, &currency).mint(u, &(100_000 * ONE));
    }

    let client = NftContractClient::new(&ctx.env, &nft);
    client.initialize(
        &ctx.admin,
        &String::from_str(&ctx.env, "StellarPunks"),
        &String::from_str(&ctx.env, "SPUNK"),
        &creator,
        &500,
    );

    NftAssertFixture {
        ctx,
        nft,
        currency,
        _creator: creator,
        user_1,
        user_2,
        operator,
    }
}

#[test]
fn assert_mint_event_emitted() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);

    client.mint(&fx.user_1, &1, &String::from_str(&fx.ctx.env, "uri-1"));

    EventMatcher::new(&fx.ctx.env)
        .from_contract(&fx.nft)
        .with_topic("mint")
        .assert_emitted();

    assert_event_topic_count!(fx.ctx.env, "mint", 1);
}

#[test]
fn assert_transfer_event_emitted() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);
    client.mint(&fx.user_1, &1, &String::from_str(&fx.ctx.env, "uri-1"));

    client.transfer(&fx.user_1, &fx.user_2, &1);

    EventMatcher::new(&fx.ctx.env)
        .from_contract(&fx.nft)
        .with_topic("transfer")
        .assert_emitted();

    assert_event_topic_count!(fx.ctx.env, "transfer", 1);
}

#[test]
fn assert_approval_events_emitted() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);
    client.mint(&fx.user_1, &1, &String::from_str(&fx.ctx.env, "uri-1"));

    client.approve(&fx.user_1, &fx.operator, &1, &1000);

    EventMatcher::new(&fx.ctx.env)
        .from_contract(&fx.nft)
        .with_topic("approve")
        .assert_emitted();

    assert_event_topic_count!(fx.ctx.env, "approve", 1);

    client.revoke_approval(&fx.user_1, &1);
    assert_event_topic_count!(fx.ctx.env, "revoked", 1);
}

#[test]
fn assert_secondary_sale_and_royalty_events_emitted() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);
    client.mint(&fx.user_1, &1, &String::from_str(&fx.ctx.env, "art"));

    client.secondary_sale(&fx.user_1, &fx.user_2, &1, &fx.currency, &(50 * ONE));

    EventMatcher::new(&fx.ctx.env)
        .from_contract(&fx.nft)
        .with_topic("royalty")
        .assert_emitted();

    EventMatcher::new(&fx.ctx.env)
        .from_contract(&fx.nft)
        .with_topic("sale")
        .assert_emitted();
}

#[test]
fn assert_admin_auth_required_for_mint() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);

    client.mint(&fx.user_1, &1, &String::from_str(&fx.ctx.env, "uri-1"));
    assert_auth_matches!(fx.ctx.env, {
        invoker: fx.ctx.admin,
        function: "mint",
        args: [fx.user_1.clone(), 1u32, String::from_str(&fx.ctx.env, "uri-1")],
    });
}

#[test]
fn assert_owner_auth_required_for_transfer() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);
    client.mint(&fx.user_1, &1, &String::from_str(&fx.ctx.env, "uri-1"));

    client.transfer(&fx.user_1, &fx.user_2, &1);
    assert_auth_matches!(fx.ctx.env, {
        invoker: fx.user_1,
        function: "transfer",
        args: [fx.user_1.clone(), fx.user_2.clone(), 1u32],
    });
}

#[test]
fn assert_buyer_auth_required_for_secondary_sale() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);
    client.mint(&fx.user_1, &1, &String::from_str(&fx.ctx.env, "art"));

    client.secondary_sale(&fx.user_1, &fx.user_2, &1, &fx.currency, &(20 * ONE));
    assert_auth_matches!(fx.ctx.env, {
        invoker: fx.user_2,
        function: "secondary_sale",
        args: [fx.user_1.clone(), fx.user_2.clone(), 1u32, fx.currency.clone(), 20 * ONE],
    });
}

#[test]
fn assert_metadata_update_emits_event_and_requires_admin() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);
    client.mint(&fx.user_1, &1, &String::from_str(&fx.ctx.env, "v1"));

    client.update_metadata(&fx.ctx.admin, &1, &String::from_str(&fx.ctx.env, "v2"));
    assert_auth_matches!(fx.ctx.env, {
        invoker: fx.ctx.admin,
        function: "update_metadata",
        args: [fx.ctx.admin.clone(), 1u32, String::from_str(&fx.ctx.env, "v2")],
    });
    assert_event_topic_count!(fx.ctx.env, "meta_upd", 1);
    assert_event_count_range!(fx.ctx.env, 1..=10);
}
