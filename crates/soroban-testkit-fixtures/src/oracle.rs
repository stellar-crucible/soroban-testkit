//! Reusable price-oracle fixture with programmable feeds and failure states.
//!
//! Price-dependent contracts need more than a number: they need feeds with
//! independent decimals, a round sequence that moves when an update arrives,
//! timestamps that age, and the ability to fail in the specific ways a real
//! oracle fails. This module provides an on-chain [`MockPriceOracle`], a typed
//! [`MockPriceOracleClient`], and an [`OracleHarness`] that programs both the
//! stored data and the failure modes from a test.
//!
//! # Quick start
//!
//! ```rust
//! use soroban_sdk::{testutils::Ledger as _, Env, Symbol};
//! use soroban_testkit_fixtures::OracleHarness;
//!
//! let env = Env::default();
//! env.ledger().set_timestamp(1_700_000_000);
//! let mut oracle = OracleHarness::new(&env);
//!
//! oracle.set_price("BTC", 6_400_000, 2);
//! oracle.set_price("USD", 1_000_000, 6);
//!
//! assert_eq!(oracle.price("BTC").price, 6_400_000);
//! assert_eq!(oracle.price("BTC").decimals, 2);
//! assert_eq!(oracle.price("USD").decimals, 6);
//!
//! let btc = Symbol::new(&env, "BTC");
//! assert!(oracle.client().try_get_price(&btc).is_ok());
//! ```
//!
//! The harness registers the contract against the environment it is given; it
//! does not create or replace the [`Env`]. Every harness gets its own generated
//! admin address and a heartbeat of 3,600 seconds. Use [`OracleHarness::with_heartbeat`]
//! to change the freshness window before seeding prices.
//!
//! The admin address is a configuration key, not a signer: admin-gated calls
//! compare the supplied address with the stored one but do not invoke
//! `require_auth`. The harness therefore never changes the caller's auth policy.
//!
//! # Stored data and validation
//!
//! [`AssetPrice`] is the full feed record: `price`, its `decimals`, the update
//! `round`, and the `last_updated` ledger timestamp. Prices are keyed by
//! Soroban [`Symbol`] asset names, so each asset carries its own scale and its
//! own sequence. `Symbol::new` accepts names up to 32 characters; the harness
//! converts `&str` names with it.
//!
//! [`MockPriceOracle::get_price`] is the validated read. It checks, in order:
//!
//! 1. `offline` — returns [`OracleError::OracleOffline`].
//! 2. asset existence — returns [`OracleError::AssetNotFound`].
//! 3. non-positive price — returns [`OracleError::InvalidPrice`].
//! 4. heartbeat age — returns [`OracleError::StalePrice`] when
//!    `env.ledger().timestamp().saturating_sub(last_updated) > heartbeat`.
//!
//! The boundary is inclusive: a feed exactly `heartbeat` seconds old is still
//! fresh. A timestamp in the future ages as zero, so a clock that moved
//! backwards does not make a feed stale.
//!
//! [`MockPriceOracle::peek_price`] deliberately skips the offline, price-sign
//! and freshness checks. It still returns [`OracleError::AssetNotFound`] for an
//! unknown asset. Tests use it through [`OracleHarness::price`] to assert on the
//! bad state that `get_price` is about to reject.
//!
//! # Simulating failures
//!
//! The simulation helpers write a real update through the admin-gated contract
//! API at the next round and the current ledger time, so a consumer sees the
//! same shape of failure it would see on a live feed:
//!
//! ```rust
//! use soroban_sdk::{testutils::Ledger as _, Env, Symbol};
//! use soroban_testkit_fixtures::{OracleError, OracleHarness};
//!
//! let env = Env::default();
//! env.ledger().set_timestamp(1_700_000_000);
//! let mut oracle = OracleHarness::new(&env);
//! oracle.set_price("ETH", 2_000, 2);
//! let eth = Symbol::new(&env, "ETH");
//!
//! oracle.simulate_stale("ETH");
//! assert!(matches!(
//!     oracle.client().try_get_price(&eth),
//!     Err(Ok(OracleError::StalePrice))
//! ));
//!
//! oracle.set_price("ETH", 2_000, 2);
//! oracle.simulate_negative_price("ETH");
//! assert_eq!(oracle.price("ETH").price, -2_000);
//! assert!(matches!(
//!     oracle.client().try_get_price(&eth),
//!     Err(Ok(OracleError::InvalidPrice))
//! ));
//!
//! oracle.set_offline(true);
//! assert!(matches!(
//!     oracle.client().try_get_price(&eth),
//!     Err(Ok(OracleError::OracleOffline))
//! ));
//! ```
//!
//! [`OracleHarness::simulate_deviation`] takes a signed percentage: `-30`
//! publishes a 30% drop and `50` publishes a 50% rise, computed as
//! `price * (100 + percent) / 100`. [`OracleHarness::advance_last_updated_time`]
//! accepts a negative delta, which lets a test move one feed's clock backwards
//! without moving the ledger's clock or any other feed.

