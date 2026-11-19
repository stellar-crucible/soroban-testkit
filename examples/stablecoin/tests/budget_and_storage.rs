//! One crate per file: this suite drives `examples/stablecoin` through
//! `soroban-testkit-core`, benchmarking mint and liquidation resource overhead
//! against committed budget baselines and verifying that instance, persistent,
//! and temporary storage keys land in their intended tiers.
//!
//! The harness builds bare `Env` instances rather than `TestContext` so this
//! file depends only on `soroban-testkit-core`.

use std::path::{Path, PathBuf};

use soroban_sdk::{testutils::Address as _, Address, Env, Symbol};
use soroban_testkit_core::budget::{BudgetBaseline, BudgetMetric, BudgetSnapshot};
use soroban_testkit_core::storage::{inspect_storage, StorageSnapshot, StorageTier};
use soroban_testkit_example_stablecoin::{
    PriceOracleContract, PriceOracleContractClient, StablecoinContract, StablecoinContractClient,
    DEFAULT_MCR_BPS, PRICE_SCALE,
};

struct BareProtocol {
    env: Env,
    oracle_id: Address,
    stable_id: Address,
}

fn deploy_unregistered() -> BareProtocol {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);

    let oracle_id = env.register(PriceOracleContract, ());
    PriceOracleContractClient::new(&env, &oracle_id).initialize(&admin);

    let stable_id = env.register(StablecoinContract, ());
    StablecoinContractClient::new(&env, &stable_id).initialize(
        &admin,
        &oracle_id,
        &DEFAULT_MCR_BPS,
    );

    BareProtocol {
        env,
        oracle_id,
        stable_id,
    }
}

fn deploy_ready() -> BareProtocol {
    let proto = deploy_unregistered();
    let xlm = Symbol::new(&proto.env, "XLM");
    let wbtc = Symbol::new(&proto.env, "WBTC");

    let oracle = PriceOracleContractClient::new(&proto.env, &proto.oracle_id);
    oracle.set_price(&xlm, &PRICE_SCALE);
    oracle.set_price(&wbtc, &(50_000 * PRICE_SCALE));

    let stable = StablecoinContractClient::new(&proto.env, &proto.stable_id);
    stable.register_collateral(&xlm, &15_000);
    stable.register_collateral(&wbtc, &16_000);
    proto
}

fn stable<'a>(proto: &'a BareProtocol) -> StablecoinContractClient<'a> {
    StablecoinContractClient::new(&proto.env, &proto.stable_id)
}

fn oracle<'a>(proto: &'a BareProtocol) -> PriceOracleContractClient<'a> {
    PriceOracleContractClient::new(&proto.env, &proto.oracle_id)
}

/// Prepares a protocol state with an underwater XLM vault and a funded
/// Stability Pool so the next call can measure `liquidate` on its own.
fn setup_liquidation() -> (BareProtocol, Address, Address, Symbol) {
    let proto = deploy_ready();
    let borrower = Address::generate(&proto.env);
    let staker = Address::generate(&proto.env);
    let liquidator = Address::generate(&proto.env);
    let xlm = Symbol::new(&proto.env, "XLM");
    let wbtc = Symbol::new(&proto.env, "WBTC");

    let sc = stable(&proto);
    sc.deposit_collateral(&borrower, &xlm, &1_500);
    sc.mint(&borrower, &xlm, &1_000);

    sc.deposit_collateral(&staker, &wbtc, &1);
    sc.mint(&staker, &wbtc, &1_200);
    sc.stake_stability_pool(&staker, &1_200);

    oracle(&proto).set_price(&xlm, &(8 * PRICE_SCALE / 10));
    (proto, liquidator, borrower, xlm)
}

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("budget.json")
}

#[test]
fn mint_stays_inside_its_transaction_ceiling() {
    let proto = deploy_ready();
    let borrower = Address::generate(&proto.env);
    let xlm = Symbol::new(&proto.env, "XLM");
    stable(&proto).deposit_collateral(&borrower, &xlm, &3_000);

    soroban_testkit_core::budget_guard!(
        &proto.env,
        "mint",
        { cpu_max: 50_000_000, mem_max: 50_000_000 },
        || stable(&proto).mint(&borrower, &xlm, &1_000)
    );
}

