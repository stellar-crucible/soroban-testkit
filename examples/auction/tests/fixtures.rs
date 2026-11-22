//! Auction lifecycle driven through `soroban-testkit-fixtures`: escrow on
//! creation, Dutch price decay on a moving ledger clock (`advance_time`),
//! bids before and after price changes, batch bid submission, clearing-price
//! discovery, settlement and claims with different bidder counts, refunds,
//! and insufficient balances.

mod common;

use common::*;
use soroban_testkit_example_auction::{AuctionKind, BookEntry};

// ------------------------------------------------------------------ Dutch

#[test]
fn dutch_creation_escrows_the_lot_and_records_the_schedule() {
    let house = House::new(1);
    let id = house.dutch();

    let auction = house.client().auction(&id);
    assert_eq!(auction.kind, AuctionKind::Dutch);
    assert_eq!(auction.seller, house.seller);
    assert_eq!(auction.supply, LOT);
    assert_eq!(auction.start_price, START_PRICE);
    assert_eq!(auction.floor_price, FLOOR_PRICE);
    assert_eq!(auction.start_time, T0);
    assert_eq!(auction.end_time, T0 + DECAY);
    assert!(!auction.settled);
    assert_eq!(auction.winner, None);

    assert_eq!(house.asset_of(&house.contract_id), LOT);
    assert_eq!(house.asset_of(&house.seller), SELLER_ASSET - LOT);
}

#[test]
fn dutch_price_decays_linearly_as_time_advances() {
    let mut house = House::new(0);
    let id = house.dutch();

    // spread 6_000 over 600 s: 10 per second.
    let mut observed = Vec::new();
    for step in [0u64, 150, 150, 150, 150] {
        house.advance(step);
        observed.push(house.client().current_price(&id));
    }
    assert_eq!(observed, vec![10_000, 8_500, 7_000, 5_500, 4_000]);

    // The price never falls below the floor once the decay window is over.
    house.advance(10_000);
    assert_eq!(house.client().current_price(&id), FLOOR_PRICE);
}

#[test]
fn dutch_stepped_schedule_holds_the_price_between_steps() {
    let mut house = House::new(0);
    // 6_000 over 600 s in 60 s steps: 600 per step.
    let id = house.dutch_with(START_PRICE, FLOOR_PRICE, DECAY, 60);

    house.advance(59);
    assert_eq!(house.client().current_price(&id), 10_000);
    house.advance(1);
    assert_eq!(house.client().current_price(&id), 9_400);
    house.advance(59);
    assert_eq!(house.client().current_price(&id), 9_400);
    house.advance(1);
    assert_eq!(house.client().current_price(&id), 8_800);
}

#[test]
fn dutch_price_rounds_up_in_favour_of_the_schedule() {
    let mut house = House::new(0);
    // 999 over 7 s: the exact price after 1 s is 857.29, so 858 is asked.
    let id = house.dutch_with(1_000, 1, 7, 1);
    house.advance(1);
    assert_eq!(house.client().current_price(&id), 858);
    house.advance(6);
    assert_eq!(house.client().current_price(&id), 1);
}

#[test]
fn dutch_bid_below_the_price_fails_until_the_price_falls_to_it() {
    let mut house = House::new(1);
    let id = house.dutch();
    let bidder = house.bidder(0);

    assert!(house.client().try_bid_dutch(&id, &bidder, &8_000).is_err());
    assert_eq!(house.payment_of(&bidder), BIDDER_FUNDS);

    // 200 s later the price is exactly 8_000.
    house.advance(200);
    let paid = house.client().bid_dutch(&id, &bidder, &8_000);
    assert_eq!(paid, 8_000);
    assert_eq!(house.payment_of(&bidder), BIDDER_FUNDS - 8_000);
    assert_eq!(house.payment_of(&house.contract_id), 8_000);
}

#[test]
fn dutch_winner_pays_the_asking_price_not_their_maximum() {
    let mut house = House::new(1);
    let id = house.dutch();
    house.advance(100);

    let paid = house.client().bid_dutch(&id, &house.bidder(0), &9_999);
    assert_eq!(paid, 9_000);
    let auction = house.client().auction(&id);
    assert_eq!(auction.winner, Some(house.bidder(0)));
    assert_eq!(auction.clearing_price, Some(9_000));
    assert_eq!(auction.total_escrow, 9_000);
}

#[test]
fn dutch_settlement_delivers_the_lot_and_the_proceeds() {
    let mut house = House::new(1);
    let id = house.dutch();
    house.advance(300);
    house.client().bid_dutch(&id, &house.bidder(0), &7_000);

    let settled = house.client().settle(&id);
    assert!(settled.settled);
    assert_eq!(settled.units_sold, LOT);
    assert_eq!(settled.proceeds, 7_000);
    assert_eq!(house.asset_of(&house.bidder(0)), LOT);
    assert_eq!(house.payment_of(&house.seller), 7_000);
    assert_eq!(house.asset_of(&house.contract_id), 0);
    assert_eq!(house.payment_of(&house.contract_id), 0);
}