use std::collections::BTreeMap;

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Symbol, Vec};

const DEFAULT_HEARTBEAT_SECS: u64 = 3_600;
const MAX_SEP40_HISTORY: u32 = 32;

/// A single asset's stored price record.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetPrice {
    /// The quoted price. `get_price` rejects values at or below zero.
    pub price: i128,
    /// Decimal places this asset's quote is scaled by.
    pub decimals: u32,
    /// Monotonic update number, advanced by the harness between publishes.
    pub round: u64,
    /// Ledger timestamp of the update, used for heartbeat freshness.
    pub last_updated: u64,
}

/// SEP-40 compatible asset identifier.
///
/// `Other` maps to the oracle's `Symbol` asset name. `Stellar` is accepted by
/// the interface but has no stored feed in this mock.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Sep40Asset {
    Stellar(Address),
    Other(Symbol),
}

/// SEP-40 price record: a quote and the timestamp it was published at.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sep40PriceData {
    pub price: i128,
    pub timestamp: u64,
}

/// Errors returned by the oracle's validated read and by its raw peek.
#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum OracleError {
    /// No price has been stored for the requested asset.
    AssetNotFound = 1,
    /// The stored quote is older than the configured heartbeat.
    StalePrice = 2,
    /// The stored quote is zero or negative.
    InvalidPrice = 3,
    /// The oracle has been taken offline.
    OracleOffline = 4,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum DataKey {
    Admin,
    Heartbeat,
    Offline,
    Price(Symbol),
    Assets,
    Sep40History(Symbol),
    Sep40Decimals,
}

/// On-chain mock price oracle contract.
#[contract]
pub struct MockPriceOracle;