#[test]
fn liquidation_stays_inside_its_transaction_ceiling() {
    let (proto, liquidator, borrower, xlm) = setup_liquidation();

    soroban_testkit_core::budget_guard!(
        &proto.env,
        "liquidate",
        { cpu_max: 50_000_000, mem_max: 50_000_000 },
        || stable(&proto).liquidate(&liquidator, &borrower, &xlm)
    );
}

#[test]
#[should_panic(expected = "BUDGET kind=ceiling case=mint metric=cpu_insns")]
fn a_mint_past_its_ceiling_fails_with_parseable_budget_line() {
    let proto = deploy_ready();
    let borrower = Address::generate(&proto.env);
    let xlm = Symbol::new(&proto.env, "XLM");
    stable(&proto).deposit_collateral(&borrower, &xlm, &3_000);

    soroban_testkit_core::budget_guard!(&proto.env, "mint", { cpu_max: 1 }, || {
        stable(&proto).mint(&borrower, &xlm, &1_000)
    });
}

#[test]
fn read_costs_less_cpu_than_mint_and_liquidation() {
    let (proto, liquidator, borrower, xlm) = setup_liquidation();

    stable(&proto).liquidate(&liquidator, &borrower, &xlm);
    let liquidated = BudgetSnapshot::last_invocation(&proto.env);

    stable(&proto).total_supply();
    let read = BudgetSnapshot::last_invocation(&proto.env);

    assert!(
        read.cpu_insns < liquidated.cpu_insns,
        "a supply read should cost less CPU than a liquidation: {} vs {}",
        read.cpu_insns,
        liquidated.cpu_insns,
    );
    assert!(read.diff(&liquidated).cpu_insns > 0);
}

#[test]
fn recorded_mint_and_liquidation_baselines_accept_same_cost_and_flag_growth() {
    let proto = deploy_ready();
    let borrower = Address::generate(&proto.env);
    let xlm = Symbol::new(&proto.env, "XLM");
    stable(&proto).deposit_collateral(&borrower, &xlm, &6_000);

    stable(&proto).mint(&borrower, &xlm, &1_000);
    let measured_mint = BudgetSnapshot::last_invocation(&proto.env);

    let (liq_proto, liquidator, liq_borrower, liq_xlm) = setup_liquidation();
    stable(&liq_proto).liquidate(&liquidator, &liq_borrower, &liq_xlm);
    let measured_liq = BudgetSnapshot::last_invocation(&liq_proto.env);

    let mut baseline = BudgetBaseline::new();
    baseline.record("mint", measured_mint);
    baseline.record("liquidate", measured_liq);

    baseline
        .guard("mint")
        .tolerance_percent(25)
        .run(&proto.env, || {
            stable(&proto).mint(&borrower, &xlm, &500);
        });

    let halved = BudgetSnapshot {
        cpu_insns: measured_liq.cpu_insns / 2,
        mem_bytes: measured_liq.mem_bytes / 2,
    };
    let violations = baseline
        .guard("liquidate")
        .baseline(Some(halved))
        .tolerance_percent(10)
        .violations(&measured_liq);
    assert!(
        violations
            .iter()
            .any(|violation| violation.metric == BudgetMetric::Cpu),
        "a liquidation costing twice its baseline must be flagged as CPU growth"
    );
}

#[test]
fn committed_budget_baseline_loads_and_covers_mint_and_liquidate() {
    let committed = BudgetBaseline::load(&baseline_path())
        .expect("examples/stablecoin/budget.json must exist and parse");

    assert_eq!(committed.len(), 4);
    assert_eq!(
        committed
            .cases()
            .map(|(case, _)| case)
            .collect::<Vec<&str>>(),
        ["deposit_collateral", "liquidate", "mint", "redeem"]
    );

    let mint = committed.get("mint").expect("mint baseline present");
    let liquidate = committed
        .get("liquidate")
        .expect("liquidate baseline present");
    assert!(mint.cpu_insns > 0 && mint.mem_bytes > 0);
    assert!(liquidate.cpu_insns > 0 && liquidate.mem_bytes > 0);

    let round_tripped = BudgetBaseline::parse(&committed.to_json_string())
        .expect("serialized baseline must parse cleanly");
    assert_eq!(round_tripped, committed);
}

