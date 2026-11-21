extern crate std;

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env, String};
use soroban_testkit_assert::assert_storage_tier_count;
use soroban_testkit_core::budget::BudgetSnapshot;
use soroban_testkit_core::storage::StorageSnapshot;
use soroban_testkit_example_nft::{NftContract, NftContractClient};

struct NftBudgetFixture {
    env: Env,
    nft: Address,
    _admin: Address,
    recipient: Address,
}

fn setup() -> NftBudgetFixture {
    let env = Env::default();
    env.mock_all_auths();

    let nft = env.register(NftContract, ());
    let admin = Address::generate(&env);
    let creator = Address::generate(&env);
    let recipient = Address::generate(&env);

    let client = NftContractClient::new(&env, &nft);
    client.initialize(
        &admin,
        &String::from_str(&env, "BenchmarkNFT"),
        &String::from_str(&env, "BNFT"),
        &creator,
        &500,
    );

    NftBudgetFixture {
        env,
        nft,
        _admin: admin,
        recipient,
    }
}

#[test]
fn storage_inspection_verifies_persistent_storage_entries_per_token_id() {
    let fx = setup();
    let client = NftContractClient::new(&fx.env, &fx.nft);

    let base_storage = StorageSnapshot::capture(&fx.env, &fx.nft);
    // Initially just Config = 1 entry
    assert_eq!(base_storage.len(), 1);

    // Mint token 1: adds Owner(1), TokenUri(1), and Balance(recipient) -> 3 new entries
    client.mint(&fx.recipient, &1, &String::from_str(&fx.env, "uri-1"));
    let storage_1 = StorageSnapshot::capture(&fx.env, &fx.nft);
    assert_eq!(storage_1.len(), 4);

    // Mint token 2: adds Owner(2), TokenUri(2) -> 2 new entries (Balance already exists, updated)
    client.mint(&fx.recipient, &2, &String::from_str(&fx.env, "uri-2"));
    let storage_2 = StorageSnapshot::capture(&fx.env, &fx.nft);
    assert_eq!(storage_2.len(), 6);

    // Mint token 3: adds Owner(3), TokenUri(3) -> 2 new entries
    client.mint(&fx.recipient, &3, &String::from_str(&fx.env, "uri-3"));
    let storage_3 = StorageSnapshot::capture(&fx.env, &fx.nft);
    assert_eq!(storage_3.len(), 8);

    // Assert exact persistent key tier count
    assert_storage_tier_count!(fx.env, fx.nft, persistent = 8, temporary = 0);
}

#[test]
fn benchmark_storage_growth_across_1_10_and_100_minted_tokens() {
    // 1 token benchmark
    let fx1 = setup();
    let client1 = NftContractClient::new(&fx1.env, &fx1.nft);
    client1.mint(&fx1.recipient, &1, &String::from_str(&fx1.env, "uri"));
    let storage_1 = StorageSnapshot::capture(&fx1.env, &fx1.nft);

    // 10 tokens benchmark
    let fx10 = setup();
    let client10 = NftContractClient::new(&fx10.env, &fx10.nft);
    for i in 1..=10 {
        client10.mint(&fx10.recipient, &i, &String::from_str(&fx10.env, "uri"));
    }
    let storage_10 = StorageSnapshot::capture(&fx10.env, &fx10.nft);

    // 100 tokens benchmark
    let fx100 = setup();
    let client100 = NftContractClient::new(&fx100.env, &fx100.nft);
    for i in 1..=100 {
        client100.mint(&fx100.recipient, &i, &String::from_str(&fx100.env, "uri"));
    }
    let storage_100 = StorageSnapshot::capture(&fx100.env, &fx100.nft);

    // Storage keys must grow linearly: 1 < 10 < 100
    assert!(
        storage_1.len() < storage_10.len(),
        "10 tokens storage keys ({}) should exceed 1 token ({})",
        storage_10.len(),
        storage_1.len()
    );
    assert!(
        storage_10.len() < storage_100.len(),
        "100 tokens storage keys ({}) should exceed 10 tokens ({})",
        storage_100.len(),
        storage_10.len()
    );

    // 1 token = 1 config + 1 balance + 1 owner + 1 uri = 4
    assert_eq!(storage_1.len(), 4);
    // 10 tokens = 1 config + 1 balance + 10 owners + 10 uris = 22
    assert_eq!(storage_10.len(), 22);
    // 100 tokens = 1 config + 1 balance + 100 owners + 100 uris = 202
    assert_eq!(storage_100.len(), 202);
}

#[test]
fn mint_and_transfer_consume_bounded_cpu_and_memory() {
    let fx = setup();
    let client = NftContractClient::new(&fx.env, &fx.nft);

    client.mint(&fx.recipient, &1, &String::from_str(&fx.env, "uri-1"));
    let mint_cost = BudgetSnapshot::last_invocation(&fx.env);

    assert!(mint_cost.cpu_insns > 0);
    assert!(mint_cost.mem_bytes > 0);
    assert!(mint_cost.cpu_insns < 2_000_000);

    let other = Address::generate(&fx.env);
    client.transfer(&fx.recipient, &other, &1);
    let transfer_cost = BudgetSnapshot::last_invocation(&fx.env);

    assert!(transfer_cost.cpu_insns > 0);
    assert!(transfer_cost.mem_bytes > 0);
    assert!(transfer_cost.cpu_insns < 2_000_000);
}

#[test]
fn burning_cleans_up_persistent_storage() {
    let fx = setup();
    let client = NftContractClient::new(&fx.env, &fx.nft);

    let pre_mint_storage = StorageSnapshot::capture(&fx.env, &fx.nft);
    client.mint(&fx.recipient, &99, &String::from_str(&fx.env, "uri-99"));

    let post_mint_storage = StorageSnapshot::capture(&fx.env, &fx.nft);
    assert!(post_mint_storage.len() > pre_mint_storage.len());

    client.burn(&fx.recipient, &99);
    let post_burn_storage = StorageSnapshot::capture(&fx.env, &fx.nft);

    // Owner and TokenUri removed, balance remains 0
    assert!(post_burn_storage.len() < post_mint_storage.len());
}
