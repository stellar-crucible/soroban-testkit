#![no_std]

use soroban_sdk::{contract, contractevent, contractimpl, contracttype, Address, Env, Map, Symbol};

/// Price precision (7 decimals, matching Stellar stroops: $1.00 == 10_000_000).
pub const PRICE_SCALE: i128 = 10_000_000;

/// Basis-point denominator (100% == 10_000 bps).
pub const BPS_SCALE: i128 = 10_000;

/// Default minimum collateralization ratio (150% == 15_000 bps).
pub const DEFAULT_MCR_BPS: i128 = 15_000;

/// Minimum algorithmic redemption fee floor (0.50% == 50 bps).
pub const BASE_REDEMPTION_FEE_BPS: i128 = 50;

/// Maximum algorithmic redemption fee cap (5.00% == 500 bps).
pub const MAX_REDEMPTION_FEE_BPS: i128 = 500;

/// Base-rate decay cadence: 1 bps of decay per elapsed hour.
pub const FEE_DECAY_WINDOW_SECS: u64 = 3_600;

/// A borrower's collateralized debt position for a single collateral asset.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vault {
    pub collateral: i128,
    pub debt: i128,
}

/// Emitted by [`PriceOracleContract`] whenever an asset price is updated.
#[contractevent(topics = ["oracle", "price"])]
#[derive(Clone)]
pub struct PriceUpdated {
    pub asset: Symbol,
    pub price: i128,
}

/// Emitted when collateral is added to a borrower's vault.
#[contractevent(topics = ["stable", "deposit"])]
#[derive(Clone)]
pub struct CollateralDeposited {
    pub owner: Address,
    pub asset: Symbol,
    pub amount: i128,
    pub total_collateral: i128,
}

/// Emitted when collateral is withdrawn from a borrower's vault.
#[contractevent(topics = ["stable", "withdraw"])]
#[derive(Clone)]
pub struct CollateralWithdrawn {
    pub owner: Address,
    pub asset: Symbol,
    pub amount: i128,
    pub remaining_collateral: i128,
}

/// Emitted when stablecoins are minted against a collateralized vault.
#[contractevent(topics = ["stable", "mint"])]
#[derive(Clone)]
pub struct StablecoinMinted {
    pub owner: Address,
    pub asset: Symbol,
    pub amount: i128,
    pub total_debt: i128,
}

/// Emitted when a borrower repays stablecoin debt.
#[contractevent(topics = ["stable", "repay"])]
#[derive(Clone)]
pub struct DebtRepaid {
    pub owner: Address,
    pub asset: Symbol,
    pub amount: i128,
    pub remaining_debt: i128,
}

/// Emitted when stablecoins are staked into the Stability Pool.
#[contractevent(topics = ["stable", "sp_stake"])]
#[derive(Clone)]
pub struct StabilityStaked {
    pub staker: Address,
    pub amount: i128,
    pub pool_total: i128,
}

/// Emitted when stablecoins are unstaked from the Stability Pool.
#[contractevent(topics = ["stable", "sp_unstake"])]
#[derive(Clone)]
pub struct StabilityUnstaked {
    pub staker: Address,
    pub amount: i128,
    pub pool_total: i128,
}

/// Emitted when an under-collateralized vault is absorbed by the Stability Pool.
#[contractevent(topics = ["stable", "liquidate"])]
#[derive(Clone)]
pub struct VaultLiquidated {
    pub borrower: Address,
    pub liquidator: Address,
    pub asset: Symbol,
    pub debt_absorbed: i128,
    pub collateral_seized: i128,
}

/// Emitted when stablecoins are redeemed directly against a vault for collateral.
#[contractevent(topics = ["stable", "redeem"])]
#[derive(Clone)]
pub struct StablecoinRedeemed {
    pub redeemer: Address,
    pub borrower: Address,
    pub asset: Symbol,
    pub amount: i128,
    pub collateral_out: i128,
    pub fee_bps: i128,
}