#[test]
fn initialize_populates_only_instance_storage() {
    let proto = deploy_unregistered();

    soroban_testkit_core::assert_storage_tier!(
        &proto.env,
        &proto.stable_id,
        "admin",
        StorageTier::Instance
    );
    soroban_testkit_core::assert_storage_tier!(
        &proto.env,
        &proto.stable_id,
        "oracle",
        StorageTier::Instance
    );
    soroban_testkit_core::assert_storage_tier!(
        &proto.env,
        &proto.stable_id,
        "mcr_bps",
        StorageTier::Instance
    );
    soroban_testkit_core::assert_storage_tier!(
        &proto.env,
        &proto.stable_id,
        "supply",
        StorageTier::Instance
    );

    let snapshot = StorageSnapshot::capture(&proto.env, &proto.stable_id);
    assert_eq!(snapshot.len(), 7);
    assert!(snapshot.in_tier(StorageTier::Persistent).is_empty());
    assert!(snapshot.in_tier(StorageTier::Temporary).is_empty());
}

#[test]
fn depositing_and_minting_writes_persistent_vaults_and_balances() {
    let proto = deploy_ready();
    let borrower = Address::generate(&proto.env);
    let xlm = Symbol::new(&proto.env, "XLM");

    let before = StorageSnapshot::capture(&proto.env, &proto.stable_id);
    stable(&proto).deposit_collateral(&borrower, &xlm, &1_500);
    stable(&proto).mint(&borrower, &xlm, &1_000);
    let after = StorageSnapshot::capture(&proto.env, &proto.stable_id);

    before.assert_entry_added(&after, "vaults");
    before.assert_entry_added(&after, "balances");
    soroban_testkit_core::assert_storage_tier!(
        &proto.env,
        &proto.stable_id,
        "vaults",
        StorageTier::Persistent
    );
    soroban_testkit_core::assert_storage_tier!(
        &proto.env,
        &proto.stable_id,
        "balances",
        StorageTier::Persistent
    );
}

#[test]
fn stability_pool_keys_share_the_persistent_tier_under_prefix_pattern() {
    let (proto, liquidator, borrower, xlm) = setup_liquidation();
    stable(&proto).liquidate(&liquidator, &borrower, &xlm);

    soroban_testkit_core::assert_storage_tier!(
        &proto.env,
        &proto.stable_id,
        "pool_*",
        StorageTier::Persistent
    );

    let qualified: Vec<String> = inspect_storage(&proto.env, &proto.stable_id)
        .iter()
        .filter(|entry| entry.key.starts_with("pool_"))
        .map(|entry| entry.qualified())
        .collect();
    assert_eq!(
        qualified,
        ["persistent:pool_collateral", "persistent:pool_stakes"]
    );
}

#[test]
fn liquidation_and_redemption_record_ephemeral_caches_in_temporary_tier() {
    let (proto, liquidator, borrower, xlm) = setup_liquidation();
    stable(&proto).liquidate(&liquidator, &borrower, &xlm);

    // Open a fresh healthy vault and execute a redemption to populate `cache_fee_bps`.
    let holder = Address::generate(&proto.env);
    let wbtc = Symbol::new(&proto.env, "WBTC");
    stable(&proto).deposit_collateral(&holder, &wbtc, &1);
    stable(&proto).mint(&holder, &wbtc, &1_000);
    stable(&proto).redeem(&holder, &holder, &wbtc, &100);

    soroban_testkit_core::assert_storage_tier!(
        &proto.env,
        &proto.stable_id,
        "cache_*",
        StorageTier::Temporary
    );

    let snapshot = StorageSnapshot::capture(&proto.env, &proto.stable_id);
    let temp_keys: Vec<String> = snapshot
        .in_tier(StorageTier::Temporary)
        .iter()
        .map(|entry| entry.key.clone())
        .collect();
    assert_eq!(temp_keys, ["cache_fee_bps", "cache_liq_ts"]);
}

#[test]
#[should_panic(
    expected = "Storage tier mismatch for key 'vaults': expected temporary, found persistent"
)]
fn wrong_storage_tier_assertion_fails_with_diagnostic() {
    let proto = deploy_ready();
    let borrower = Address::generate(&proto.env);
    let xlm = Symbol::new(&proto.env, "XLM");
    stable(&proto).deposit_collateral(&borrower, &xlm, &1_500);

    soroban_testkit_core::assert_storage_tier!(
        &proto.env,
        &proto.stable_id,
        "vaults",
        StorageTier::Temporary
    );
}

