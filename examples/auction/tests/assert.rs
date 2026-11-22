//! Auction behaviour asserted through `soroban-testkit-assert`: exact
//! contract errors for invalid, duplicate, early, late and repeated calls;
//! the token error an under-funded escrow surfaces; event payloads (clearing
//! price included); bidder authorization; and balance conservation across
//! clearing, settlement and claims.

mod common;

use common::*;
use soroban_sdk::{Address, Error};
use soroban_testkit_assert::events::EventMatcher;
use soroban_testkit_assert::{
    assert_auth_matches, assert_balance_conserved, assert_contract_error, assert_host_error,
};
use soroban_testkit_example_auction::{AuctionError, MAX_BATCH_BIDS};

// ------------------------------------------------------------- validation

#[test]
fn dutch_configs_that_break_the_schedule_are_rejected() {
    let house = House::new(0);
    let client = house.client();
    let invalid = [
        house.dutch_config(START_PRICE, 0, DECAY, 1), // zero floor
        house.dutch_config(3_000, 4_000, DECAY, 1),   // floor above start
        house.dutch_config(START_PRICE, FLOOR_PRICE, 0, 1), // no decay window
        house.dutch_config(START_PRICE, FLOOR_PRICE, DECAY, 0), // zero step
        house.dutch_config(START_PRICE, FLOOR_PRICE, DECAY, 700), // step longer than decay
        house.dutch_config(START_PRICE, FLOOR_PRICE, DECAY, 7), // floor never reached
    ];
    for config in invalid {
        assert_contract_error!(
            client.try_create_dutch(&house.seller, &config),
            AuctionError::InvalidConfig
        );
    }
    let mut empty_lot = house.dutch_config(START_PRICE, FLOOR_PRICE, DECAY, 1);
    empty_lot.lot_size = 0;
    assert_contract_error!(
        client.try_create_dutch(&house.seller, &empty_lot),
        AuctionError::InvalidConfig
    );
}

#[test]
fn dutch_schedule_whose_price_math_could_overflow_is_rejected() {
    let house = House::new(0);
    let config = house.dutch_config(i128::MAX, 1, u64::from(u32::MAX), 1);
    assert_contract_error!(
        house.client().try_create_dutch(&house.seller, &config),
        AuctionError::ArithmeticOverflow
    );
}

#[test]
fn batch_configs_without_supply_reserve_or_window_are_rejected() {
    let house = House::new(0);
    let client = house.client();
    let mut no_window = house.batch_config(SUPPLY, RESERVE);
    no_window.end_time = no_window.start_time;
    for config in [
        house.batch_config(0, RESERVE),
        house.batch_config(SUPPLY, 0),
        house.batch_config(-5, RESERVE),
        no_window,
    ] {
        assert_contract_error!(
            client.try_create_batch(&house.seller, &config),
            AuctionError::InvalidConfig
        );
    }
}

#[test]
fn unknown_auction_and_wrong_kind_are_distinguished() {
    let house = House::new(1);
    let dutch = house.dutch();
    let batch = house.batch();
    let client = house.client();

    assert_contract_error!(client.try_auction(&99), AuctionError::AuctionNotFound);
    assert_contract_error!(
        client.try_bid_batch(&dutch, &house.bidder(0), &1, &RESERVE),
        AuctionError::WrongAuctionKind
    );
    assert_contract_error!(
        client.try_bid_dutch(&batch, &house.bidder(0), &START_PRICE),
        AuctionError::WrongAuctionKind
    );
    assert_contract_error!(
        client.try_current_price(&batch),
        AuctionError::WrongAuctionKind
    );
}

// ------------------------------------------------------------ Dutch rules

#[test]
fn dutch_bid_below_the_asking_price_is_refused() {
    let mut house = House::new(1);
    let id = house.dutch();
    house.advance(100); // asking 9_000
    assert_contract_error!(
        house.client().try_bid_dutch(&id, &house.bidder(0), &8_999),
        AuctionError::BidBelowPrice
    );
}

#[test]
fn only_the_first_valid_dutch_bid_wins() {
    let mut house = House::new(2);
    let id = house.dutch();
    house.advance(10);
    house
        .client()
        .bid_dutch(&id, &house.bidder(0), &START_PRICE);
    assert_contract_error!(
        house
            .client()
            .try_bid_dutch(&id, &house.bidder(1), &START_PRICE),
        AuctionError::LotAlreadySold
    );
    assert_eq!(house.payment_of(&house.bidder(1)), BIDDER_FUNDS);
}

