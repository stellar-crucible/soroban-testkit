extern crate std;

use soroban_sdk::testutils::Address as _;
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::{Address, Env};
use soroban_testkit_assert::assert_storage_tier_count;
use soroban_testkit_core::budget::BudgetSnapshot;
use soroban_testkit_core::storage::StorageSnapshot;
use soroban_testkit_example_dynamic_amm::{
    DynamicAmmContract, DynamicAmmContractClient, DEFAULT_BASE_FEE_BPS, DEFAULT_MAX_FEE_BPS,
    DEFAULT_MIN_FEE_BPS, DEFAULT_TWAP_WINDOW, DEFAULT_VOLATILITY_FACTOR,
};

const ONE: i128 = 10_000_000;

struct AmmBudgetFixture {
    env: Env,
    pool: Address,
    token_a: Address,
    _token_b: Address,
    lp: Address,
    trader: Address,
}

fn setup() -> AmmBudgetFixture {
    let env = Env::default();
    env.mock_all_auths();

    let pool = env.register(DynamicAmmContract, ());
    let admin = Address::generate(&env);
    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    let lp = Address::generate(&env);
    let trader = Address::generate(&env);

    for user in [&lp, &trader] {
        StellarAssetClient::new(&env, &token_a).mint(user, &(1_000_000 * ONE));
        StellarAssetClient::new(&env, &token_b).mint(user, &(1_000_000 * ONE));
    }

    let client = DynamicAmmContractClient::new(&env, &pool);
    client.initialize(
        &admin,
        &token_a,
        &token_b,
        &DEFAULT_MIN_FEE_BPS,
        &DEFAULT_MAX_FEE_BPS,
        &DEFAULT_BASE_FEE_BPS,
        &DEFAULT_VOLATILITY_FACTOR,
        &DEFAULT_TWAP_WINDOW,
    );

    AmmBudgetFixture {
        env,
        pool,
        token_a,
        _token_b: token_b,
        lp,
        trader,
    }
}

#[test]
fn benchmark_dynamic_fee_swap_vs_constant_fee_swap_overhead() {
    let fx1 = setup();
    let client1 = DynamicAmmContractClient::new(&fx1.env, &fx1.pool);
    client1.deposit(&fx1.lp, &(100_000 * ONE), &(100_000 * ONE), &1);

    // Standard constant-product AMM swap baseline (fixed fee, no TWAP deviation calculation)
    client1.swap_constant_fee(&fx1.trader, &fx1.token_a, &(5_000 * ONE), &1, &30);
    let constant_cost = BudgetSnapshot::last_invocation(&fx1.env);

    let fx2 = setup();
    let client2 = DynamicAmmContractClient::new(&fx2.env, &fx2.pool);
    client2.deposit(&fx2.lp, &(100_000 * ONE), &(100_000 * ONE), &1);

    // Dynamic AMM swap with volatility-adjusted fee & TWAP tracking
    client2.swap(&fx2.trader, &fx2.token_a, &(5_000 * ONE), &1);
    let dynamic_cost = BudgetSnapshot::last_invocation(&fx2.env);

    // Dynamic AMM swap overhead must be strictly bounded (< 30% overhead compared to constant AMM)
    let cpu_overhead_pct = ((dynamic_cost.cpu_insns as f64 - constant_cost.cpu_insns as f64)
        / constant_cost.cpu_insns as f64)
        * 100.0;
    assert!(
        cpu_overhead_pct < 30.0,
        "Dynamic fee CPU overhead was {:.2}%, expected < 30%",
        cpu_overhead_pct
    );
}

#[test]
fn budget_guard_bounds_swap_cpu_and_memory() {
    let fx = setup();
    let client = DynamicAmmContractClient::new(&fx.env, &fx.pool);
    client.deposit(&fx.lp, &(100_000 * ONE), &(100_000 * ONE), &1);

    // Swap must execute well within Soroban transaction resource limits
    soroban_testkit_core::budget_guard!(
        &fx.env,
        "dynamic_swap",
        { cpu_max: 5_000_000, mem_max: 5_000_000 },
        || client.swap(&fx.trader, &fx.token_a, &(2_000 * ONE), &1)
    );
}