fn oracle_admin_key(env: &Env) -> Symbol {
    Symbol::new(env, "admin")
}

fn oracle_prices_key(env: &Env) -> Symbol {
    Symbol::new(env, "prices")
}

/// On-chain price oracle serving USD prices (scaled by [`PRICE_SCALE`]) for
/// registered collateral assets.
#[contract]
pub struct PriceOracleContract;

#[contractimpl]
impl PriceOracleContract {
    /// Initializes the oracle with an authorized price updater.
    pub fn initialize(env: Env, admin: Address) {
        admin.require_auth();
        let key = oracle_admin_key(&env);
        assert!(
            !env.storage().instance().has(&key),
            "oracle already initialized"
        );
        env.storage().instance().set(&key, &admin);
    }

    /// Returns the oracle administrator.
    pub fn admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&oracle_admin_key(&env))
            .expect("oracle not initialized")
    }

    /// Records a new positive USD price (scaled by [`PRICE_SCALE`]) for `asset`.
    pub fn set_price(env: Env, asset: Symbol, price: i128) {
        Self::admin(env.clone()).require_auth();
        assert!(price > 0, "price must be positive");

        let mut prices: Map<Symbol, i128> = env
            .storage()
            .persistent()
            .get(&oracle_prices_key(&env))
            .unwrap_or_else(|| Map::new(&env));
        prices.set(asset.clone(), price);
        env.storage()
            .persistent()
            .set(&oracle_prices_key(&env), &prices);

        PriceUpdated { asset, price }.publish(&env);
    }

    /// Returns the latest recorded price for `asset`.
    pub fn price(env: Env, asset: Symbol) -> i128 {
        let prices: Map<Symbol, i128> = env
            .storage()
            .persistent()
            .get(&oracle_prices_key(&env))
            .unwrap_or_else(|| Map::new(&env));
        prices.get(asset).expect("price not set")
    }
}

fn admin_key(env: &Env) -> Symbol {
    Symbol::new(env, "admin")
}

fn oracle_key(env: &Env) -> Symbol {
    Symbol::new(env, "oracle")
}

fn mcr_key(env: &Env) -> Symbol {
    Symbol::new(env, "mcr_bps")
}

fn supply_key(env: &Env) -> Symbol {
    Symbol::new(env, "supply")
}

fn sp_total_key(env: &Env) -> Symbol {
    Symbol::new(env, "sp_total")
}

fn base_rate_key(env: &Env) -> Symbol {
    Symbol::new(env, "base_rate")
}

fn last_fee_at_key(env: &Env) -> Symbol {
    Symbol::new(env, "last_fee_at")
}

fn assets_key(env: &Env) -> Symbol {
    Symbol::new(env, "assets")
}

fn balances_key(env: &Env) -> Symbol {
    Symbol::new(env, "balances")
}

fn vaults_key(env: &Env) -> Symbol {
    Symbol::new(env, "vaults")
}

fn pool_stakes_key(env: &Env) -> Symbol {
    Symbol::new(env, "pool_stakes")
}

fn pool_collateral_key(env: &Env) -> Symbol {
    Symbol::new(env, "pool_collateral")
}

fn cache_liq_ts_key(env: &Env) -> Symbol {
    Symbol::new(env, "cache_liq_ts")
}

fn cache_fee_bps_key(env: &Env) -> Symbol {
    Symbol::new(env, "cache_fee_bps")
}

fn supply(env: &Env) -> i128 {
    env.storage().instance().get(&supply_key(env)).unwrap_or(0)
}

fn set_supply(env: &Env, value: i128) {
    env.storage().instance().set(&supply_key(env), &value);
}

fn sp_total(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&sp_total_key(env))
        .unwrap_or(0)
}

fn set_sp_total(env: &Env, value: i128) {
    env.storage().instance().set(&sp_total_key(env), &value);
}

fn assets_map(env: &Env) -> Map<Symbol, i128> {
    env.storage()
        .persistent()
        .get(&assets_key(env))
        .unwrap_or_else(|| Map::new(env))
}