#[test]
fn dutch_bid_at_the_last_second_pays_the_floor_and_after_it_is_refused() {
    let mut house = House::new(2);
    let id = house.dutch();
    house.advance(DECAY);
    assert_eq!(
        house
            .client()
            .bid_dutch(&id, &house.bidder(0), &FLOOR_PRICE),
        FLOOR_PRICE
    );

    let second = house.dutch();
    house.advance(DECAY + 1);
    assert!(house
        .client()
        .try_bid_dutch(&second, &house.bidder(1), &START_PRICE)
        .is_err());
}

#[test]
fn unsold_dutch_lot_returns_to_the_seller_after_the_window() {
    let mut house = House::new(0);
    let id = house.dutch();
    house.advance(DECAY + 1);

    let settled = house.client().settle(&id);
    assert_eq!(settled.units_sold, 0);
    assert_eq!(settled.proceeds, 0);
    assert_eq!(house.asset_of(&house.seller), SELLER_ASSET);
    assert_eq!(house.asset_of(&house.contract_id), 0);
}

// ------------------------------------------------------------------ Batch

#[test]
fn batch_bids_escrow_quantity_times_max_price() {
    let house = House::new(2);
    let id = house.batch();
    house.place_bids(id, &[(100, 25), (40, 12)]);

    assert_eq!(house.payment_of(&house.bidder(0)), BIDDER_FUNDS - 2_500);
    assert_eq!(house.payment_of(&house.bidder(1)), BIDDER_FUNDS - 480);
    assert_eq!(house.payment_of(&house.contract_id), 2_980);

    let auction = house.client().auction(&id);
    assert_eq!(auction.bid_count, 2);
    assert_eq!(auction.total_demand, 140);
    assert_eq!(auction.total_escrow, 2_980);
    assert_eq!(house.asset_of(&house.contract_id), SUPPLY);
}

#[test]
fn undersubscribed_batch_fills_every_bid_at_the_reserve_price() {
    let mut house = House::new(3);
    let id = house.batch();
    house.place_bids(id, &[(100, 50), (200, 20), (300, 10)]);
    house.close_window();

    let settled = house.client().settle(&id);
    assert_eq!(settled.clearing_price, Some(RESERVE));
    assert_eq!(settled.units_sold, 600);
    assert_eq!(settled.proceeds, 6_000);

    // Settlement pays the seller; the bidders' escrow waits for their claims.
    assert_eq!(house.client().auction(&id).total_escrow, 6_000);
    house.claim_all(id, 3);

    let expected = [(100, 1_000, 4_000), (200, 2_000, 2_000), (300, 3_000, 0)];
    for (index, (allocation, payment, refund)) in expected.into_iter().enumerate() {
        let bid = house.bid_of(id, index);
        assert_eq!(
            (bid.allocation, bid.payment, bid.refund),
            (allocation, payment, refund)
        );
        assert_eq!(house.asset_of(&house.bidder(index)), allocation);
        assert_eq!(
            house.payment_of(&house.bidder(index)),
            BIDDER_FUNDS - payment
        );
    }
    // 400 unsold units and the proceeds go back to the seller.
    assert_eq!(house.asset_of(&house.seller), SELLER_ASSET - SUPPLY + 400);
    assert_eq!(house.payment_of(&house.seller), 6_000);
    assert_eq!(house.payment_of(&house.contract_id), 0);
    assert_eq!(house.asset_of(&house.contract_id), 0);
}

#[test]
fn oversubscribed_batch_clears_at_the_marginal_bid() {
    let mut house = House::new(4);
    let id = house.batch();
    // Ranked 30, 25, 20, 15; cumulative 400, 900, 1_200 — the 20 bid is
    // marginal and gets the last 100 units.
    house.place_bids(id, &[(400, 30), (500, 25), (300, 20), (200, 15)]);
    house.close_window();

    let settled = house.client().settle(&id);
    assert_eq!(settled.clearing_price, Some(20));
    assert_eq!(settled.units_sold, SUPPLY);
    assert_eq!(settled.proceeds, 20_000);

    let expected = [
        (400, 8_000, 4_000),
        (500, 10_000, 2_500),
        (100, 2_000, 4_000),
        (0, 0, 3_000),
    ];
    for (index, (allocation, payment, refund)) in expected.into_iter().enumerate() {
        let bid = house.bid_of(id, index);
        assert_eq!(
            (bid.allocation, bid.payment, bid.refund),
            (allocation, payment, refund),
            "bidder {index}"
        );
        assert!(bid.payment <= bid.allocation * bid.max_price);
    }
    assert_eq!(house.asset_of(&house.seller), SELLER_ASSET - SUPPLY);
    assert_eq!(house.payment_of(&house.seller), 20_000);

    let claimed = house.claim_all(id, 4);
    for (index, bid) in claimed.iter().enumerate() {
        assert_eq!((bid.allocation, bid.payment, bid.refund), expected[index]);
        assert_eq!(house.asset_of(&house.bidder(index)), bid.allocation);
        assert_eq!(
            house.payment_of(&house.bidder(index)),
            BIDDER_FUNDS - bid.payment
        );
    }
    assert_eq!(house.payment_of(&house.contract_id), 0);
    assert_eq!(house.asset_of(&house.contract_id), 0);
    assert_eq!(house.client().auction(&id).total_escrow, 0);
}