#[test]
fn dutch_bids_outside_the_window_are_refused() {
    let mut house = House::new(1);
    let mut future = house.dutch_config(START_PRICE, FLOOR_PRICE, DECAY, 1);
    future.start_time = T0 + 100;
    let id = house.client().create_dutch(&house.seller, &future);

    assert_contract_error!(
        house
            .client()
            .try_bid_dutch(&id, &house.bidder(0), &START_PRICE),
        AuctionError::NotStarted
    );
    house.advance(100 + DECAY + 1);
    assert_contract_error!(
        house
            .client()
            .try_bid_dutch(&id, &house.bidder(0), &START_PRICE),
        AuctionError::BiddingClosed
    );
}

#[test]
fn dutch_settlement_happens_exactly_once() {
    let mut house = House::new(1);
    let id = house.dutch();
    assert_contract_error!(house.client().try_settle(&id), AuctionError::BiddingOpen);

    house.advance(50);
    house
        .client()
        .bid_dutch(&id, &house.bidder(0), &START_PRICE);
    house.client().settle(&id);
    assert_contract_error!(house.client().try_settle(&id), AuctionError::AlreadySettled);
    assert_contract_error!(
        house
            .client()
            .try_bid_dutch(&id, &house.bidder(0), &START_PRICE),
        AuctionError::AlreadySettled
    );
    // The second attempt moved nothing.
    assert_eq!(house.asset_of(&house.bidder(0)), LOT);
    assert_eq!(house.payment_of(&house.seller), 9_500);
}

#[test]
fn seller_cannot_bid_on_their_own_auction() {
    let house = House::new(0);
    let dutch = house.dutch();
    let batch = house.batch();
    house.fund(&house.seller, BIDDER_FUNDS);
    assert_contract_error!(
        house
            .client()
            .try_bid_dutch(&dutch, &house.seller, &START_PRICE),
        AuctionError::SellerCannotBid
    );
    assert_contract_error!(
        house
            .client()
            .try_bid_batch(&batch, &house.seller, &1, &RESERVE),
        AuctionError::SellerCannotBid
    );
}

// ------------------------------------------------------------ Batch rules

#[test]
fn batch_bids_with_bad_quantity_or_price_are_refused() {
    let house = House::new(1);
    let id = house.batch();
    let bidder = house.bidder(0);
    let client = house.client();
    assert_contract_error!(
        client.try_bid_batch(&id, &bidder, &0, &RESERVE),
        AuctionError::InvalidQuantity
    );
    assert_contract_error!(
        client.try_bid_batch(&id, &bidder, &-1, &RESERVE),
        AuctionError::InvalidQuantity
    );
    assert_contract_error!(
        client.try_bid_batch(&id, &bidder, &1, &(RESERVE - 1)),
        AuctionError::BidBelowPrice
    );
    // Exactly the reserve is accepted.
    client.bid_batch(&id, &bidder, &1, &RESERVE);
}

#[test]
fn a_bidder_may_place_only_one_batch_bid() {
    let house = House::new(1);
    let id = house.batch();
    house.client().bid_batch(&id, &house.bidder(0), &10, &20);
    assert_contract_error!(
        house.client().try_bid_batch(&id, &house.bidder(0), &5, &50),
        AuctionError::DuplicateBid
    );
    assert_eq!(house.payment_of(&house.bidder(0)), BIDDER_FUNDS - 200);
    assert_eq!(house.client().auction(&id).bid_count, 1);
}

#[test]
fn escrow_that_would_overflow_is_refused_before_any_transfer() {
    let house = House::new(1);
    let id = house.batch();
    assert_contract_error!(
        house
            .client()
            .try_bid_batch(&id, &house.bidder(0), &i128::MAX, &RESERVE),
        AuctionError::ArithmeticOverflow
    );
    assert_eq!(house.payment_of(&house.bidder(0)), BIDDER_FUNDS);
}

#[test]
fn insufficient_balance_surfaces_the_token_error_not_an_auction_error() {
    let house = House::new(0);
    let id = house.batch();
    let poor = house.unfunded();
    // The Stellar Asset Contract refuses with its BalanceError, code 10.
    assert_host_error!(
        house.client().try_bid_batch(&id, &poor, &10, &10),
        Error::from_contract_error(10)
    );
}

