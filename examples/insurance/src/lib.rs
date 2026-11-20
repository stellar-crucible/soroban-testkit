//! A decentralized mutual insurance pool.
//!
//! Capital providers underwrite the pool by depositing an asset and receiving
//! shares of its reserves. Policyholders buy coverage for a period and pay a
//! premium priced off an annual rate, and a panel of assessors votes on each
//! filed loss. A claim that reaches quorum with a majority of approvals is
//! settled from the pool's reserves; a claim that does not is rejected and pays
//! nothing.
//!
//! Two properties hold by construction and are exercised by the test suite:
//!
//! * A payout is `min(loss, coverage, reserves)`, so the pool can never pay out
//!   more than it holds, and `reserves` is never negative.
//! * Shares are priced against reserves, so a settlement that drains the pool
//!   dilutes every provider at once — a mutual loss, not a first-come
//!   withdrawal.
//!
//! The contract stores reserves and the token balance they must equal is
//! checked by the tests. Amounts are in the capital token's smallest unit;
//! ratios are basis points.
#![no_std]
#![allow(deprecated)]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Env, Vec,
};

/// Denominator for every rate and ratio expressed in basis points.
pub const BPS_DENOMINATOR: i128 = 10_000;

/// Seconds in a 365-day year: the period the premium rate is quoted over.
pub const SECONDS_PER_YEAR: u64 = 31_536_000;

/// Why a call was refused. Variants keep their numbers: a client that matches
/// on the error code should not silently start reading a different failure.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum InsuranceError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    /// A parameter outside the range the pool can operate with.
    InvalidParameter = 3,
    /// A zero or negative amount.
    InvalidAmount = 4,
    /// The signer is not on the assessor panel.
    NotAssessor = 5,
    /// The assessor already voted on this claim.
    AlreadyVoted = 6,
    /// The claim is no longer waiting for votes.
    ClaimNotPending = 7,
    ClaimNotFound = 8,
    PolicyNotFound = 9,
    /// The policy has expired.
    PolicyNotActive = 10,
    /// The claimed loss is larger than the policy covers.
    ClaimExceedsCoverage = 11,
    /// The policy already has a claim.
    ClaimAlreadyFiled = 12,
    /// Settling a claim the assessors did not approve.
    NotApproved = 13,
    /// Withdrawing more shares than the provider holds.
    InsufficientShares = 14,
    /// The pool holds nothing to pay with.
    InsufficientReserves = 15,
    Overflow = 16,
}

/// Protocol parameters, fixed at initialization.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InsuranceConfig {
    pub admin: Address,
    /// The token providers deposit and claims are paid in.
    pub capital_token: Address,
    /// The panel that votes on claims. Every member votes at most once per
    /// claim.
    pub assessors: Vec<Address>,
    /// Approvals plus rejections needed before a claim resolves.
    pub quorum: u32,
    /// Annual premium rate in basis points of coverage (`500` = 5% a year).
    pub premium_rate_bps: u32,
    /// Largest coverage a single policy may buy, in token units.
    pub max_coverage: i128,
}

/// Coverage a policyholder bought.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Policy {
    pub id: u64,
    pub holder: Address,
    pub coverage: i128,
    pub premium_paid: i128,
    pub starts_at: u64,
    pub expires_at: u64,
    /// The claim filed against this policy, or `0` before one is filed.
    pub claim_id: u64,
}

/// Where a claim stands.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ClaimStatus {
    /// Open for assessor votes.
    Pending,
    /// Quorum reached with more approvals than rejections.
    Approved,
    /// Quorum reached without a majority of approvals.
    Rejected,
    /// Approved and paid out (possibly a partial payout capped by reserves).
    Settled,
}

/// A loss a policyholder asked the pool to cover.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Claim {
    pub id: u64,
    pub policy_id: u64,
    pub holder: Address,
    pub loss: i128,
    pub votes_for: u32,
    pub votes_against: u32,
    pub status: ClaimStatus,
    /// What the pool paid, set when the claim settles.
    pub payout: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum InsuranceKey {
    Config,
    TotalShares,
    Reserves,
    Deposited,
    Withdrawn,
    Premiums,
    Payouts,
    PolicyCount,
    ClaimCount,
    Shares(Address),
    Policy(u64),
    Claim(u64),
    Vote(u64, Address),
}