#[contractimpl]
impl MockPriceOracle {
    /// Install the admin, default heartbeat and online state.
    ///
    /// # Panics
    ///
    /// Panics if the contract is already initialized.
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("oracle already initialized");
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::Heartbeat, &DEFAULT_HEARTBEAT_SECS);
        env.storage().instance().set(&DataKey::Offline, &false);
        env.storage()
            .instance()
            .set(&DataKey::Assets, &Vec::<Symbol>::new(&env));
        env.storage().instance().set(&DataKey::Sep40Decimals, &7u32);
    }

    /// Publish a quote. Negative prices are accepted here so tests can make a
    /// feed invalid; [`Self::get_price`] is where they are rejected.
    ///
    /// # Panics
    ///
    /// Panics when called by anyone other than the configured admin.
    pub fn set_price(
        env: Env,
        admin: Address,
        asset: Symbol,
        price: i128,
        decimals: u32,
        round: u64,
        last_updated: u64,
    ) {
        require_admin(&env, &admin);
        env.storage().persistent().set(
            &DataKey::Price(asset.clone()),
            &AssetPrice {
                price,
                decimals,
                round,
                last_updated,
            },
        );

        let mut assets: Vec<Symbol> = env
            .storage()
            .instance()
            .get(&DataKey::Assets)
            .unwrap_or_else(|| Vec::new(&env));
        if !assets.contains(&asset) {
            assets.push_back(asset.clone());
            env.storage().instance().set(&DataKey::Assets, &assets);
        }

        let key = DataKey::Sep40History(asset);
        let mut history: Vec<Sep40PriceData> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(&env));
        history.push_back(Sep40PriceData {
            price,
            timestamp: last_updated,
        });
        if history.len() > MAX_SEP40_HISTORY {
            let mut trimmed = Vec::new(&env);
            for item in history
                .iter()
                .skip((history.len() - MAX_SEP40_HISTORY) as usize)
            {
                trimmed.push_back(item);
            }
            history = trimmed;
        }
        env.storage().persistent().set(&key, &history);
    }

    /// Return a fresh, valid quote or the first validation error.
    ///
    /// Checks run in this order: offline, existence, non-positive price, then
    /// heartbeat age against `env.ledger().timestamp()`.
    pub fn get_price(env: Env, asset: Symbol) -> Result<AssetPrice, OracleError> {
        let offline: bool = env
            .storage()
            .instance()
            .get(&DataKey::Offline)
            .unwrap_or(false);
        if offline {
            return Err(OracleError::OracleOffline);
        }

        let stored: AssetPrice = env
            .storage()
            .persistent()
            .get(&DataKey::Price(asset))
            .ok_or(OracleError::AssetNotFound)?;

        if stored.price <= 0 {
            return Err(OracleError::InvalidPrice);
        }

        let heartbeat: u64 = env
            .storage()
            .instance()
            .get(&DataKey::Heartbeat)
            .unwrap_or(DEFAULT_HEARTBEAT_SECS);
        if env.ledger().timestamp().saturating_sub(stored.last_updated) > heartbeat {
            return Err(OracleError::StalePrice);
        }

        Ok(stored)
    }

    /// Return the stored record without offline, sign or freshness validation.
    ///
    /// Only a missing asset is an error; tests use this to observe exactly the
    /// state that [`Self::get_price`] rejects.
    pub fn peek_price(env: Env, asset: Symbol) -> Result<AssetPrice, OracleError> {
        env.storage()
            .persistent()
            .get(&DataKey::Price(asset))
            .ok_or(OracleError::AssetNotFound)
    }

    /// Change the heartbeat window in seconds.
    ///
    /// # Panics
    ///
    /// Panics when called by anyone other than the configured admin.
    pub fn set_heartbeat(env: Env, admin: Address, heartbeat_secs: u64) {
        require_admin(&env, &admin);
        env.storage()
            .instance()
            .set(&DataKey::Heartbeat, &heartbeat_secs);
    }

    /// Take the oracle offline or back online.
    ///
    /// # Panics
    ///
    /// Panics when called by anyone other than the configured admin.
    pub fn set_offline(env: Env, admin: Address, offline: bool) {
        require_admin(&env, &admin);
        env.storage().instance().set(&DataKey::Offline, &offline);
    }

    /// SEP-40 base asset: every feed in this mock is quoted in USD.
    pub fn base(env: Env) -> Sep40Asset {
        Sep40Asset::Other(Symbol::new(&env, "USD"))
    }

    /// SEP-40 list of assets with at least one published feed.
    pub fn assets(env: Env) -> Vec<Sep40Asset> {
        let symbols: Vec<Symbol> = env
            .storage()
            .instance()
            .get(&DataKey::Assets)
            .unwrap_or_else(|| Vec::new(&env));
        let mut result = Vec::new(&env);
        for symbol in symbols.iter() {
            result.push_back(Sep40Asset::Other(symbol));
        }
        result
    }

    /// SEP-40 quote precision shared by the compatibility methods.
    pub fn decimals(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::Sep40Decimals)
            .unwrap_or(7)
    }

    /// SEP-40 quote precision setter.
    ///
    /// # Panics
    ///
    /// Panics when called by anyone other than the configured admin.
    pub fn set_decimals(env: Env, admin: Address, decimals: u32) {
        require_admin(&env, &admin);
        env.storage()
            .instance()
            .set(&DataKey::Sep40Decimals, &decimals);
    }

    /// SEP-40 resolution, using the oracle heartbeat as the tick period.
    pub fn resolution(env: Env) -> u32 {
        env.storage()
            .instance()
            .get::<_, u64>(&DataKey::Heartbeat)
            .unwrap_or(DEFAULT_HEARTBEAT_SECS)
            .try_into()
            .unwrap_or(u32::MAX)
    }

    /// SEP-40 price at an exact published timestamp.
    pub fn price(env: Env, asset: Sep40Asset, timestamp: u64) -> Option<Sep40PriceData> {
        let symbol = sep40_symbol(&asset)?;
        let history: Vec<Sep40PriceData> = env
            .storage()
            .persistent()
            .get(&DataKey::Sep40History(symbol))?;
        let mut matched = None;
        for record in history.iter() {
            if record.timestamp == timestamp {
                matched = Some(record);
            }
        }
        matched
    }

    /// SEP-40 last N published records, newest last.
    pub fn prices(env: Env, asset: Sep40Asset, records: u32) -> Option<Vec<Sep40PriceData>> {
        let symbol = sep40_symbol(&asset)?;
        let history: Vec<Sep40PriceData> = env
            .storage()
            .persistent()
            .get(&DataKey::Sep40History(symbol))?;
        let wanted = records.min(history.len());
        let mut result = Vec::new(&env);
        for record in history.iter().skip((history.len() - wanted) as usize) {
            result.push_back(record);
        }
        Some(result)
    }

    /// SEP-40 most recent published record.
    pub fn lastprice(env: Env, asset: Sep40Asset) -> Option<Sep40PriceData> {
        let symbol = sep40_symbol(&asset)?;
        let history: Vec<Sep40PriceData> = env
            .storage()
            .persistent()
            .get(&DataKey::Sep40History(symbol))?;
        history.last()
    }
}