fn balances_map(env: &Env) -> Map<Address, i128> {
    env.storage()
        .persistent()
        .get(&balances_key(env))
        .unwrap_or_else(|| Map::new(env))
}

fn write_balances(env: &Env, map: &Map<Address, i128>) {
    env.storage().persistent().set(&balances_key(env), map);
}

fn vaults_map(env: &Env) -> Map<(Address, Symbol), Vault> {
    env.storage()
        .persistent()
        .get(&vaults_key(env))
        .unwrap_or_else(|| Map::new(env))
}

fn write_vaults(env: &Env, map: &Map<(Address, Symbol), Vault>) {
    env.storage().persistent().set(&vaults_key(env), map);
}

fn pool_stakes_map(env: &Env) -> Map<Address, i128> {
    env.storage()
        .persistent()
        .get(&pool_stakes_key(env))
        .unwrap_or_else(|| Map::new(env))
}

fn write_pool_stakes(env: &Env, map: &Map<Address, i128>) {
    env.storage().persistent().set(&pool_stakes_key(env), map);
}

fn pool_collateral_map(env: &Env) -> Map<Symbol, i128> {
    env.storage()
        .persistent()
        .get(&pool_collateral_key(env))
        .unwrap_or_else(|| Map::new(env))
}

fn write_pool_collateral(env: &Env, map: &Map<Symbol, i128>) {
    env.storage()
        .persistent()
        .set(&pool_collateral_key(env), map);
}

fn empty_vault() -> Vault {
    Vault {
        collateral: 0,
        debt: 0,
    }
}

fn collateral_value(collateral: i128, price: i128) -> i128 {
    let scaled = collateral.checked_mul(price).expect("value overflow");
    scaled / PRICE_SCALE
}

fn is_healthy(collateral: i128, debt: i128, price: i128, mcr_bps: i128) -> bool {
    if debt == 0 {
        return true;
    }
    let value = collateral_value(collateral, price);
    let lhs = value.checked_mul(BPS_SCALE).expect("ratio overflow");
    let rhs = debt.checked_mul(mcr_bps).expect("debt threshold overflow");
    lhs >= rhs
}

fn decayed_base_rate(env: &Env) -> i128 {
    let stored: i128 = env
        .storage()
        .instance()
        .get(&base_rate_key(env))
        .unwrap_or(0);
    let last_at: u64 = env
        .storage()
        .instance()
        .get(&last_fee_at_key(env))
        .unwrap_or(0);
    let elapsed = env.ledger().timestamp().saturating_sub(last_at);
    let decay = i128::from(elapsed / FEE_DECAY_WINDOW_SECS);
    (stored - decay).max(0)
}

fn compute_redemption_fee(env: &Env, amount: i128) -> (i128, i128) {
    let decayed = decayed_base_rate(env);
    let current_supply = supply(env);
    if current_supply <= 0 || amount <= 0 {
        let fee = (BASE_REDEMPTION_FEE_BPS + decayed).min(MAX_REDEMPTION_FEE_BPS);
        return (decayed, fee);
    }

    let numerator = amount.checked_mul(BPS_SCALE).expect("fee overflow");
    let denom = current_supply.checked_mul(2).expect("supply overflow");
    let rate_increase = numerator / denom;
    let max_base = MAX_REDEMPTION_FEE_BPS - BASE_REDEMPTION_FEE_BPS;
    let next_base = decayed.saturating_add(rate_increase).min(max_base);
    let fee_bps = (BASE_REDEMPTION_FEE_BPS + next_base).min(MAX_REDEMPTION_FEE_BPS);
    (next_base, fee_bps)
}

fn fetch_oracle_price(env: &Env, asset: &Symbol) -> i128 {
    let oracle_id: Address = env
        .storage()
        .instance()
        .get(&oracle_key(env))
        .expect("stablecoin not initialized");
    PriceOracleContractClient::new(env, &oracle_id).price(asset)
}