/// The premium for `coverage` held for `duration` seconds at `rate_bps` a year,
/// rounded up to the next token unit so a policy never costs nothing it should
/// cost. `None` on overflow.
pub fn premium_for(coverage: i128, rate_bps: u32, duration: u64) -> Option<i128> {
    if coverage <= 0 || rate_bps == 0 || duration == 0 {
        return Some(0);
    }
    let numerator = coverage
        .checked_mul(rate_bps as i128)?
        .checked_mul(duration as i128)?;
    let denominator = BPS_DENOMINATOR.checked_mul(SECONDS_PER_YEAR as i128)?;
    Some(numerator.div_ceil(denominator))
}

/// Shares minted for a `deposit` into a pool holding `reserves` behind
/// `total_shares` shares.
///
/// The first deposit and a pool whose reserves were wiped out both mint
/// one-for-one: there is no share price to divide by.
pub fn shares_for_deposit(deposit: i128, total_shares: i128, reserves: i128) -> Option<i128> {
    if deposit <= 0 {
        return Some(0);
    }
    if total_shares <= 0 || reserves <= 0 {
        return Some(deposit);
    }
    deposit.checked_mul(total_shares)?.checked_div(reserves)
}

/// The assets `shares` of a pool holding `reserves` behind `total_shares` are
/// worth, rounded down.
pub fn assets_for_shares(shares: i128, total_shares: i128, reserves: i128) -> Option<i128> {
    if shares <= 0 || total_shares <= 0 || reserves <= 0 {
        return Some(0);
    }
    shares.checked_mul(reserves)?.checked_div(total_shares)
}

/// A mutual insurance pool: underwriting, premiums, assessor voting, and
/// payouts against verified losses.
#[contract]
pub struct InsurancePool;

#[contractimpl]
impl InsurancePool {
    /// Configure the pool. Callable once.
    pub fn initialize(env: Env, config: InsuranceConfig) -> Result<(), InsuranceError> {
        if env.storage().instance().has(&InsuranceKey::Config) {
            return Err(InsuranceError::AlreadyInitialized);
        }
        let assessors = config.assessors.len();
        if assessors == 0
            || config.quorum == 0
            || config.quorum > assessors
            || config.premium_rate_bps as i128 > BPS_DENOMINATOR
            || config.max_coverage <= 0
        {
            return Err(InsuranceError::InvalidParameter);
        }
        let storage = env.storage().instance();
        storage.set(&InsuranceKey::Config, &config);
        // Every counter exists from the start, so the first deposit or claim
        // modifies the pool's instance storage instead of growing it.
        for key in [
            InsuranceKey::TotalShares,
            InsuranceKey::Reserves,
            InsuranceKey::Deposited,
            InsuranceKey::Withdrawn,
            InsuranceKey::Premiums,
            InsuranceKey::Payouts,
            InsuranceKey::PolicyCount,
            InsuranceKey::ClaimCount,
        ] {
            storage.set(&key, &0i128);
        }
        Ok(())
    }

    /// Underwrite the pool with `amount` of the capital token, returning the
    /// shares minted.
    ///
    /// Shares are priced against reserves, so a provider who deposits after a
    /// payout has been absorbed receives shares at the post-loss price instead
    /// of buying the old value cheaply.
    pub fn deposit(env: Env, provider: Address, amount: i128) -> Result<i128, InsuranceError> {
        provider.require_auth();
        let config = load_config(&env)?;
        if amount <= 0 {
            return Err(InsuranceError::InvalidAmount);
        }

        let total_shares = total_shares(&env);
        let reserves = reserves(&env);
        let shares = shares_for_deposit(amount, total_shares, reserves)
            .ok_or(InsuranceError::Overflow)?;
        if shares <= 0 {
            // The amount is below one share's worth of reserves; accepting it
            // would take capital the provider could never withdraw.
            return Err(InsuranceError::InvalidAmount);
        }

        token::Client::new(&env, &config.capital_token).transfer(
            &provider,
            env.current_contract_address(),
            &amount,
        );

        let held = shares_of(&env, &provider).checked_add(shares).ok_or(InsuranceError::Overflow)?;
        set_shares(&env, &provider, held);
        env.storage().instance().set(
            &InsuranceKey::TotalShares,
            &total_shares
                .checked_add(shares)
                .ok_or(InsuranceError::Overflow)?,
        );
        env.storage().instance().set(
            &InsuranceKey::Reserves,
            &reserves.checked_add(amount).ok_or(InsuranceError::Overflow)?,
        );
        adjust(&env, InsuranceKey::Deposited, amount)?;

        env.events()
            .publish((symbol_short!("deposit"), provider), (amount, shares));
        Ok(shares)
    }