#[test]
fn batch_window_boundaries_and_repeated_settlement() {
    let mut house = House::new(2);
    let id = house.batch();
    assert_contract_error!(house.client().try_settle(&id), AuctionError::BiddingOpen);

    house.advance(WINDOW - 1);
    house.client().bid_batch(&id, &house.bidder(0), &10, &20); // last open second
    house.advance(1);
    assert_contract_error!(
        house
            .client()
            .try_bid_batch(&id, &house.bidder(1), &10, &20),
        AuctionError::BiddingClosed
    );

    house.client().settle(&id);
    assert_contract_error!(house.client().try_settle(&id), AuctionError::AlreadySettled);
    assert_eq!(
        house.asset_of(&house.bidder(0)),
        0,
        "units wait for the claim"
    );
    house.client().claim(&id, &house.bidder(0));
    assert_eq!(house.asset_of(&house.bidder(0)), 10);
}

#[test]
fn each_bid_is_claimed_once_and_only_after_settlement() {
    let mut house = House::new(2);
    let id = house.batch();
    let dutch = house.dutch();
    house.place_bids(id, &[(10, 20)]);
    let client = house.client();

    assert_contract_error!(
        client.try_claim(&id, &house.bidder(0)),
        AuctionError::NotSettled
    );
    house.close_window();
    let client = house.client();
    client.settle(&id);

    assert_contract_error!(
        client.try_claim(&id, &house.bidder(1)),
        AuctionError::BidNotFound
    );
    assert_contract_error!(
        client.try_claim(&dutch, &house.bidder(0)),
        AuctionError::WrongAuctionKind
    );

    let first = client.claim(&id, &house.bidder(0));
    assert!(first.claimed);
    assert_contract_error!(
        client.try_claim(&id, &house.bidder(0)),
        AuctionError::AlreadyClaimed
    );
    // The second attempt paid nothing more.
    assert_eq!(house.asset_of(&house.bidder(0)), 10);
    assert_eq!(
        house.payment_of(&house.bidder(0)),
        BIDDER_FUNDS - 10 * RESERVE
    );
}

#[test]
fn the_order_book_is_capped() {
    let house = House::new(MAX_BATCH_BIDS as usize + 1);
    let id = house.batch();
    for index in 0..MAX_BATCH_BIDS as usize {
        house
            .client()
            .bid_batch(&id, &house.bidder(index), &1, &RESERVE);
    }
    assert_contract_error!(
        house
            .client()
            .try_bid_batch(&id, &house.bidder(MAX_BATCH_BIDS as usize), &1, &RESERVE),
        AuctionError::TooManyBids
    );
}

// ------------------------------------------------------ events and auth

#[test]
fn bid_and_settlement_events_carry_the_clearing_price() {
    let mut house = House::new(2);
    let id = house.batch_with(100, RESERVE);

    house.client().bid_batch(&id, &house.bidder(0), &80, &40);
    EventMatcher::new(&house.ctx.env)
        .from_contract(&house.contract_id)
        .with_topic("batch_bid")
        .assert_data_matches(|data| {
            data.field("quantity").and_then(|v| v.deserialize::<i128>()) == Some(80)
                && data.field("escrow").and_then(|v| v.deserialize::<i128>()) == Some(3_200)
        });

    house.client().bid_batch(&id, &house.bidder(1), &50, &25);
    house.close_window();
    house.client().settle(&id);
    EventMatcher::new(&house.ctx.env)
        .from_contract(&house.contract_id)
        .with_topic("settled")
        .assert_data_matches(|data| {
            data.field("clearing_price")
                .and_then(|v| v.deserialize::<i128>())
                == Some(25)
                && data
                    .field("units_sold")
                    .and_then(|v| v.deserialize::<i128>())
                    == Some(100)
                && data.field("proceeds").and_then(|v| v.deserialize::<i128>()) == Some(2_500)
        });
}

#[test]
fn dutch_bid_event_records_the_price_paid() {
    let mut house = House::new(1);
    let id = house.dutch();
    house.advance(250);
    house
        .client()
        .bid_dutch(&id, &house.bidder(0), &START_PRICE);
    EventMatcher::new(&house.ctx.env)
        .from_contract(&house.contract_id)
        .with_topic("dutch_bid")
        .assert_data_matches(|data| {
            data.field("price").and_then(|v| v.deserialize::<i128>()) == Some(7_500)
        });
}