#[test]
fn tied_bids_are_filled_in_submission_order() {
    let mut house = House::new(3);
    let id = house.batch_with(500, RESERVE);
    house.place_bids(id, &[(300, 20), (300, 20), (300, 20)]);

    let book = house.client().book(&id);
    let order: Vec<u32> = book.iter().map(|entry: BookEntry| entry.sequence).collect();
    assert_eq!(order, vec![0, 1, 2]);

    house.close_window();
    house.client().settle(&id);
    assert_eq!(house.bid_of(id, 0).allocation, 300);
    assert_eq!(house.bid_of(id, 1).allocation, 200);
    assert_eq!(house.bid_of(id, 2).allocation, 0);
    assert_eq!(house.bid_of(id, 2).refund, 6_000);
}

#[test]
fn a_later_higher_bid_outranks_an_earlier_lower_one() {
    let mut house = House::new(3);
    let id = house.batch_with(100, RESERVE);
    house.place_bids(id, &[(100, 11), (100, 30), (100, 11)]);

    let book = house.client().book(&id);
    let ranked: Vec<(u32, i128)> = book
        .iter()
        .map(|entry: BookEntry| (entry.sequence, entry.max_price))
        .collect();
    assert_eq!(ranked, vec![(1, 30), (0, 11), (2, 11)]);

    house.close_window();
    let settled = house.client().settle(&id);
    // Only the 30 bid is filled, so it sets the clearing price itself.
    assert_eq!(settled.clearing_price, Some(30));
    assert_eq!(house.bid_of(id, 1).allocation, 100);
    assert_eq!(house.bid_of(id, 0).allocation, 0);
}

#[test]
fn demand_exactly_equal_to_supply_is_not_oversubscribed() {
    let mut house = House::new(2);
    let id = house.batch_with(300, RESERVE);
    house.place_bids(id, &[(200, 40), (100, 15)]);
    house.close_window();

    let settled = house.client().settle(&id);
    assert_eq!(settled.clearing_price, Some(RESERVE));
    assert_eq!(settled.units_sold, 300);
}

#[test]
fn settlement_scales_across_bidder_counts() {
    for count in [1usize, 3, 10, 25] {
        let mut house = House::new(count);
        let id = house.batch_with(500, RESERVE);
        let bids: Vec<(i128, i128)> = (0..count).map(|i| (60, 10 + i as i128)).collect();
        house.place_bids(id, &bids);
        house.close_window();

        let settled = house.client().settle(&id);
        let demand = 60 * count as i128;
        assert_eq!(settled.units_sold, demand.min(500), "{count} bidders");
        let allocated: i128 = house
            .claim_all(id, count)
            .iter()
            .map(|bid| bid.allocation)
            .sum();
        assert_eq!(allocated, settled.units_sold, "{count} bidders");
        assert_eq!(house.payment_of(&house.contract_id), 0, "{count} bidders");
        assert_eq!(house.asset_of(&house.contract_id), 0, "{count} bidders");
    }
}

#[test]
fn batch_with_no_bids_returns_the_whole_supply() {
    let mut house = House::new(0);
    let id = house.batch();
    house.close_window();

    let settled = house.client().settle(&id);
    assert_eq!(settled.clearing_price, None);
    assert_eq!(settled.units_sold, 0);
    assert_eq!(house.asset_of(&house.seller), SELLER_ASSET);
}

#[test]
fn bid_without_enough_balance_fails_and_leaves_no_trace() {
    let house = House::new(1);
    let id = house.batch();
    let poor = house.unfunded();
    house.fund(&poor, 99);

    // 10 × 10 = 100 > 99 held.
    assert!(house.client().try_bid_batch(&id, &poor, &10, &10).is_err());
    assert_eq!(house.payment_of(&poor), 99);
    let auction = house.client().auction(&id);
    assert_eq!(auction.bid_count, 0);
    assert_eq!(auction.total_escrow, 0);
    assert!(house.client().book(&id).is_empty());

    // The same bidder can still bid once they can cover the escrow.
    house.fund(&poor, 1);
    house.client().bid_batch(&id, &poor, &10, &10);
    assert_eq!(house.payment_of(&poor), 0);
}

#[test]
fn dutch_bid_without_enough_balance_does_not_claim_the_lot() {
    let house = House::new(1);
    let id = house.dutch();
    let poor = house.unfunded();
    house.fund(&poor, START_PRICE - 1);

    assert!(house
        .client()
        .try_bid_dutch(&id, &poor, &START_PRICE)
        .is_err());
    assert_eq!(house.client().auction(&id).winner, None);
    // An honest bidder can still buy.
    house
        .client()
        .bid_dutch(&id, &house.bidder(0), &START_PRICE);
}
