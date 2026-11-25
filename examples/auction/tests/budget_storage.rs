//! Resource use measured through `soroban-testkit-core`: a settlement
//! benchmark across 5, 20 and 50 bidders, per-call budget ceilings, a
//! committed budget baseline, a full order book checked against the network
//! resource limits, and where every entry lives in storage.
//!
//! The numbers are what the soroban test host meters for a *native* test
//! contract: VM instantiation and Wasm execution are absent, so a deployed
//! Wasm build costs more. They are deterministic for a given lockfile, which
//! is what makes them useful for spotting regressions.

mod common;

use std::path::{Path, PathBuf};

use common::*;
use soroban_testkit_core::budget::{BudgetBaseline, BudgetGuard, BudgetSnapshot};
use soroban_testkit_core::storage::{StorageSnapshot, StorageTier};
use soroban_testkit_example_auction::MAX_BATCH_BIDS;
use soroban_testkit_fixtures::NetworkProfile;

/// Testnet's per-transaction ceilings: 100M instructions and 40 MiB.
const TX_CPU: u64 = 100_000_000;
const TX_MEM: u64 = 40 * 1024 * 1024;

/// Growth a committed baseline tolerates before the suite fails.
const BASELINE_TOLERANCE_PERCENT: u64 = 10;

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("budget.json")
}

/// What one batch auction with `bidders` bidders costs.
#[derive(Debug, Clone, Copy)]
struct Measurement {
    bidders: usize,
    /// Sum of every `bid_batch` invocation.
    bidding: BudgetSnapshot,
    /// The one `settle` invocation: clearing-price discovery and the
    /// seller's payout.
    settle: BudgetSnapshot,
    /// Sum of every `claim` invocation: each bid's allocation and refund.
    claims: BudgetSnapshot,
    /// The most expensive single `claim` (the lowest-ranked bid, which scans
    /// the whole book).
    worst_claim: BudgetSnapshot,
    clearing_price: i128,
}

fn add(total: &mut BudgetSnapshot, step: BudgetSnapshot) {
    total.cpu_insns += step.cpu_insns;
    total.mem_bytes += step.mem_bytes;
}

fn zero() -> BudgetSnapshot {
    BudgetSnapshot {
        cpu_insns: 0,
        mem_bytes: 0,
    }
}

/// Builds the benchmark scenario, fixed so runs are comparable: bidder `i`
/// asks for 10 units at `10 + i` per unit, and supply is `5 × bidders`, so
/// the auction is exactly 2× oversubscribed and half the bidders win.
fn scenario(bidders: usize, profile: NetworkProfile) -> (House, u64, BudgetSnapshot) {
    let mut house = House::with_profile(bidders, profile);
    let id = house.batch_with(5 * bidders as i128, RESERVE);
    let mut bidding = zero();
    for index in 0..bidders {
        house
            .client()
            .bid_batch(&id, &house.bidder(index), &10, &(10 + index as i128));
        add(
            &mut bidding,
            BudgetSnapshot::last_invocation(&house.ctx.env),
        );
    }
    house.close_window();
    (house, id, bidding)
}

fn measure(bidders: usize) -> Measurement {
    let (house, id, bidding) = scenario(bidders, NetworkProfile::Local);

    let settled = house.client().settle(&id);
    let settle = BudgetSnapshot::last_invocation(&house.ctx.env);

    let mut claims = zero();
    let mut worst_claim = zero();
    for index in 0..bidders {
        house.client().claim(&id, &house.bidder(index));
        let step = BudgetSnapshot::last_invocation(&house.ctx.env);
        add(&mut claims, step);
        if step.cpu_insns > worst_claim.cpu_insns {
            worst_claim = step;
        }
    }

    Measurement {
        bidders,
        bidding,
        settle,
        claims,
        worst_claim,
        clearing_price: settled.clearing_price.unwrap(),
    }
}

#[test]
fn settlement_benchmark_for_5_20_and_50_bidders() {
    let measured: Vec<Measurement> = [5usize, 20, 50].into_iter().map(measure).collect();

    // A benchmark is a recorded measurement: print the table so a reviewer can
    // compare it with the committed baseline. Run with `-- --nocapture`.
    println!(
        "bidders | settle cpu | settle mem | all claims cpu | all claims mem | worst claim cpu | worst claim mem | all bids cpu | all bids mem"
    );
    for m in &measured {
        println!(
            "{:>7} | {:>10} | {:>10} | {:>14} | {:>14} | {:>15} | {:>15} | {:>12} | {:>12}",
            m.bidders,
            m.settle.cpu_insns,
            m.settle.mem_bytes,
            m.claims.cpu_insns,
            m.claims.mem_bytes,
            m.worst_claim.cpu_insns,
            m.worst_claim.mem_bytes,
            m.bidding.cpu_insns,
            m.bidding.mem_bytes
        );
    }

    for m in &measured {
        // Half the bidders win; the marginal bid is the one at index
        // `bidders / 2`, priced `10 + bidders / 2`.
        assert_eq!(m.clearing_price, 10 + (m.bidders / 2) as i128);
        for (call, cost) in [("settle", m.settle), ("claim", m.worst_claim)] {
            BudgetGuard::new(format!("{call}_{}_bidders", m.bidders))
                .cpu_ceiling(TX_CPU)
                .mem_ceiling(TX_MEM)
                .assert_within(&cost);
        }
    }
    for pair in measured.windows(2) {
        let (small, large) = (pair[0], pair[1]);
        assert!(
            large.settle.cpu_insns > small.settle.cpu_insns
                && large.claims.cpu_insns > small.claims.cpu_insns,
            "{} bidders cannot cost less than {}",
            large.bidders,
            small.bidders
        );
    }
}