#[test]
fn claim_event_reports_the_allocation_payment_and_refund() {
    let mut house = House::new(1);
    let id = house.batch_with(50, RESERVE);
    house.place_bids(id, &[(80, 30)]);
    house.close_window();
    house.client().settle(&id);
    house.client().claim(&id, &house.bidder(0));
    EventMatcher::new(&house.ctx.env)
        .from_contract(&house.contract_id)
        .with_topic("claimed")
        .assert_data_matches(|data| {
            data.field("allocation")
                .and_then(|v| v.deserialize::<i128>())
                == Some(50)
                && data.field("payment").and_then(|v| v.deserialize::<i128>()) == Some(1_500)
                && data.field("refund").and_then(|v| v.deserialize::<i128>()) == Some(900)
        });
}

#[test]
fn bids_require_the_bidders_authorization() {
    let house = House::new(1);
    let id = house.batch();
    house
        .client()
        .bid_batch(&id, &house.bidder(0), &5, &RESERVE);
    assert_auth_matches!(house.ctx.env, {
        invoker: house.bidder(0),
        function: "bid_batch",
    });

    let dutch = house.dutch();
    assert_auth_matches!(house.ctx.env, {
        invoker: house.seller.clone(),
        function: "create_dutch",
    });
    house
        .client()
        .bid_dutch(&dutch, &house.bidder(0), &START_PRICE);
    assert_auth_matches!(house.ctx.env, {
        invoker: house.bidder(0),
        function: "bid_dutch",
    });
}

// --------------------------------------------------------- conservation

#[test]
fn settlement_and_claims_conserve_both_tokens() {
    let mut house = House::new(4);
    let id = house.batch();
    house.place_bids(id, &[(400, 30), (500, 25), (300, 20), (200, 15)]);
    house.close_window();

    let mut accounts: Vec<(Address, String)> = vec![
        (house.contract_id.clone(), "auction".to_string()),
        (house.seller.clone(), "seller".to_string()),
    ];
    for (index, bidder) in house.bidders().into_iter().enumerate() {
        accounts.push((bidder, format!("bidder {index}")));
    }

    // Payment tokens: settlement moves the proceeds from escrow to the seller.
    let payment = house.payment_token();
    let settled = assert_balance_conserved!(
        accounts: accounts.clone(),
        balance: |addr: &Address| payment.balance(addr),
        call: { house.client().settle(&id) }
    );
    assert_eq!(settled.clearing_price, Some(20));

    // Payment tokens again: the claims move every refund out of escrow.
    assert_balance_conserved!(
        accounts: accounts.clone(),
        balance: |addr: &Address| payment.balance(addr),
        call: { house.claim_all(id, 4) }
    );

    // Asset units: the claims deliver every allocation.
    let mut again = House::new(4);
    let second = again.batch();
    again.place_bids(second, &[(400, 30), (500, 25), (300, 20), (200, 15)]);
    again.close_window();
    again.client().settle(&second);
    let mut asset_accounts: Vec<(Address, String)> = vec![
        (again.contract_id.clone(), "auction".to_string()),
        (again.seller.clone(), "seller".to_string()),
    ];
    for (index, bidder) in again.bidders().into_iter().enumerate() {
        asset_accounts.push((bidder, format!("bidder {index}")));
    }
    let asset = again.asset_token();
    assert_balance_conserved!(
        accounts: asset_accounts,
        balance: |addr: &Address| asset.balance(addr),
        call: { again.claim_all(second, 4) }
    );

    // Nothing is left behind in escrow.
    assert_eq!(house.payment_of(&house.contract_id), 0);
    assert_eq!(again.asset_of(&again.contract_id), 0);
}

#[test]
fn no_winner_pays_more_than_their_maximum_bid() {
    let mut house = House::new(6);
    let id = house.batch_with(700, RESERVE);
    house.place_bids(
        id,
        &[
            (200, 90),
            (200, 15),
            (200, 40),
            (200, 40),
            (200, 12),
            (200, 75),
        ],
    );
    house.close_window();
    let settled = house.client().settle(&id);
    let clearing = settled.clearing_price.unwrap();

    for index in 0..6 {
        let bid = house.bid_of(id, index);
        if bid.allocation > 0 {
            assert!(clearing <= bid.max_price, "bidder {index}");
        }
        assert!(
            bid.payment <= bid.allocation * bid.max_price,
            "bidder {index}"
        );
        assert_eq!(bid.payment + bid.refund, bid.escrow, "bidder {index}");
    }
}