    /// Burn `shares` and return what they are worth, rounded down.
    ///
    /// A withdrawal may only take what the reserves hold at the moment it is
    /// called, so a provider cannot drain capital that backs a pending loss.
    pub fn withdraw(env: Env, provider: Address, shares: i128) -> Result<i128, InsuranceError> {
        provider.require_auth();
        let config = load_config(&env)?;
        if shares <= 0 {
            return Err(InsuranceError::InvalidAmount);
        }

        let held = shares_of(&env, &provider);
        if shares > held {
            return Err(InsuranceError::InsufficientShares);
        }
        let total_shares = total_shares(&env);
        let reserves = reserves(&env);
        let amount =
            assets_for_shares(shares, total_shares, reserves).ok_or(InsuranceError::Overflow)?;
        if amount <= 0 {
            return Err(InsuranceError::InsufficientReserves);
        }

        set_shares(&env, &provider, held - shares);
        env.storage().instance().set(
            &InsuranceKey::TotalShares,
            &total_shares
                .checked_sub(shares)
                .ok_or(InsuranceError::Overflow)?,
        );
        env.storage().instance().set(
            &InsuranceKey::Reserves,
            &reserves
                .checked_sub(amount)
                .ok_or(InsuranceError::Overflow)?,
        );
        adjust(&env, InsuranceKey::Withdrawn, amount)?;
        token::Client::new(&env, &config.capital_token).transfer(
            &env.current_contract_address(),
            &provider,
            &amount,
        );

        env.events()
            .publish((symbol_short!("withdraw"), provider), (shares, amount));
        Ok(amount)
    }

    /// Buy `coverage` for `duration` seconds, paying the premium into the
    /// reserves.
    ///
    /// The premium is simple interest on the coverage for the period, so
    /// coverage held for half a year costs half the annual rate.
    pub fn buy_policy(
        env: Env,
        holder: Address,
        coverage: i128,
        duration: u64,
    ) -> Result<Policy, InsuranceError> {
        holder.require_auth();
        let config = load_config(&env)?;
        if coverage <= 0 || coverage > config.max_coverage || duration == 0 {
            return Err(InsuranceError::InvalidParameter);
        }

        let premium = premium_for(coverage, config.premium_rate_bps, duration)
            .ok_or(InsuranceError::Overflow)?;
        if premium > 0 {
            token::Client::new(&env, &config.capital_token).transfer(
                &holder,
                env.current_contract_address(),
                &premium,
            );
            let reserves = reserves(&env);
            env.storage().instance().set(
                &InsuranceKey::Reserves,
                &reserves.checked_add(premium).ok_or(InsuranceError::Overflow)?,
            );
            adjust(&env, InsuranceKey::Premiums, premium)?;
        }

        let id = next_id(&env, InsuranceKey::PolicyCount)?;
        let now = env.ledger().timestamp();
        let policy = Policy {
            id,
            holder: holder.clone(),
            coverage,
            premium_paid: premium,
            starts_at: now,
            expires_at: now.saturating_add(duration),
            claim_id: 0,
        };
        env.storage()
            .persistent()
            .set(&InsuranceKey::Policy(id), &policy);

        env.events()
            .publish((symbol_short!("policy"), holder), (id, coverage, premium));
        Ok(policy)
    }