fn sep40_symbol(asset: &Sep40Asset) -> Option<Symbol> {
    match asset {
        Sep40Asset::Other(symbol) => Some(symbol.clone()),
        Sep40Asset::Stellar(_) => None,
    }
}

fn require_admin(env: &Env, admin: &Address) {
    let configured: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .expect("oracle not initialized");
    assert_eq!(admin, &configured, "caller is not the oracle admin");
}

/// Builder-style driver around [`MockPriceOracle`].
///
/// The harness keeps the [`Env`] and registered contract address together with
/// a Rust-side round counter per asset. `set_price` uses that counter and the
/// ledger's current timestamp; explicit helpers can override either field.
pub struct OracleHarness {
    env: Env,
    address: Address,
    admin: Address,
    rounds: BTreeMap<String, u64>,
}

impl OracleHarness {
    /// Register and initialize an oracle on `env` with a generated admin and a
    /// 3,600-second heartbeat.
    pub fn new(env: &Env) -> Self {
        let admin = Address::generate(env);
        let address = env.register(MockPriceOracle, ());
        MockPriceOracleClient::new(env, &address).initialize(&admin);
        Self {
            env: env.clone(),
            address,
            admin,
            rounds: BTreeMap::new(),
        }
    }

    /// Replace the admin used by subsequent programmatic updates.
    ///
    /// This rewrites the contract's stored admin directly, so prices already
    /// seeded are preserved. Call it before handing the harness to test code
    /// that should not administer the feed.
    pub fn with_admin(mut self, admin: Address) -> Self {
        self.env.as_contract(&self.address, || {
            self.env.storage().instance().set(&DataKey::Admin, &admin);
        });
        self.admin = admin;
        self
    }

    /// Configure the freshness window in seconds before seeding prices.
    pub fn with_heartbeat(self, seconds: u64) -> Self {
        self.client().set_heartbeat(&self.admin, &seconds);
        self
    }

    /// Publish a fresh update at the current ledger time and next round.
    pub fn set_price(&mut self, asset: &str, price: i128, decimals: u32) {
        let round = self.next_round(asset);
        let timestamp = self.env.ledger().timestamp();
        self.set_price_with_round(asset, price, decimals, round, timestamp);
    }

    /// Publish a quote with an explicit round and timestamp.
    ///
    /// The harness's next round for this asset becomes `round + 1`.
    pub fn set_price_with_round(
        &mut self,
        asset: &str,
        price: i128,
        decimals: u32,
        round: u64,
        timestamp: u64,
    ) {
        self.client().set_price(
            &self.admin,
            &self.asset_symbol(asset),
            &price,
            &decimals,
            &round,
            &timestamp,
        );
        self.rounds.insert(asset.to_string(), round);
    }

    /// Move an asset's round forward by `rounds`, preserving price and time.
    pub fn advance_round(&mut self, asset: &str, rounds: u64) {
        let mut stored = self.price(asset);
        stored.round = stored.round.saturating_add(rounds);
        self.rewrite(asset, &stored);
        self.rounds.insert(asset.to_string(), stored.round);
    }