#[test]
fn budget_snapshot_measures_deposit_consumption() {
    let fx = setup();
    let client = DynamicAmmContractClient::new(&fx.env, &fx.pool);

    client.deposit(&fx.lp, &(50_000 * ONE), &(50_000 * ONE), &1);
    let initial_cost = BudgetSnapshot::last_invocation(&fx.env);

    // Subsequent deposit with existing reserves
    client.deposit(&fx.lp, &(25_000 * ONE), &(25_000 * ONE), &1);
    let second_cost = BudgetSnapshot::last_invocation(&fx.env);

    assert!(initial_cost.cpu_insns > 0);
    assert!(second_cost.cpu_insns > 0);
}

#[test]
fn storage_snapshot_tracks_pool_state_keys() {
    let fx = setup();
    let client = DynamicAmmContractClient::new(&fx.env, &fx.pool);

    let snapshot_before = StorageSnapshot::capture(&fx.env, &fx.pool);
    client.deposit(&fx.lp, &(100_000 * ONE), &(100_000 * ONE), &1);
    let snapshot_after = StorageSnapshot::capture(&fx.env, &fx.pool);

    let diff = snapshot_before.diff(&snapshot_after);
    // Depositing adds persistent LP share key
    assert!(!diff.added().is_empty());
}

#[test]
fn assert_storage_tier_count_enforces_allocation_quotas() {
    let fx = setup();
    let client = DynamicAmmContractClient::new(&fx.env, &fx.pool);

    client.deposit(&fx.lp, &(100_000 * ONE), &(100_000 * ONE), &1);

    // Pool maintains 4 instance entries (Config, Reserves, TotalShares, Twap)
    // and 1 persistent entry for LP shares
    assert_storage_tier_count!(fx.env, fx.pool, {
        instance: 4,
        persistent: 1,
        temporary: 0,
    });
}

#[test]
fn twap_sync_gas_cost_is_strictly_bounded() {
    let fx = setup();
    let client = DynamicAmmContractClient::new(&fx.env, &fx.pool);
    client.deposit(&fx.lp, &(100_000 * ONE), &(100_000 * ONE), &1);

    client.sync_twap();
    let sync_cost = BudgetSnapshot::last_invocation(&fx.env);

    assert!(
        sync_cost.cpu_insns < 2_000_000,
        "TWAP sync CPU was {}",
        sync_cost.cpu_insns
    );
}

#[test]
fn withdrawal_storage_cleanup() {
    let fx = setup();
    let client = DynamicAmmContractClient::new(&fx.env, &fx.pool);

    let shares = client.deposit(&fx.lp, &(50_000 * ONE), &(50_000 * ONE), &1);
    assert_storage_tier_count!(fx.env, fx.pool, persistent = 1);

    // Full withdrawal cleans up the persistent LP share entry
    client.withdraw(&fx.lp, &shares, &1, &1);
    assert_storage_tier_count!(fx.env, fx.pool, persistent = 0);
}

#[test]
fn consecutive_swaps_budget_scaling_is_constant() {
    let fx = setup();
    let client = DynamicAmmContractClient::new(&fx.env, &fx.pool);
    client.deposit(&fx.lp, &(100_000 * ONE), &(100_000 * ONE), &1);

    client.swap(&fx.trader, &fx.token_a, &(1_000 * ONE), &1);
    let cost_1 = BudgetSnapshot::last_invocation(&fx.env);

    client.swap(&fx.trader, &fx.token_a, &(1_000 * ONE), &1);
    let cost_2 = BudgetSnapshot::last_invocation(&fx.env);

    // Second swap CPU cost should be within 15% of first swap (O(1) complexity)
    let ratio = cost_2.cpu_insns as f64 / cost_1.cpu_insns as f64;
    assert!(
        (0.85..=1.15).contains(&ratio),
        "Consecutive swap cost ratio was {:.3}",
        ratio
    );
}