/// Multi-asset over-collateralized stablecoin with Stability Pool debt
/// absorption, algorithmic redemption fees, and oracle-driven liquidations.
#[contract]
pub struct StablecoinContract;

#[contractimpl]
impl StablecoinContract {
    /// Initializes protocol governance, the price oracle address, and the
    /// global minimum collateralization ratio floor (`mcr_bps`).
    pub fn initialize(env: Env, admin: Address, oracle: Address, mcr_bps: i128) {
        admin.require_auth();
        let key = admin_key(&env);
        assert!(
            !env.storage().instance().has(&key),
            "stablecoin already initialized"
        );
        assert!(mcr_bps >= BPS_SCALE, "MCR must be at least 100%");

        env.storage().instance().set(&key, &admin);
        env.storage().instance().set(&oracle_key(&env), &oracle);
        env.storage().instance().set(&mcr_key(&env), &mcr_bps);
        set_supply(&env, 0);
        set_sp_total(&env, 0);
        env.storage().instance().set(&base_rate_key(&env), &0i128);
        env.storage()
            .instance()
            .set(&last_fee_at_key(&env), &env.ledger().timestamp());
    }

    /// Registers a collateral asset with its own MCR (at least the global MCR).
    pub fn register_collateral(env: Env, asset: Symbol, asset_mcr_bps: i128) {
        Self::admin(env.clone()).require_auth();
        let global_mcr = Self::mcr_bps(env.clone());
        assert!(
            asset_mcr_bps >= global_mcr,
            "asset MCR cannot be below global MCR"
        );

        let mut assets = assets_map(&env);
        assets.set(asset, asset_mcr_bps);
        env.storage().persistent().set(&assets_key(&env), &assets);
    }

