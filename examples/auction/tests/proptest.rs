//! Auction invariants checked with `proptest` and `soroban-testkit-generators`
//! over randomized bid books and Dutch schedules:
//!
//! * a winning bidder never pays more than their maximum bid;
//! * allocations never exceed the supply, and match an independent model of
//!   the documented fill and clearing-price rules;
//! * escrow accounting conserves both tokens, and refunds return every
//!   escrowed token that was not paid;
//! * with distinct prices, the outcome does not depend on submission order;
//! * the Dutch price is non-increasing, bounded by start and floor, and a
//!   Dutch bid is accepted exactly when it covers the asking price.

mod common;

use common::*;
use proptest::prelude::*;
use soroban_testkit_generators::strategies::{timestamp, token_amount};

/// Up to 12 `(quantity, max_price)` bids.
fn bid_book() -> impl Strategy<Value = Vec<(i128, i128)>> {
    proptest::collection::vec((1i128..=200, RESERVE..=200i128), 1..=12)
}

/// The documented allocation rule, written independently of the contract:
/// rank by price descending then submission order, fill greedily, clear at
/// the reserve when undersubscribed and at the marginal bid's price
/// otherwise. Returns the clearing price and each bid's allocation in
/// submission order.
fn model(supply: i128, bids: &[(i128, i128)]) -> (i128, Vec<i128>) {
    let mut ranked: Vec<usize> = (0..bids.len()).collect();
    // A stable sort keeps submission order among equal prices.
    ranked.sort_by(|&a, &b| bids[b].1.cmp(&bids[a].1));
    let demand: i128 = bids.iter().map(|b| b.0).sum();

    let mut allocations = vec![0; bids.len()];
    let mut remaining = supply;
    let mut marginal = RESERVE;
    for &index in &ranked {
        if remaining == 0 {
            break;
        }
        let fill = bids[index].0.min(remaining);
        allocations[index] = fill;
        remaining -= fill;
        marginal = bids[index].1;
    }
    let clearing = if demand <= supply { RESERVE } else { marginal };
    (clearing, allocations)
}