    /// Set an asset's round to an absolute value.
    pub fn set_round(&mut self, asset: &str, round: u64) {
        let mut stored = self.price(asset);
        stored.round = round;
        self.rewrite(asset, &stored);
        self.rounds.insert(asset.to_string(), round);
    }

    /// Set an asset's last-updated timestamp to an absolute value.
    pub fn set_last_updated(&mut self, asset: &str, timestamp: u64) {
        let mut stored = self.price(asset);
        stored.last_updated = timestamp;
        self.rewrite(asset, &stored);
    }

    /// Shift an asset's timestamp by `delta`, saturating at zero or `u64::MAX`.
    pub fn advance_last_updated_time(&mut self, asset: &str, delta: i64) {
        let mut stored = self.price(asset);
        stored.last_updated = if delta.is_negative() {
            stored.last_updated.saturating_sub(delta.unsigned_abs())
        } else {
            stored.last_updated.saturating_add(delta as u64)
        };
        self.rewrite(asset, &stored);
    }

    /// Backdate an asset just beyond its heartbeat so `get_price` rejects it.
    pub fn simulate_stale(&mut self, asset: &str) {
        let heartbeat = self.heartbeat_secs();
        let stale_at = self
            .env
            .ledger()
            .timestamp()
            .saturating_sub(heartbeat.saturating_add(1));
        self.set_last_updated(asset, stale_at);
    }

    /// Publish a negative quote for an asset.
    pub fn simulate_negative_price(&mut self, asset: &str) {
        let stored = self.price(asset);
        let invalid = if stored.price > 0 {
            stored.price.saturating_neg()
        } else if stored.price == 0 {
            -1
        } else {
            stored.price
        };
        self.set_price(asset, invalid, stored.decimals);
    }

    /// Publish a signed percentage change from the current quote.
    ///
    /// `-30` means a 30% drop; `50` means a 50% rise. A percent of `-100`
    /// produces a zero price, which `get_price` reports as invalid.
    pub fn simulate_deviation(&mut self, asset: &str, percent: i32) {
        let stored = self.price(asset);
        let factor = 100i128 + i128::from(percent);
        let changed = stored.price.saturating_mul(factor) / 100;
        self.set_price(asset, changed, stored.decimals);
    }

    /// Take the whole oracle offline or back online.
    pub fn set_offline(&mut self, offline: bool) {
        self.client().set_offline(&self.admin, &offline);
    }

    /// Peek at the raw stored record, including invalid or stale quotes.
    ///
    /// # Panics
    ///
    /// Panics with a diagnostic if the asset is not stored or cannot be
    /// decoded.
    #[track_caller]
    pub fn price(&self, asset: &str) -> AssetPrice {
        let symbol = self.asset_symbol(asset);
        match self.client().try_peek_price(&symbol) {
            Ok(Ok(stored)) => stored,
            Ok(Err(conversion)) => {
                panic!("could not decode the oracle price for '{asset}': {conversion:?}")
            }
            Err(Ok(error)) => panic!("oracle has no price for '{asset}': {error:?}"),
            Err(Err(invoke)) => panic!("peek_price failed for '{asset}': {invoke:?}"),
        }
    }