#[test]
fn settlement_costs_stay_within_the_committed_baseline() {
    let committed = BudgetBaseline::load(&baseline_path())
        .expect("examples/auction/budget.json must exist and parse");
    for bidders in [5usize, 20, 50] {
        let m = measure(bidders);
        for (case, cost) in [
            (format!("settle_{bidders}_bidders"), m.settle),
            (format!("claims_{bidders}_bidders"), m.claims),
        ] {
            assert!(
                committed.get(&case).is_some(),
                "missing baseline for {case}"
            );
            committed
                .guard(&case)
                .tolerance_percent(BASELINE_TOLERANCE_PERCENT)
                .assert_within(&cost);
        }
    }
}

#[test]
fn every_entry_point_stays_inside_a_transaction() {
    let mut house = House::new(2);

    let dutch = soroban_testkit_core::budget_guard!(
        &house.ctx.env,
        "create_dutch",
        { cpu_max: TX_CPU, mem_max: TX_MEM },
        || house.dutch()
    );
    let batch = soroban_testkit_core::budget_guard!(
        &house.ctx.env,
        "create_batch",
        { cpu_max: TX_CPU, mem_max: TX_MEM },
        || house.batch()
    );
    soroban_testkit_core::budget_guard!(
        &house.ctx.env,
        "bid_batch",
        { cpu_max: TX_CPU, mem_max: TX_MEM },
        || house.client().bid_batch(&batch, &house.bidder(0), &10, &20)
    );
    soroban_testkit_core::budget_guard!(
        &house.ctx.env,
        "bid_dutch",
        { cpu_max: TX_CPU, mem_max: TX_MEM },
        || house.client().bid_dutch(&dutch, &house.bidder(1), &START_PRICE)
    );
    house.close_window();
    soroban_testkit_core::budget_guard!(
        &house.ctx.env,
        "settle",
        { cpu_max: TX_CPU, mem_max: TX_MEM },
        || house.client().settle(&batch)
    );
    soroban_testkit_core::budget_guard!(
        &house.ctx.env,
        "claim",
        { cpu_max: TX_CPU, mem_max: TX_MEM },
        || house.client().claim(&batch, &house.bidder(0))
    );
}

#[test]
fn reading_the_price_costs_less_than_buying_at_it() {
    let mut house = House::new(1);
    let id = house.dutch();
    house.advance(120);

    house.client().current_price(&id);
    let read = BudgetSnapshot::last_invocation(&house.ctx.env);
    house
        .client()
        .bid_dutch(&id, &house.bidder(0), &START_PRICE);
    let buy = BudgetSnapshot::last_invocation(&house.ctx.env);

    assert!(read.cpu_insns < buy.cpu_insns);
    assert!(read.mem_bytes < buy.mem_bytes);
}

/// Every resource the last invocation used, checked against a network
/// profile's per-transaction limits.
fn assert_fits(house: &House, call: &str, profile: NetworkProfile) {
    let used = house.ctx.env.cost_estimate().resources();
    let limits = profile
        .invocation_limits()
        .expect("metered profiles have limits");
    let footprint = used.disk_read_entries + used.memory_read_entries + used.write_entries;
    let checks = [
        ("instructions", used.instructions, limits.instructions),
        ("mem_bytes", used.mem_bytes, limits.mem_bytes),
        (
            "disk_read_entries",
            i64::from(used.disk_read_entries),
            i64::from(limits.disk_read_entries),
        ),
        (
            "write_entries",
            i64::from(used.write_entries),
            i64::from(limits.write_entries),
        ),
        (
            "ledger_entries (upper bound)",
            i64::from(footprint),
            i64::from(limits.ledger_entries),
        ),
        (
            "disk_read_bytes",
            i64::from(used.disk_read_bytes),
            i64::from(limits.disk_read_bytes),
        ),
        (
            "write_bytes",
            i64::from(used.write_bytes),
            i64::from(limits.write_bytes),
        ),
        (
            "contract_events_size_bytes",
            i64::from(used.contract_events_size_bytes),
            i64::from(limits.contract_events_size_bytes),
        ),
    ];
    for (resource, value, limit) in checks {
        assert!(
            value <= limit,
            "{call} uses {value} {resource}, over the {} limit of {limit}",
            profile.name()
        );
    }
}

