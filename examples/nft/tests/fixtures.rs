extern crate std;

use soroban_sdk::testutils::Ledger as _;
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::{Address, String, Vec};
use soroban_testkit_example_nft::{NftContract, NftContractClient};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

const ONE: i128 = 10_000_000;

struct NftFixture {
    ctx: TestContext,
    nft: Address,
    currency: Address,
    creator: Address,
    user_1: Address,
    user_2: Address,
    operator: Address,
}

fn setup() -> NftFixture {
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
        &500, // 5% royalty
    );

    NftFixture {
        ctx,
        nft,
        currency,
        creator,
        user_1,
        user_2,
        operator,
    }
}

#[test]
fn initialize_sets_metadata_and_royalty_terms() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);

    let config = client.get_config();
    assert_eq!(config.admin, fx.ctx.admin);
    assert_eq!(config.name, String::from_str(&fx.ctx.env, "StellarPunks"));
    assert_eq!(config.symbol, String::from_str(&fx.ctx.env, "SPUNK"));
    assert_eq!(config.royalty_recipient, fx.creator);
    assert_eq!(config.royalty_bps, 500);
    assert_eq!(config.total_supply, 0);
}

#[test]
fn initialize_twice_is_rejected() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);
    let err = client.try_initialize(
        &fx.ctx.admin,
        &String::from_str(&fx.ctx.env, "Other"),
        &String::from_str(&fx.ctx.env, "OTH"),
        &fx.creator,
        &250,
    );
    assert!(err.is_err());
}

#[test]
fn mint_assigns_owner_and_metadata_uri() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);

    let uri = String::from_str(
        &fx.ctx.env,
        "ipfs://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
    );
    let token_id = client.mint(&fx.user_1, &1, &uri);
    assert_eq!(token_id, 1);

    assert_eq!(client.owner_of(&1), Some(fx.user_1.clone()));
    assert_eq!(client.balance_of(&fx.user_1), 1);
    assert_eq!(client.total_supply(), 1);
    assert_eq!(client.token_uri(&1), Some(uri));
}

#[test]
fn minting_existing_token_id_fails() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);
    let uri = String::from_str(&fx.ctx.env, "ipfs://item-1");

    client.mint(&fx.user_1, &100, &uri);
    let err = client.try_mint(&fx.user_2, &100, &uri);
    assert!(err.is_err());
}

#[test]
fn batch_minting_mints_multiple_nfts() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);

    let mut token_ids = Vec::new(&fx.ctx.env);
    let mut uris = Vec::new(&fx.ctx.env);

    for i in 1..=5 {
        token_ids.push_back(i);
        uris.push_back(String::from_str(&fx.ctx.env, "ipfs://nft-item"));
    }

    let count = client.batch_mint(&fx.user_1, &token_ids, &uris);
    assert_eq!(count, 5);
    assert_eq!(client.balance_of(&fx.user_1), 5);
    assert_eq!(client.total_supply(), 5);

    for i in 1..=5 {
        assert_eq!(client.owner_of(&i), Some(fx.user_1.clone()));
    }
}

#[test]
fn transfer_moves_nft_ownership_and_updates_balances() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);
    let uri = String::from_str(&fx.ctx.env, "ipfs://item");
    client.mint(&fx.user_1, &1, &uri);

    client.transfer(&fx.user_1, &fx.user_2, &1);

    assert_eq!(client.owner_of(&1), Some(fx.user_2.clone()));
    assert_eq!(client.balance_of(&fx.user_1), 0);
    assert_eq!(client.balance_of(&fx.user_2), 1);
}

#[test]
fn single_token_approval_allows_operator_to_transfer() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);
    let uri = String::from_str(&fx.ctx.env, "ipfs://item");
    client.mint(&fx.user_1, &1, &uri);

    // Approve operator until ledger 1000
    fx.ctx.env.ledger().set_sequence_number(100);
    client.approve(&fx.user_1, &fx.operator, &1, &1000);

    assert_eq!(client.get_approved(&1), Some(fx.operator.clone()));

    // Operator executes transfer
    client.transfer(&fx.operator, &fx.user_2, &1);
    assert_eq!(client.owner_of(&1), Some(fx.user_2.clone()));

    // Approval is cleared on transfer
    assert_eq!(client.get_approved(&1), None);
}