// The two baseline tests below mirror `examples/counter` and are `#[ignore]`d
// so `cargo test --workspace` stays immune to cross-SDK instruction drift while
// providing reproducible baseline recording and enforcement commands.

fn hot_paths() -> [(&'static str, BudgetSnapshot); 4] {
    let dep_proto = deploy_ready();
    let dep_user = Address::generate(&dep_proto.env);
    let dep_xlm = Symbol::new(&dep_proto.env, "XLM");
    stable(&dep_proto).deposit_collateral(&dep_user, &dep_xlm, &1_000);
    stable(&dep_proto).deposit_collateral(&dep_user, &dep_xlm, &1_000);
    let deposit_cost = BudgetSnapshot::last_invocation(&dep_proto.env);

    let mint_proto = deploy_ready();
    let mint_user = Address::generate(&mint_proto.env);
    let mint_xlm = Symbol::new(&mint_proto.env, "XLM");
    stable(&mint_proto).deposit_collateral(&mint_user, &mint_xlm, &6_000);
    stable(&mint_proto).mint(&mint_user, &mint_xlm, &1_000);
    stable(&mint_proto).mint(&mint_user, &mint_xlm, &1_000);
    let mint_cost = BudgetSnapshot::last_invocation(&mint_proto.env);

    let (liq_proto, liquidator, borrower, liq_xlm) = setup_liquidation();
    stable(&liq_proto).liquidate(&liquidator, &borrower, &liq_xlm);
    let liquidate_cost = BudgetSnapshot::last_invocation(&liq_proto.env);

    let red_proto = deploy_ready();
    let red_user = Address::generate(&red_proto.env);
    let red_xlm = Symbol::new(&red_proto.env, "XLM");
    stable(&red_proto).deposit_collateral(&red_user, &red_xlm, &4_000);
    stable(&red_proto).mint(&red_user, &red_xlm, &2_000);
    stable(&red_proto).redeem(&red_user, &red_user, &red_xlm, &100);
    let redeem_cost = BudgetSnapshot::last_invocation(&red_proto.env);

    [
        ("deposit_collateral", deposit_cost),
        ("mint", mint_cost),
        ("liquidate", liquidate_cost),
        ("redeem", redeem_cost),
    ]
}

fn report(case: &str, cost: &BudgetSnapshot, recorded: &BudgetSnapshot) {
    let metrics = [
        (BudgetMetric::Cpu, cost.cpu_insns, recorded.cpu_insns),
        (BudgetMetric::Memory, cost.mem_bytes, recorded.mem_bytes),
    ];
    for (metric, actual, start) in metrics {
        let drift = if start == 0 {
            0.0
        } else {
            (actual as f64 - start as f64) * 100.0 / start as f64
        };
        println!(
            "BUDGET_SUMMARY case={case} metric={} actual={actual} baseline={start} drift={drift:+.2}%",
            metric.as_str()
        );
    }
}

#[test]
#[ignore = "rewrites examples/stablecoin/budget.json — run it deliberately, then commit the file"]
fn budget_baseline_records_current_costs() {
    let mut baseline = BudgetBaseline::new();
    for (case, cost) in hot_paths() {
        baseline.record(case, cost);
    }

    baseline.save(&baseline_path()).unwrap();
    println!("{}", baseline.to_json_string());
}

#[test]
#[ignore = "enforced on a pinned runner against examples/stablecoin/budget.json"]
fn budget_baseline_rejects_drifted_costs() {
    let recorded = BudgetBaseline::load(&baseline_path()).expect(
        "examples/stablecoin/budget.json is missing; run budget_baseline_records_current_costs",
    );

    let tolerance = std::env::var("TESTKIT_BUDGET_TOLERANCE")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(10);

    let mut breaches = Vec::new();
    for (case, cost) in hot_paths() {
        match recorded.get(case) {
            Some(start) => {
                report(case, &cost, &start);
                breaches.extend(
                    recorded
                        .guard(case)
                        .tolerance_percent(tolerance)
                        .violations(&cost),
                );
            }
            None => println!("BUDGET_SUMMARY case={case} status=unrecorded"),
        }
    }

    if !breaches.is_empty() {
        let lines = breaches
            .iter()
            .map(|breach| breach.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        panic!("budget drifted past {tolerance}%:\n{lines}");
    }
}