/// Runs a batch auction over `bids` to completion (bids, settle, every
/// claim) and returns it with the auction id.
fn run_batch(supply: i128, bids: &[(i128, i128)]) -> (House, u64) {
    let mut house = House::new(bids.len());
    let id = house.batch_with(supply, RESERVE);
    house.place_bids(id, bids);
    house.close_window();
    house.client().settle(&id);
    house.claim_all(id, bids.len());
    (house, id)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn a_winning_bidder_never_pays_more_than_their_maximum_bid(
        supply in 1i128..=1_000,
        bids in bid_book(),
    ) {
        let (house, id) = run_batch(supply, &bids);
        let clearing = house.client().auction(&id).clearing_price.unwrap();

        for (index, &(quantity, max_price)) in bids.iter().enumerate() {
            let bid = house.bid_of(id, index);
            prop_assert!(bid.allocation <= quantity);
            if bid.allocation > 0 {
                prop_assert!(clearing <= max_price, "bidder {} pays {} > max {}", index, clearing, max_price);
            }
            prop_assert!(bid.payment <= bid.allocation * max_price);
            // What the bidder actually lost is exactly what they paid.
            prop_assert_eq!(BIDDER_FUNDS - house.payment_of(&house.bidder(index)), bid.payment);
            prop_assert!(bid.payment <= quantity * max_price);
        }
    }

    #[test]
    fn allocations_never_exceed_supply_and_follow_the_documented_rule(
        supply in 1i128..=1_000,
        bids in bid_book(),
    ) {
        let (house, id) = run_batch(supply, &bids);
        let (clearing, allocations) = model(supply, &bids);
        let auction = house.client().auction(&id);

        let total: i128 = (0..bids.len()).map(|i| house.bid_of(id, i).allocation).sum();
        let demand: i128 = bids.iter().map(|b| b.0).sum();
        prop_assert!(total <= supply);
        prop_assert_eq!(total, supply.min(demand));
        prop_assert_eq!(auction.units_sold, total);
        prop_assert_eq!(auction.clearing_price, Some(clearing));
        for (index, expected) in allocations.iter().enumerate() {
            prop_assert_eq!(house.bid_of(id, index).allocation, *expected, "bidder {}", index);
            prop_assert_eq!(house.asset_of(&house.bidder(index)), *expected);
        }
    }

    #[test]
    fn escrow_accounting_conserves_both_tokens(
        supply in 1i128..=1_000,
        bids in bid_book(),
    ) {
        let (house, id) = run_batch(supply, &bids);
        let auction = house.client().auction(&id);
        let clearing = auction.clearing_price.unwrap();

        let mut escrowed = 0;
        let mut paid = 0;
        let mut refunded = 0;
        for index in 0..bids.len() {
            let bid = house.bid_of(id, index);
            prop_assert!(bid.claimed);
            prop_assert!(bid.refund >= 0);
            prop_assert_eq!(bid.escrow, bid.quantity * bid.max_price);
            prop_assert_eq!(bid.payment + bid.refund, bid.escrow);
            escrowed += bid.escrow;
            paid += bid.payment;
            refunded += bid.refund;
        }
        prop_assert_eq!(paid + refunded, escrowed);
        prop_assert_eq!(paid, auction.units_sold * clearing);
        prop_assert_eq!(auction.proceeds, paid);
        prop_assert_eq!(auction.total_escrow, 0);

        // Nothing is created or destroyed, and nothing is left in escrow.
        let funds = BIDDER_FUNDS * bids.len() as i128;
        let payment_total: i128 = house.payment_of(&house.seller)
            + house.payment_of(&house.contract_id)
            + house.bidders().iter().map(|b| house.payment_of(b)).sum::<i128>();
        prop_assert_eq!(payment_total, funds);
        prop_assert_eq!(house.payment_of(&house.contract_id), 0);
        prop_assert_eq!(house.payment_of(&house.seller), paid);

        let asset_total: i128 = house.asset_of(&house.seller)
            + house.asset_of(&house.contract_id)
            + house.bidders().iter().map(|b| house.asset_of(b)).sum::<i128>();
        prop_assert_eq!(asset_total, SELLER_ASSET);
        prop_assert_eq!(house.asset_of(&house.contract_id), 0);
    }

    #[test]
    fn with_distinct_prices_submission_order_does_not_matter(
        supply in 1i128..=600,
        quantities in proptest::collection::vec(1i128..=150, 2..=8),
        rotation in 0usize..8,
    ) {
        // Distinct prices: bidder i bids at RESERVE + 7i.
        let bids: Vec<(i128, i128)> = quantities
            .iter()
            .enumerate()
            .map(|(i, &q)| (q, RESERVE + 7 * i as i128))
            .collect();
        let shift = rotation % bids.len();
        let mut rotated = bids.clone();
        rotated.rotate_left(shift);

        let (a, id_a) = run_batch(supply, &bids);
        let (b, id_b) = run_batch(supply, &rotated);
        prop_assert_eq!(
            a.client().auction(&id_a).clearing_price,
            b.client().auction(&id_b).clearing_price
        );
        for index in 0..bids.len() {
            // Bid `index` of the original order sits at `index - shift` after
            // the rotation.
            let moved = (index + bids.len() - shift) % bids.len();
            prop_assert_eq!(a.bid_of(id_a, index).allocation, b.bid_of(id_b, moved).allocation);
        }
    }

    #[test]
    fn dutch_price_is_non_increasing_and_bounded(
        start in timestamp(),
        floor in token_amount().prop_map(|v| v % 1_000_000 + 1),
        spread in token_amount().prop_map(|v| v % 1_000_000),
        steps in 1u64..=20,
        step in 1u64..=120,
    ) {
        let mut house = House::new(0);
        house.ctx.set_timestamp(start);
        let duration = steps * step;
        let id = house.dutch_with(floor + spread, floor, duration, step);

        prop_assert_eq!(house.client().current_price(&id), floor + spread);
        let mut previous = floor + spread;
        let mut elapsed = 0u64;
        while elapsed <= duration + step {
            let price = house.client().current_price(&id);
            prop_assert!(price <= previous);
            prop_assert!(price >= floor && price <= floor + spread);
            // The published rule: drop = spread × stepped / duration, rounded down.
            let stepped = elapsed.min(duration) / step * step;
            prop_assert_eq!(price, floor + spread - spread * stepped as i128 / duration as i128);
            previous = price;
            house.advance(step / 2 + 1);
            elapsed = house.now() - start;
        }
        prop_assert_eq!(house.client().current_price(&id), floor);
    }

    #[test]
    fn dutch_bid_is_accepted_exactly_when_it_covers_the_asking_price(
        wait in 0u64..=DECAY,
        max_price in FLOOR_PRICE - 500..=START_PRICE + 500,
    ) {
        let mut house = House::new(1);
        let id = house.dutch();
        house.advance(wait);
        let asking = house.client().current_price(&id);

        let outcome = house.client().try_bid_dutch(&id, &house.bidder(0), &max_price);
        if max_price >= asking {
            prop_assert_eq!(outcome.unwrap().unwrap(), asking);
            prop_assert_eq!(house.payment_of(&house.bidder(0)), BIDDER_FUNDS - asking);
            prop_assert!(asking <= max_price);
        } else {
            prop_assert!(outcome.is_err());
            prop_assert_eq!(house.payment_of(&house.bidder(0)), BIDDER_FUNDS);
        }
    }
}