    /// Returns the protocol administrator.
    pub fn admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&admin_key(&env))
            .expect("stablecoin not initialized")
    }

    /// Returns the configured [`PriceOracleContract`] address.
    pub fn oracle(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&oracle_key(&env))
            .expect("stablecoin not initialized")
    }

    /// Returns the global minimum collateralization ratio in basis points.
    pub fn mcr_bps(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&mcr_key(&env))
            .expect("stablecoin not initialized")
    }

    /// Returns the per-asset minimum collateralization ratio in basis points.
    pub fn asset_mcr_bps(env: Env, asset: Symbol) -> i128 {
        assets_map(&env)
            .get(asset)
            .expect("collateral asset not registered")
    }

    /// Returns total outstanding stablecoin supply across all active vaults.
    pub fn total_supply(env: Env) -> i128 {
        supply(&env)
    }

    /// Returns the stablecoin balance held by `owner`.
    pub fn balance_of(env: Env, owner: Address) -> i128 {
        balances_map(&env).get(owner).unwrap_or(0)
    }

    /// Returns the collateral and debt position for `(owner, asset)`.
    pub fn vault(env: Env, owner: Address, asset: Symbol) -> Vault {
        let key = (owner, asset);
        vaults_map(&env).get(key).unwrap_or_else(empty_vault)
    }

    /// Returns the current collateralization ratio in basis points for a vault,
    /// or `i128::MAX` when the vault has no debt.
    pub fn collateral_ratio_bps(env: Env, owner: Address, asset: Symbol) -> i128 {
        let position = Self::vault(env.clone(), owner, asset.clone());
        if position.debt == 0 {
            return i128::MAX;
        }
        let price = fetch_oracle_price(&env, &asset);
        let value = collateral_value(position.collateral, price);
        let scaled = value.checked_mul(BPS_SCALE).expect("ratio overflow");
        scaled / position.debt
    }

    /// Returns total stablecoins currently staked in the Stability Pool.
    pub fn stability_pool_total(env: Env) -> i128 {
        sp_total(&env)
    }

    /// Returns the recorded Stability Pool stake for `staker`.
    pub fn stability_stake_of(env: Env, staker: Address) -> i128 {
        pool_stakes_map(&env).get(staker).unwrap_or(0)
    }

    /// Returns total seized collateral held by the Stability Pool for `asset`.
    pub fn pool_collateral_of(env: Env, asset: Symbol) -> i128 {
        pool_collateral_map(&env).get(asset).unwrap_or(0)
    }

    /// Returns the current decayed algorithmic base rate in basis points.
    pub fn base_rate_bps(env: Env) -> i128 {
        decayed_base_rate(&env)
    }

    /// Quotes the algorithmic redemption fee in basis points for `amount`.
    pub fn quote_redemption_fee_bps(env: Env, amount: i128) -> i128 {
        compute_redemption_fee(&env, amount).1
    }

    /// Deposits `amount` of a registered collateral `asset` into `owner`'s vault.
    pub fn deposit_collateral(env: Env, owner: Address, asset: Symbol, amount: i128) -> Vault {
        owner.require_auth();
        assert!(amount > 0, "amount must be positive");
        let _ = Self::asset_mcr_bps(env.clone(), asset.clone());

        let mut vaults = vaults_map(&env);
        let key = (owner.clone(), asset.clone());
        let mut position = vaults.get(key.clone()).unwrap_or_else(empty_vault);
        position.collateral = position
            .collateral
            .checked_add(amount)
            .expect("collateral overflow");
        vaults.set(key, position.clone());
        write_vaults(&env, &vaults);

        CollateralDeposited {
            owner,
            asset,
            amount,
            total_collateral: position.collateral,
        }
        .publish(&env);
        position
    }

    /// Withdraws `amount` of collateral from `owner`'s vault while maintaining
    /// the asset's minimum collateralization ratio.
    pub fn withdraw_collateral(env: Env, owner: Address, asset: Symbol, amount: i128) -> Vault {
        owner.require_auth();
        assert!(amount > 0, "amount must be positive");

        let mut vaults = vaults_map(&env);
        let key = (owner.clone(), asset.clone());
        let mut position = vaults.get(key.clone()).unwrap_or_else(empty_vault);
        assert!(position.collateral >= amount, "insufficient collateral");
        position.collateral -= amount;

        if position.debt > 0 {
            let asset_mcr = Self::asset_mcr_bps(env.clone(), asset.clone());
            let price = fetch_oracle_price(&env, &asset);
            assert!(
                is_healthy(position.collateral, position.debt, price, asset_mcr),
                "withdrawal breaches MCR"
            );
        }

        vaults.set(key, position.clone());
        write_vaults(&env, &vaults);

        CollateralWithdrawn {
            owner,
            asset,
            amount,
            remaining_collateral: position.collateral,
        }
        .publish(&env);
        position
    }

    /// Mints `amount` of stablecoin against `owner`'s collateral vault if the
    /// resulting collateralization ratio remains at or above the asset MCR.
    pub fn mint(env: Env, owner: Address, asset: Symbol, amount: i128) -> i128 {
        owner.require_auth();
        assert!(amount > 0, "amount must be positive");

        let asset_mcr = Self::asset_mcr_bps(env.clone(), asset.clone());
        let price = fetch_oracle_price(&env, &asset);

        let mut vaults = vaults_map(&env);
        let key = (owner.clone(), asset.clone());
        let mut position = vaults.get(key.clone()).unwrap_or_else(empty_vault);
        let next_debt = position.debt.checked_add(amount).expect("debt overflow");
        assert!(
            is_healthy(position.collateral, next_debt, price, asset_mcr),
            "mint breaches MCR"
        );
        position.debt = next_debt;
        vaults.set(key, position);
        write_vaults(&env, &vaults);

        let mut balances = balances_map(&env);
        let held = balances.get(owner.clone()).unwrap_or(0);
        balances.set(owner.clone(), held + amount);
        write_balances(&env, &balances);

        let next_supply = supply(&env) + amount;
        set_supply(&env, next_supply);

        StablecoinMinted {
            owner,
            asset,
            amount,
            total_debt: next_debt,
        }
        .publish(&env);
        next_supply
    }

    /// Repays `amount` of stablecoin debt on `owner`'s vault and burns the
    /// corresponding supply.
    pub fn repay(env: Env, owner: Address, asset: Symbol, amount: i128) -> i128 {
        owner.require_auth();
        assert!(amount > 0, "amount must be positive");

        let mut balances = balances_map(&env);
        let held = balances.get(owner.clone()).unwrap_or(0);
        assert!(held >= amount, "insufficient stablecoin balance");

        let mut vaults = vaults_map(&env);
        let key = (owner.clone(), asset.clone());
        let mut position = vaults.get(key.clone()).unwrap_or_else(empty_vault);
        assert!(position.debt >= amount, "repay exceeds debt");

        balances.set(owner.clone(), held - amount);
        write_balances(&env, &balances);

        position.debt -= amount;
        let remaining = position.debt;
        vaults.set(key, position);
        write_vaults(&env, &vaults);

        set_supply(&env, supply(&env) - amount);

        DebtRepaid {
            owner,
            asset,
            amount,
            remaining_debt: remaining,
        }
        .publish(&env);
        remaining
    }

    /// Stakes `amount` of `staker`'s stablecoins into the Stability Pool to
    /// absorb under-collateralized vault debt during liquidations.
    pub fn stake_stability_pool(env: Env, staker: Address, amount: i128) -> i128 {
        staker.require_auth();
        assert!(amount > 0, "amount must be positive");

        let mut balances = balances_map(&env);
        let held = balances.get(staker.clone()).unwrap_or(0);
        assert!(held >= amount, "insufficient stablecoin balance");
        balances.set(staker.clone(), held - amount);
        write_balances(&env, &balances);

        let mut stakes = pool_stakes_map(&env);
        let prior = stakes.get(staker.clone()).unwrap_or(0);
        stakes.set(staker.clone(), prior + amount);
        write_pool_stakes(&env, &stakes);

        let pool_total = sp_total(&env) + amount;
        set_sp_total(&env, pool_total);

        StabilityStaked {
            staker,
            amount,
            pool_total,
        }
        .publish(&env);
        pool_total
    }

    /// Unstakes `amount` of stablecoins from the Stability Pool back to
    /// `staker`.
    pub fn unstake_stability_pool(env: Env, staker: Address, amount: i128) -> i128 {
        staker.require_auth();
        assert!(amount > 0, "amount must be positive");

        let mut stakes = pool_stakes_map(&env);
        let prior = stakes.get(staker.clone()).unwrap_or(0);
        let current_total = sp_total(&env);
        assert!(
            prior >= amount && current_total >= amount,
            "insufficient stability pool stake"
        );
        stakes.set(staker.clone(), prior - amount);
        write_pool_stakes(&env, &stakes);

        let pool_total = current_total - amount;
        set_sp_total(&env, pool_total);

        let mut balances = balances_map(&env);
        let held = balances.get(staker.clone()).unwrap_or(0);
        balances.set(staker.clone(), held + amount);
        write_balances(&env, &balances);

        StabilityUnstaked {
            staker,
            amount,
            pool_total,
        }
        .publish(&env);
        pool_total
    }

    /// Liquidates an under-collateralized vault by absorbing its debt from the
    /// Stability Pool and transferring its collateral into the pool reserve.
    pub fn liquidate(env: Env, liquidator: Address, borrower: Address, asset: Symbol) -> i128 {
        liquidator.require_auth();

        let asset_mcr = Self::asset_mcr_bps(env.clone(), asset.clone());
        let price = fetch_oracle_price(&env, &asset);

        let mut vaults = vaults_map(&env);
        let key = (borrower.clone(), asset.clone());
        let position = vaults.get(key.clone()).unwrap_or_else(empty_vault);
        assert!(position.debt > 0, "vault has no debt");
        assert!(
            !is_healthy(position.collateral, position.debt, price, asset_mcr),
            "vault is healthy"
        );

        let current_pool = sp_total(&env);
        assert!(
            current_pool >= position.debt,
            "insufficient stability pool depth"
        );

        let debt_absorbed = position.debt;
        let collateral_seized = position.collateral;

        set_sp_total(&env, current_pool - debt_absorbed);
        set_supply(&env, supply(&env) - debt_absorbed);

        let mut pool_col = pool_collateral_map(&env);
        let prev_col = pool_col.get(asset.clone()).unwrap_or(0);
        pool_col.set(asset.clone(), prev_col + collateral_seized);
        write_pool_collateral(&env, &pool_col);

        vaults.set(key, empty_vault());
        write_vaults(&env, &vaults);

        env.storage()
            .temporary()
            .set(&cache_liq_ts_key(&env), &env.ledger().timestamp());

        VaultLiquidated {
            borrower,
            liquidator,
            asset,
            debt_absorbed,
            collateral_seized,
        }
        .publish(&env);
        collateral_seized
    }

    /// Redeems `amount` of stablecoin against `borrower`'s vault at the oracle
    /// price minus an algorithmic redemption fee that rises with volume and
    /// decays over time.
    pub fn redeem(
        env: Env,
        redeemer: Address,
        borrower: Address,
        asset: Symbol,
        amount: i128,
    ) -> i128 {
        redeemer.require_auth();
        assert!(amount > 0, "amount must be positive");

        let mut balances = balances_map(&env);
        let held = balances.get(redeemer.clone()).unwrap_or(0);
        assert!(held >= amount, "insufficient stablecoin balance");

        let mut vaults = vaults_map(&env);
        let key = (borrower.clone(), asset.clone());
        let mut position = vaults.get(key.clone()).unwrap_or_else(empty_vault);
        assert!(position.debt >= amount, "redemption exceeds vault debt");

        let price = fetch_oracle_price(&env, &asset);
        let (next_base, fee_bps) = compute_redemption_fee(&env, amount);

        let scaled = amount.checked_mul(PRICE_SCALE).expect("redemption overflow");
        let gross = scaled / price;
        let fee_scaled = gross.checked_mul(fee_bps).expect("fee overflow");
        let collateral_out = gross - fee_scaled / BPS_SCALE;
        assert!(
            position.collateral >= collateral_out,
            "insufficient vault collateral"
        );

        balances.set(redeemer.clone(), held - amount);
        write_balances(&env, &balances);

        position.debt -= amount;
        position.collateral -= collateral_out;
        vaults.set(key, position);
        write_vaults(&env, &vaults);

        set_supply(&env, supply(&env) - amount);
        env.storage()
            .instance()
            .set(&base_rate_key(&env), &next_base);
        env.storage()
            .instance()
            .set(&last_fee_at_key(&env), &env.ledger().timestamp());
        env.storage()
            .temporary()
            .set(&cache_fee_bps_key(&env), &fee_bps);

        StablecoinRedeemed {
            redeemer,
            borrower,
            asset,
            amount,
            collateral_out,
            fee_bps,
        }
        .publish(&env);
        collateral_out
    }
}

/// Multi-hop convenience router combining collateral deposit and stablecoin
/// minting into a single atomic invocation, demonstrating nested authorization
/// sub-trees.
#[contract]
pub struct VaultRouterContract;

#[contractimpl]
impl VaultRouterContract {
    /// Atomically deposits collateral and mints stablecoins in `stablecoin`.
    pub fn deposit_and_mint(
        env: Env,
        stablecoin: Address,
        owner: Address,
        asset: Symbol,
        collateral_amount: i128,
        mint_amount: i128,
    ) -> i128 {
        owner.require_auth();
        let client = StablecoinContractClient::new(&env, &stablecoin);
        client.deposit_collateral(&owner, &asset, &collateral_amount);
        client.mint(&owner, &asset, &mint_amount)
    }
}

#[cfg(test)]
mod test;
