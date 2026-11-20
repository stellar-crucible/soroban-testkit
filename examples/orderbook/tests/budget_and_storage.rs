extern crate std;

use soroban_sdk::testutils::Address as _;
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::{Address, Env};
use soroban_testkit_assert::assert_storage_tier_count;
use soroban_testkit_core::budget::BudgetSnapshot;
use soroban_testkit_core::storage::StorageSnapshot;
use soroban_testkit_example_orderbook::{
    OrderBookContract, OrderBookContractClient, Side, PRICE_PRECISION,
};

const ONE: i128 = 10_000_000;

struct OrderBookBudgetFixture {
    env: Env,
    book: Address,
    _base_token: Address,
    _quote_token: Address,
    maker: Address,
    taker: Address,
}

fn setup() -> OrderBookBudgetFixture {
    let env = Env::default();
    env.mock_all_auths();

    let book = env.register(OrderBookContract, ());
    let admin = Address::generate(&env);
    let base_token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let quote_token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    let maker = Address::generate(&env);
    let taker = Address::generate(&env);

    for user in [&maker, &taker] {
        StellarAssetClient::new(&env, &base_token).mint(user, &(100_000_000 * ONE));
        StellarAssetClient::new(&env, &quote_token).mint(user, &(100_000_000 * ONE));
    }

    let client = OrderBookContractClient::new(&env, &book);
    client.initialize(&admin, &base_token, &quote_token, &10, &20);

    OrderBookBudgetFixture {
        env,
        book,
        _base_token: base_token,
        _quote_token: quote_token,
        maker,
        taker,
    }
}

#[test]
fn storage_diff_proves_order_deletion_upon_full_fill() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.env, &fx.book);

    let initial_storage = StorageSnapshot::capture(&fx.env, &fx.book);

    // Place an ask order
    let order_id = client.place_order(&fx.maker, &Side::Sell, &(2 * PRICE_PRECISION), &(10 * ONE));
    let placed_storage = StorageSnapshot::capture(&fx.env, &fx.book);

    // Storage must increase after placing order (order entry added)
    assert!(
        placed_storage.len() > initial_storage.len(),
        "Expected persistent storage count to increase after placing order"
    );

    // Taker buys entire order (10 * ONE)
    client.execute_market_order(&fx.taker, &Side::Buy, &(10 * ONE));
    let filled_storage = StorageSnapshot::capture(&fx.env, &fx.book);

    // Storage diff proves the order was deleted upon full fill
    assert_eq!(
        filled_storage.len(),
        initial_storage.len(),
        "Order entry must be fully removed from storage upon 100% fill"
    );
    assert!(client.get_order(&order_id).is_none());
}

#[test]
fn storage_diff_proves_order_deletion_upon_cancellation() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.env, &fx.book);

    let baseline_storage = StorageSnapshot::capture(&fx.env, &fx.book);
    let order_id = client.place_order(&fx.maker, &Side::Buy, &(2 * PRICE_PRECISION), &(10 * ONE));

    assert!(StorageSnapshot::capture(&fx.env, &fx.book).len() > baseline_storage.len());

    client.cancel_order(&fx.maker, &order_id);

    let after_cancel_storage = StorageSnapshot::capture(&fx.env, &fx.book);
    assert_eq!(
        after_cancel_storage.len(),
        baseline_storage.len(),
        "Cancelled order entry must be removed from persistent storage"
    );
}

#[test]
fn benchmark_cpu_instructions_across_matching_depths_1_5_10() {
    // Benchmark matching depth 1
    let fx1 = setup();
    let client1 = OrderBookContractClient::new(&fx1.env, &fx1.book);
    client1.place_order(&fx1.maker, &Side::Sell, &(2 * PRICE_PRECISION), &(10 * ONE));

    client1.execute_market_order(&fx1.taker, &Side::Buy, &(10 * ONE));
    let depth_1_cost = BudgetSnapshot::last_invocation(&fx1.env);

    // Benchmark matching depth 5
    let fx5 = setup();
    let client5 = OrderBookContractClient::new(&fx5.env, &fx5.book);
    for i in 1..=5 {
        client5.place_order(
            &fx5.maker,
            &Side::Sell,
            &((20 + i) * PRICE_PRECISION / 10),
            &(2 * ONE),
        );
    }
    client5.execute_market_order(&fx5.taker, &Side::Buy, &(10 * ONE));
    let depth_5_cost = BudgetSnapshot::last_invocation(&fx5.env);

    // Benchmark matching depth 10
    let fx10 = setup();
    let client10 = OrderBookContractClient::new(&fx10.env, &fx10.book);
    for i in 1..=10 {
        client10.place_order(
            &fx10.maker,
            &Side::Sell,
            &((20 + i) * PRICE_PRECISION / 10),
            &ONE,
        );
    }
    client10.execute_market_order(&fx10.taker, &Side::Buy, &(10 * ONE));
    let depth_10_cost = BudgetSnapshot::last_invocation(&fx10.env);

    // Assert CPU instruction scaling is monotonic with depth: Depth 1 < Depth 5 < Depth 10
    assert!(
        depth_1_cost.cpu_insns < depth_5_cost.cpu_insns,
        "Depth 5 CPU instructions ({}) should exceed Depth 1 ({})",
        depth_5_cost.cpu_insns,
        depth_1_cost.cpu_insns
    );
    assert!(
        depth_5_cost.cpu_insns < depth_10_cost.cpu_insns,
        "Depth 10 CPU instructions ({}) should exceed Depth 5 ({})",
        depth_10_cost.cpu_insns,
        depth_5_cost.cpu_insns
    );

    // Assert memory scaling is monotonic
    assert!(
        depth_1_cost.mem_bytes <= depth_5_cost.mem_bytes,
        "Depth 5 memory ({}) should be at least Depth 1 ({})",
        depth_5_cost.mem_bytes,
        depth_1_cost.mem_bytes
    );
    assert!(
        depth_5_cost.mem_bytes <= depth_10_cost.mem_bytes,
        "Depth 10 memory ({}) should be at least Depth 5 ({})",
        depth_10_cost.mem_bytes,
        depth_5_cost.mem_bytes
    );
}

#[test]
fn orderbook_storage_tier_count_is_bounded() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.env, &fx.book);

    for _ in 0..5 {
        client.place_order(&fx.maker, &Side::Buy, &(2 * PRICE_PRECISION), &ONE);
        client.place_order(&fx.maker, &Side::Sell, &(3 * PRICE_PRECISION), &ONE);
    }

    // Config + Bids + Asks + 10 Orders = 13 persistent entries
    assert_storage_tier_count!(fx.env, fx.book, persistent = 13, temporary = 0);
}

#[test]
fn place_order_consumes_bounded_budget() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.env, &fx.book);

    client.place_order(&fx.maker, &Side::Buy, &(2 * PRICE_PRECISION), &(10 * ONE));
    let cost = BudgetSnapshot::last_invocation(&fx.env);

    assert!(cost.cpu_insns > 0);
    assert!(cost.mem_bytes > 0);
    // Placing one order should comfortably execute well within Soroban transaction limits
    assert!(cost.cpu_insns < 5_000_000);
}