#[test]
fn a_full_order_book_fits_the_network_resource_limits() {
    let bidders = MAX_BATCH_BIDS as usize;
    for profile in [NetworkProfile::Testnet, NetworkProfile::Mainnet] {
        // The profile makes the host enforce the network's limits on every
        // invocation, so any call that broke one would fail outright...
        let (house, id, _) = scenario(bidders, profile);
        house.client().settle(&id);
        // ...and the measured resources are compared with them explicitly,
        // so a report names the resource that is closest to the edge.
        assert_fits(&house, "settle", profile);

        // Claims run in book order, so the last one scans the whole book:
        // the worst case.
        for index in 0..bidders {
            house.client().claim(&id, &house.bidder(index));
        }
        assert_fits(&house, "claim", profile);
        assert_eq!(house.payment_of(&house.contract_id), 0);
        assert_eq!(house.asset_of(&house.contract_id), 0);
    }
}

// ---------------------------------------------------------------- storage

#[test]
fn creation_stores_the_auction_persistently_and_the_counter_in_instance() {
    let house = House::new(0);
    let id = house.batch();
    assert_eq!(id, 0);

    let snapshot = StorageSnapshot::capture(&house.ctx.env, &house.contract_id);
    let persistent: Vec<String> = snapshot
        .in_tier(StorageTier::Persistent)
        .iter()
        .map(|entry| entry.key.clone())
        .collect();
    assert_eq!(persistent, vec!["[Auction, 0]".to_string()]);
    assert!(snapshot.get("[NextId]").is_some());
    assert_eq!(
        snapshot.get("[NextId]").unwrap().tier,
        StorageTier::Instance
    );
    assert!(snapshot.in_tier(StorageTier::Temporary).is_empty());
}

#[test]
fn each_bid_adds_one_persistent_entry_and_updates_the_book() {
    let house = House::new(3);
    let id = house.batch();
    house.place_bids(id, &[(10, 20)]);

    let before = StorageSnapshot::capture(&house.ctx.env, &house.contract_id);
    house.client().bid_batch(&id, &house.bidder(1), &10, &30);
    let after = StorageSnapshot::capture(&house.ctx.env, &house.contract_id);

    let diff = before.diff(&after);
    assert_eq!(diff.added().len(), 1, "{diff:?}");
    assert_eq!(diff.added()[0].tier, StorageTier::Persistent);
    assert!(diff.added()[0].key.starts_with("[Bid, 0, "));
    let modified: Vec<&str> = diff.modified().iter().map(|c| c.key.as_str()).collect();
    assert!(modified.contains(&"[Book, 0]"), "{modified:?}");
    assert!(modified.contains(&"[Auction, 0]"), "{modified:?}");
    assert!(diff.removed().is_empty());

    // Auction + book + one record per bid.
    house.client().bid_batch(&id, &house.bidder(2), &10, &30);
    let full = StorageSnapshot::capture(&house.ctx.env, &house.contract_id);
    assert_eq!(full.in_tier(StorageTier::Persistent).len(), 2 + 3);
}

#[test]
fn settlement_touches_only_the_auction_and_each_claim_only_its_bid() {
    let mut house = House::new(4);
    let id = house.batch();
    house.place_bids(id, &[(400, 30), (500, 25), (300, 20), (200, 15)]);
    house.close_window();

    let before = StorageSnapshot::capture(&house.ctx.env, &house.contract_id);
    house.client().settle(&id);
    let settled = StorageSnapshot::capture(&house.ctx.env, &house.contract_id);
    let diff = before.diff(&settled);
    assert!(diff.added().is_empty() && diff.removed().is_empty());
    let modified: Vec<&str> = diff.modified().iter().map(|c| c.key.as_str()).collect();
    assert_eq!(modified, vec!["[Auction, 0]"]);

    house.client().claim(&id, &house.bidder(2));
    let claimed = StorageSnapshot::capture(&house.ctx.env, &house.contract_id);
    let diff = settled.diff(&claimed);
    assert!(diff.added().is_empty() && diff.removed().is_empty());
    let mut modified: Vec<&str> = diff.modified().iter().map(|c| c.key.as_str()).collect();
    modified.sort();
    assert_eq!(modified.len(), 2, "{modified:?}");
    assert_eq!(modified[0], "[Auction, 0]");
    assert!(modified[1].starts_with("[Bid, 0, "), "{modified:?}");
}

// ---------------------------------------------------------------- recorder

#[test]
#[ignore = "rewrites examples/auction/budget.json — run deliberately"]
fn budget_baseline_records_current_costs() {
    let mut baseline = BudgetBaseline::new();
    for bidders in [5usize, 20, 50] {
        let m = measure(bidders);
        baseline.record(format!("settle_{bidders}_bidders"), m.settle);
        baseline.record(format!("claims_{bidders}_bidders"), m.claims);
    }
    baseline.save(&baseline_path()).unwrap();
    println!("{}", baseline.to_json_string());
}