    /// A typed client bound to this harness's environment and contract.
    pub fn client(&self) -> MockPriceOracleClient<'_> {
        MockPriceOracleClient::new(&self.env, &self.address)
    }

    /// The registered oracle contract address.
    pub fn address(&self) -> &Address {
        &self.address
    }

    /// The environment the oracle was registered on.
    pub fn env(&self) -> &Env {
        &self.env
    }

    fn asset_symbol(&self, asset: &str) -> Symbol {
        Symbol::new(&self.env, asset)
    }

    fn heartbeat_secs(&self) -> u64 {
        self.env.as_contract(&self.address, || {
            self.env
                .storage()
                .instance()
                .get::<_, u64>(&DataKey::Heartbeat)
                .unwrap_or(DEFAULT_HEARTBEAT_SECS)
        })
    }

    fn next_round(&self, asset: &str) -> u64 {
        self.rounds
            .get(asset)
            .map_or(1, |round| round.saturating_add(1))
    }

    fn rewrite(&self, asset: &str, stored: &AssetPrice) {
        self.client().set_price(
            &self.admin,
            &self.asset_symbol(asset),
            &stored.price,
            &stored.decimals,
            &stored.round,
            &stored.last_updated,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl, testutils::Ledger as _, Address, Env, Symbol};

    const NOW: u64 = 1_700_000_000;

    /// A real consumer contract: its own error type is the oracle's, and its
    /// `try_read_price` exposes exactly what a caller sees from `get_price`.
    #[contract]
    pub struct OracleConsumer;

    #[contractimpl]
    impl OracleConsumer {
        pub fn read_price(
            env: Env,
            oracle: Address,
            asset: Symbol,
        ) -> Result<AssetPrice, OracleError> {
            match MockPriceOracleClient::new(&env, &oracle).try_get_price(&asset) {
                Ok(Ok(stored)) => Ok(stored),
                Err(Ok(error)) => Err(error),
                Ok(Err(_)) => panic!("oracle returned an undecodable price"),
                Err(Err(_)) => panic!("oracle invocation failed at the host level"),
            }
        }
    }

    struct Setup {
        oracle: OracleHarness,
        consumer_id: Address,
    }

    impl Setup {
        fn new() -> Self {
            let env = Env::default();
            env.ledger().set_timestamp(NOW);
            let oracle = OracleHarness::new(&env);
            let consumer_id = env.register(OracleConsumer, ());
            Self {
                oracle,
                consumer_id,
            }
        }

        fn symbol(&self, asset: &str) -> Symbol {
            Symbol::new(self.oracle.env(), asset)
        }

        fn consumer(&self) -> OracleConsumerClient<'_> {
            OracleConsumerClient::new(self.oracle.env(), &self.consumer_id)
        }
    }

    #[test]
    fn multi_asset_feeds_keep_independent_decimals_and_rounds() {
        let mut setup = Setup::new();

        setup.oracle.set_price("ORACLE_BTC", 6_400_000, 2);
        setup.oracle.set_price("ORACLE_USD", 1_000_000, 6);

        let btc = setup.oracle.price("ORACLE_BTC");
        let usd = setup.oracle.price("ORACLE_USD");
        assert_eq!(btc.price, 6_400_000);
        assert_eq!(btc.decimals, 2);
        assert_eq!(btc.round, 1);
        assert_eq!(usd.price, 1_000_000);
        assert_eq!(usd.decimals, 6);
        assert_eq!(usd.round, 1);
        assert_eq!(btc.last_updated, NOW);
        assert_eq!(usd.last_updated, NOW);

        assert_eq!(
            setup
                .consumer()
                .read_price(setup.oracle.address(), &setup.symbol("ORACLE_BTC"))
                .decimals,
            2
        );
        assert_eq!(
            setup
                .consumer()
                .read_price(setup.oracle.address(), &setup.symbol("ORACLE_USD"))
                .decimals,
            6
        );
    }

    #[test]
    fn round_advancement_and_explicit_rounds_drive_the_next_update() {
        let mut setup = Setup::new();
        setup.oracle.set_price("ORACLE_ETH", 2_000, 2);

        setup.oracle.advance_round("ORACLE_ETH", 3);
        assert_eq!(setup.oracle.price("ORACLE_ETH").round, 4);

        setup.oracle.set_round("ORACLE_ETH", 9);
        assert_eq!(setup.oracle.price("ORACLE_ETH").round, 9);

        setup.oracle.set_price("ORACLE_ETH", 2_100, 2);
        assert_eq!(setup.oracle.price("ORACLE_ETH").round, 10);

        setup
            .oracle
            .set_price_with_round("ORACLE_ETH", 2_200, 2, 42, NOW + 5);
        let explicit = setup.oracle.price("ORACLE_ETH");
        assert_eq!((explicit.round, explicit.last_updated), (42, NOW + 5));

        setup.oracle.set_price("ORACLE_ETH", 2_300, 2);
        let next = setup.oracle.price("ORACLE_ETH");
        assert_eq!(next.round, 43);
        assert_eq!(next.last_updated, NOW);
    }

    #[test]
    fn heartbeat_boundary_is_fresh_and_one_more_second_is_stale() {
        let mut setup = Setup::new();
        setup
            .oracle
            .set_price_with_round("ORACLE_TIME", 100, 2, 1, NOW);
        let symbol = setup.symbol("ORACLE_TIME");

        setup.oracle.env().ledger().set_timestamp(NOW + 3_600);
        assert!(setup.oracle.client().try_get_price(&symbol).is_ok());

        setup.oracle.env().ledger().set_timestamp(NOW + 3_601);
        assert!(matches!(
            setup.oracle.client().try_get_price(&symbol),
            Err(Ok(OracleError::StalePrice))
        ));
        assert!(matches!(
            setup
                .consumer()
                .try_read_price(setup.oracle.address(), &symbol),
            Err(Ok(OracleError::StalePrice))
        ));
    }

    #[test]
    fn custom_heartbeat_changes_when_a_feed_goes_stale() {
        let env = Env::default();
        env.ledger().set_timestamp(NOW);
        let mut oracle = OracleHarness::new(&env).with_heartbeat(60);
        oracle.set_price("ORACLE_FAST", 10, 0);
        let symbol = Symbol::new(&env, "ORACLE_FAST");

        env.ledger().set_timestamp(NOW + 60);
        assert!(oracle.client().try_get_price(&symbol).is_ok());

        env.ledger().set_timestamp(NOW + 61);
        assert!(matches!(
            oracle.client().try_get_price(&symbol),
            Err(Ok(OracleError::StalePrice))
        ));
    }

    #[test]
    fn simulate_stale_backdates_only_the_selected_feed() {
        let mut setup = Setup::new();
        setup.oracle.set_price("ORACLE_OLD", 100, 2);
        setup.oracle.set_price("ORACLE_FRESH", 200, 2);

        setup.oracle.simulate_stale("ORACLE_OLD");

        let old = setup.oracle.price("ORACLE_OLD");
        assert_eq!(old.price, 100);
        assert_eq!(old.last_updated, NOW - 3_601);
        assert_eq!(setup.oracle.price("ORACLE_FRESH").last_updated, NOW);
        assert!(matches!(
            setup
                .oracle
                .client()
                .try_get_price(&setup.symbol("ORACLE_OLD")),
            Err(Ok(OracleError::StalePrice))
        ));
        assert!(setup
            .oracle
            .client()
            .try_get_price(&setup.symbol("ORACLE_FRESH"))
            .is_ok());
    }

    #[test]
    fn timestamp_deltas_accept_negative_values_and_saturate_at_zero() {
        let mut setup = Setup::new();
        setup.oracle.set_price("ORACLE_SHIFT", 500, 2);

        setup
            .oracle
            .advance_last_updated_time("ORACLE_SHIFT", -1_200);
        assert_eq!(setup.oracle.price("ORACLE_SHIFT").last_updated, NOW - 1_200);

        setup.oracle.advance_last_updated_time("ORACLE_SHIFT", 300);
        assert_eq!(setup.oracle.price("ORACLE_SHIFT").last_updated, NOW - 900);

        setup.oracle.set_last_updated("ORACLE_SHIFT", 10);
        setup.oracle.advance_last_updated_time("ORACLE_SHIFT", -100);
        assert_eq!(setup.oracle.price("ORACLE_SHIFT").last_updated, 0);
    }

    #[test]
    fn deviation_swings_are_signed_and_use_the_next_round() {
        let mut setup = Setup::new();
        setup.oracle.set_price("ORACLE_SWING", 200, 2);

        setup.oracle.simulate_deviation("ORACLE_SWING", -30);
        let dropped = setup.oracle.price("ORACLE_SWING");
        assert_eq!(dropped.price, 140);
        assert_eq!(dropped.round, 2);

        setup.oracle.simulate_deviation("ORACLE_SWING", 50);
        let risen = setup.oracle.price("ORACLE_SWING");
        assert_eq!(risen.price, 210);
        assert_eq!(risen.round, 3);
        assert_eq!(
            setup
                .consumer()
                .read_price(setup.oracle.address(), &setup.symbol("ORACLE_SWING"))
                .price,
            210
        );
    }

    #[test]
    fn negative_price_is_peekable_but_invalid_for_consumers() {
        let mut setup = Setup::new();
        setup.oracle.set_price("ORACLE_NEG", 250, 2);

        setup.oracle.simulate_negative_price("ORACLE_NEG");

        let stored = setup.oracle.price("ORACLE_NEG");
        assert_eq!(stored.price, -250);
        assert_eq!(stored.decimals, 2);
        let symbol = setup.symbol("ORACLE_NEG");
        assert!(matches!(
            setup.oracle.client().try_get_price(&symbol),
            Err(Ok(OracleError::InvalidPrice))
        ));
        assert!(matches!(
            setup
                .consumer()
                .try_read_price(setup.oracle.address(), &symbol),
            Err(Ok(OracleError::InvalidPrice))
        ));
    }

    #[test]
    fn offline_oracle_rejects_reads_but_peek_still_works() {
        let mut setup = Setup::new();
        setup.oracle.set_price("ORACLE_OFFLINE", 100, 2);
        let symbol = setup.symbol("ORACLE_OFFLINE");

        assert_eq!(
            setup
                .consumer()
                .read_price(setup.oracle.address(), &symbol)
                .price,
            100
        );

        setup.oracle.set_offline(true);
        assert_eq!(setup.oracle.price("ORACLE_OFFLINE").price, 100);
        assert!(matches!(
            setup.oracle.client().try_get_price(&symbol),
            Err(Ok(OracleError::OracleOffline))
        ));
        assert!(matches!(
            setup
                .consumer()
                .try_read_price(setup.oracle.address(), &symbol),
            Err(Ok(OracleError::OracleOffline))
        ));

        setup.oracle.set_offline(false);
        assert_eq!(
            setup
                .consumer()
                .read_price(setup.oracle.address(), &symbol)
                .price,
            100
        );
    }

    #[test]
    fn unknown_asset_is_rejected_by_the_consumer() {
        let setup = Setup::new();
        assert!(matches!(
            setup
                .consumer()
                .try_read_price(setup.oracle.address(), &setup.symbol("ORACLE_MISSING")),
            Err(Ok(OracleError::AssetNotFound))
        ));
    }

    #[test]
    fn with_admin_replaces_the_admin_without_losing_seeded_prices() {
        let env = Env::default();
        env.ledger().set_timestamp(NOW);
        let replacement = Address::generate(&env);
        let mut oracle = OracleHarness::new(&env);
        oracle.set_price("ORACLE_ADMIN", 7, 0);

        let mut oracle = oracle.with_admin(replacement);
        oracle.set_price("ORACLE_ADMIN", 8, 0);

        assert_eq!(oracle.price("ORACLE_ADMIN").price, 8);
        assert_eq!(oracle.price("ORACLE_ADMIN").round, 2);
    }

    #[test]
    fn sep40_compatibility_exposes_assets_history_and_resolution() {
        let mut setup = Setup::new();
        setup.oracle.set_price("ORACLE_SEP40", 100, 2);
        setup.oracle.set_price("ORACLE_SEP40", 110, 2);
        setup.oracle.set_price("ORACLE_SEP40_USD", 1, 6);

        let client = setup.oracle.client();
        let asset = Sep40Asset::Other(setup.symbol("ORACLE_SEP40"));
        assert_eq!(
            client.base(),
            Sep40Asset::Other(Symbol::new(setup.oracle.env(), "USD"))
        );
        assert_eq!(client.decimals(), 7);
        assert_eq!(client.resolution(), 3_600);
        assert_eq!(client.assets().len(), 2);

        let latest = client.lastprice(&asset).expect("latest SEP-40 quote");
        assert_eq!(latest.price, 110);
        assert_eq!(latest.timestamp, NOW);
        assert_eq!(client.price(&asset, &NOW).expect("quote at NOW").price, 110);
        assert!(client.price(&asset, &(NOW - 1)).is_none());
        assert_eq!(client.prices(&asset, &1).expect("history").len(), 1);
        assert_eq!(client.prices(&asset, &8).expect("history").len(), 2);

        let stellar = Sep40Asset::Stellar(Address::generate(setup.oracle.env()));
        assert!(client.lastprice(&stellar).is_none());
    }
}