#[test]
fn expired_single_token_approval_is_rejected() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);
    let uri = String::from_str(&fx.ctx.env, "ipfs://item");
    client.mint(&fx.user_1, &1, &uri);

    fx.ctx.env.ledger().set_sequence_number(100);
    client.approve(&fx.user_1, &fx.operator, &1, &200);

    // Fast forward ledger past expiration
    fx.ctx.env.ledger().set_sequence_number(201);
    assert_eq!(client.get_approved(&1), None);

    let err = client.try_transfer(&fx.operator, &fx.user_2, &1);
    assert!(err.is_err());
}

#[test]
fn operator_approval_for_all_allows_multiple_transfers() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);
    client.mint(&fx.user_1, &1, &String::from_str(&fx.ctx.env, "1"));
    client.mint(&fx.user_1, &2, &String::from_str(&fx.ctx.env, "2"));

    fx.ctx.env.ledger().set_sequence_number(10);
    client.set_approval_for_all(&fx.user_1, &fx.operator, &true, &500);

    assert!(client.is_approved_for_all(&fx.user_1, &fx.operator));

    // Operator transfers both tokens
    client.transfer(&fx.operator, &fx.user_2, &1);
    client.transfer(&fx.operator, &fx.user_2, &2);

    assert_eq!(client.owner_of(&1), Some(fx.user_2.clone()));
    assert_eq!(client.owner_of(&2), Some(fx.user_2.clone()));
}

#[test]
fn burning_nft_destroys_token_and_decrements_supply() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);
    client.mint(&fx.user_1, &42, &String::from_str(&fx.ctx.env, "uri-42"));

    assert_eq!(client.total_supply(), 1);
    client.burn(&fx.user_1, &42);

    assert_eq!(client.owner_of(&42), None);
    assert_eq!(client.token_uri(&42), None);
    assert_eq!(client.total_supply(), 0);
    assert_eq!(client.balance_of(&fx.user_1), 0);
}

#[test]
fn secondary_sale_distributes_royalty_and_seller_proceeds() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);
    client.mint(&fx.user_1, &1, &String::from_str(&fx.ctx.env, "art"));

    let sale_price = 100 * ONE;
    let (royalty_recip, expected_royalty) = client.calculate_royalty(&sale_price);
    assert_eq!(royalty_recip, fx.creator);
    assert_eq!(expected_royalty, 5 * ONE); // 5% of 100

    let creator_balance_before =
        StellarAssetClient::new(&fx.ctx.env, &fx.currency).balance(&fx.creator);
    let seller_balance_before =
        StellarAssetClient::new(&fx.ctx.env, &fx.currency).balance(&fx.user_1);
    let buyer_balance_before =
        StellarAssetClient::new(&fx.ctx.env, &fx.currency).balance(&fx.user_2);

    // User 2 buys token 1 from User 1 for 100 currency
    client.secondary_sale(&fx.user_1, &fx.user_2, &1, &fx.currency, &sale_price);

    let creator_balance_after =
        StellarAssetClient::new(&fx.ctx.env, &fx.currency).balance(&fx.creator);
    let seller_balance_after =
        StellarAssetClient::new(&fx.ctx.env, &fx.currency).balance(&fx.user_1);
    let buyer_balance_after =
        StellarAssetClient::new(&fx.ctx.env, &fx.currency).balance(&fx.user_2);

    // Creator receives 5% royalty
    assert_eq!(creator_balance_after - creator_balance_before, 5 * ONE);
    // Seller receives 95% proceeds
    assert_eq!(seller_balance_after - seller_balance_before, 95 * ONE);
    // Buyer paid 100 total
    assert_eq!(buyer_balance_before - buyer_balance_after, 100 * ONE);

    // Ownership moved to buyer
    assert_eq!(client.owner_of(&1), Some(fx.user_2.clone()));
}

#[test]
fn update_metadata_modifies_token_uri() {
    let fx = setup();
    let client = NftContractClient::new(&fx.ctx.env, &fx.nft);
    client.mint(&fx.user_1, &7, &String::from_str(&fx.ctx.env, "old-uri"));

    let new_uri = String::from_str(&fx.ctx.env, "ipfs://new-upgraded-metadata");
    client.update_metadata(&fx.ctx.admin, &7, &new_uri);

    assert_eq!(client.token_uri(&7), Some(new_uri));
}