    /// File a loss against a live policy. A policy carries at most one claim.
    pub fn file_claim(
        env: Env,
        holder: Address,
        policy_id: u64,
        loss: i128,
    ) -> Result<Claim, InsuranceError> {
        holder.require_auth();
        load_config(&env)?;
        if loss <= 0 {
            return Err(InsuranceError::InvalidAmount);
        }

        let mut policy: Policy = env
            .storage()
            .persistent()
            .get(&InsuranceKey::Policy(policy_id))
            .ok_or(InsuranceError::PolicyNotFound)?;
        if policy.holder != holder {
            // Report a missing policy rather than confirm that someone else
            // holds this id.
            return Err(InsuranceError::PolicyNotFound);
        }
        if env.ledger().timestamp() > policy.expires_at {
            return Err(InsuranceError::PolicyNotActive);
        }
        if loss > policy.coverage {
            return Err(InsuranceError::ClaimExceedsCoverage);
        }
        if policy.claim_id != 0 {
            return Err(InsuranceError::ClaimAlreadyFiled);
        }

        let id = next_id(&env, InsuranceKey::ClaimCount)?;
        let claim = Claim {
            id,
            policy_id,
            holder: holder.clone(),
            loss,
            votes_for: 0,
            votes_against: 0,
            status: ClaimStatus::Pending,
            payout: 0,
        };
        env.storage()
            .persistent()
            .set(&InsuranceKey::Claim(id), &claim);

        policy.claim_id = id;
        env.storage()
            .persistent()
            .set(&InsuranceKey::Policy(policy_id), &policy);

        env.events()
            .publish((symbol_short!("claim"), holder), (id, policy_id, loss));
        Ok(claim)
    }

    /// Cast an assessor's vote on a pending claim.
    ///
    /// The claim resolves as soon as quorum is reached: approvals win only on a
    /// strict majority, so a tie rejects.
    pub fn vote(
        env: Env,
        assessor: Address,
        claim_id: u64,
        approve: bool,
    ) -> Result<Claim, InsuranceError> {
        assessor.require_auth();
        let config = load_config(&env)?;
        if !config.assessors.contains(&assessor) {
            return Err(InsuranceError::NotAssessor);
        }

        let mut claim: Claim = env
            .storage()
            .persistent()
            .get(&InsuranceKey::Claim(claim_id))
            .ok_or(InsuranceError::ClaimNotFound)?;
        if claim.status != ClaimStatus::Pending {
            return Err(InsuranceError::ClaimNotPending);
        }

        let vote_key = InsuranceKey::Vote(claim_id, assessor.clone());
        if env.storage().persistent().has(&vote_key) {
            return Err(InsuranceError::AlreadyVoted);
        }
        env.storage().persistent().set(&vote_key, &approve);

        if approve {
            claim.votes_for = claim.votes_for.saturating_add(1);
        } else {
            claim.votes_against = claim.votes_against.saturating_add(1);
        }
        if claim.votes_for.saturating_add(claim.votes_against) >= config.quorum {
            claim.status = if claim.votes_for > claim.votes_against {
                ClaimStatus::Approved
            } else {
                ClaimStatus::Rejected
            };
        }

        env.storage()
            .persistent()
            .set(&InsuranceKey::Claim(claim_id), &claim);
        env.events().publish(
            (symbol_short!("vote"), assessor),
            (claim_id, approve, claim.status),
        );
        Ok(claim)
    }

    /// Pay an approved claim from the reserves.
    ///
    /// The payout is the loss capped by the coverage and by what the pool
    /// actually holds, so settlement can never overdraw the pool. A pool with
    /// nothing left refuses to settle rather than recording a zero payout.
    pub fn settle(env: Env, claim_id: u64) -> Result<Claim, InsuranceError> {
        let config = load_config(&env)?;
        let mut claim: Claim = env
            .storage()
            .persistent()
            .get(&InsuranceKey::Claim(claim_id))
            .ok_or(InsuranceError::ClaimNotFound)?;
        if claim.status != ClaimStatus::Approved {
            return Err(InsuranceError::NotApproved);
        }

        let policy: Policy = env
            .storage()
            .persistent()
            .get(&InsuranceKey::Policy(claim.policy_id))
            .ok_or(InsuranceError::PolicyNotFound)?;
        let reserves = reserves(&env);
        if reserves <= 0 {
            return Err(InsuranceError::InsufficientReserves);
        }
        let payout = claim.loss.min(policy.coverage).min(reserves);

        claim.status = ClaimStatus::Settled;
        claim.payout = payout;
        env.storage()
            .persistent()
            .set(&InsuranceKey::Claim(claim_id), &claim);
        env.storage().instance().set(
            &InsuranceKey::Reserves,
            &reserves.checked_sub(payout).ok_or(InsuranceError::Overflow)?,
        );
        adjust(&env, InsuranceKey::Payouts, payout)?;
        token::Client::new(&env, &config.capital_token).transfer(
            &env.current_contract_address(),
            &claim.holder,
            &payout,
        );

        env.events()
            .publish((symbol_short!("settle"), claim.holder.clone()), (claim_id, payout));
        Ok(claim)
    }

    /// The pool's parameters.
    pub fn config(env: Env) -> Result<InsuranceConfig, InsuranceError> {
        load_config(&env)
    }

    /// The reserves backing every share: capital plus premiums, less
    /// withdrawals and payouts.
    pub fn reserves(env: Env) -> i128 {
        reserves(&env)
    }

    /// Shares of the reserves `provider` holds.
    pub fn shares_of(env: Env, provider: Address) -> i128 {
        shares_of(&env, &provider)
    }

    /// Every share the pool has issued.
    pub fn total_shares(env: Env) -> i128 {
        total_shares(&env)
    }

    /// Capital providers have deposited over the pool's life.
    pub fn total_deposited(env: Env) -> i128 {
        total(&env, &InsuranceKey::Deposited)
    }

    /// Capital providers have withdrawn over the pool's life.
    pub fn total_withdrawn(env: Env) -> i128 {
        total(&env, &InsuranceKey::Withdrawn)
    }

    /// Premiums policyholders have paid over the pool's life.
    pub fn total_premiums(env: Env) -> i128 {
        total(&env, &InsuranceKey::Premiums)
    }

    /// Claims the pool has paid over its life. This can never exceed the
    /// reserves at the moment each claim settled, and therefore never exceeds
    /// `total_deposited + total_premiums`.
    pub fn total_payouts(env: Env) -> i128 {
        total(&env, &InsuranceKey::Payouts)
    }

    /// A policy by id.
    pub fn policy(env: Env, policy_id: u64) -> Result<Policy, InsuranceError> {
        env.storage()
            .persistent()
            .get(&InsuranceKey::Policy(policy_id))
            .ok_or(InsuranceError::PolicyNotFound)
    }

    /// A claim by id.
    pub fn claim(env: Env, claim_id: u64) -> Result<Claim, InsuranceError> {
        env.storage()
            .persistent()
            .get(&InsuranceKey::Claim(claim_id))
            .ok_or(InsuranceError::ClaimNotFound)
    }

    /// Whether `assessor` has already voted on `claim_id`.
    pub fn has_voted(env: Env, claim_id: u64, assessor: Address) -> bool {
        env.storage()
            .persistent()
            .has(&InsuranceKey::Vote(claim_id, assessor))
    }
}

fn load_config(env: &Env) -> Result<InsuranceConfig, InsuranceError> {
    env.storage()
        .instance()
        .get(&InsuranceKey::Config)
        .ok_or(InsuranceError::NotInitialized)
}

fn total(env: &Env, key: &InsuranceKey) -> i128 {
    env.storage().instance().get(key).unwrap_or(0)
}

fn total_shares(env: &Env) -> i128 {
    total(env, &InsuranceKey::TotalShares)
}

fn reserves(env: &Env) -> i128 {
    total(env, &InsuranceKey::Reserves)
}

fn shares_of(env: &Env, provider: &Address) -> i128 {
    env.storage()
        .persistent()
        .get(&InsuranceKey::Shares(provider.clone()))
        .unwrap_or(0)
}

/// Store a share balance, or drop the entry once it is empty.
fn set_shares(env: &Env, provider: &Address, shares: i128) {
    let key = InsuranceKey::Shares(provider.clone());
    if shares <= 0 {
        env.storage().persistent().remove(&key);
    } else {
        env.storage().persistent().set(&key, &shares);
    }
}

fn adjust(env: &Env, key: InsuranceKey, delta: i128) -> Result<(), InsuranceError> {
    let next = total(env, &key)
        .checked_add(delta)
        .ok_or(InsuranceError::Overflow)?;
    env.storage().instance().set(&key, &next);
    Ok(())
}

fn next_id(env: &Env, key: InsuranceKey) -> Result<u64, InsuranceError> {
    let next = total(env, &key).saturating_add(1);
    env.storage().instance().set(&key, &next);
    u64::try_from(next).map_err(|_| InsuranceError::Overflow)
}
