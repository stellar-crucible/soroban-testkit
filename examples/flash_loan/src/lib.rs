//! Flash loan provider and receiver worked example.
//!
//! A flash loan lends tokens that must come back, with a fee, before the same
//! invocation ends. [`FlashLoanProvider`] holds liquidity in any number of
//! Stellar Asset Contract tokens, sends the principal to a receiver contract,
//! calls the receiver back, and then checks its own balance: if it has not
//! grown by at least the fee, the provider returns an error and the host rolls
//! the whole invocation back, principal transfer included.
//!
//! Fees are attributed per token and per borrower, so the example also shows
//! how to test bookkeeping that only exists when a loan succeeds.
//!
//! No reentrancy lock is needed: the Soroban host refuses to let a contract
//! re-enter itself, so a receiver cannot call back into the provider while a
//! loan is open.
//!
//! [`FlashLoanReceiver`] is a configurable borrower. Its [`RepayMode`] decides
//! whether it repays in full, comes up short, returns only the principal, or
//! refuses the callback, which is how the tests drive every failure path.
#![no_std]
#![allow(deprecated)]

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, panic_with_error,
    symbol_short, token, Address, Bytes, Env, Symbol, Vec,
};

/// Value a receiver returns from its callback to accept a loan.
pub const CALLBACK_SUCCESS: Symbol = symbol_short!("success");

/// Highest fee the provider can be configured with: 10%.
pub const MAX_FEE_BPS: u32 = 1_000;

/// Basis-point denominator.
pub const BPS_DENOMINATOR: i128 = 10_000;

/// Fee owed on `amount` at `fee_bps`, rounded up so that no positive loan is
/// free unless the fee rate is zero.
///
/// Returns `None` on overflow or a negative amount.
pub fn compute_fee(amount: i128, fee_bps: u32) -> Option<i128> {
    if amount < 0 {
        return None;
    }
    if fee_bps == 0 || amount == 0 {
        return Some(0);
    }
    let scaled = amount.checked_mul(fee_bps as i128)?;
    Some(scaled.checked_add(BPS_DENOMINATOR - 1)? / BPS_DENOMINATOR)
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum FlashLoanError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    InvalidAmount = 3,
    UnsupportedToken = 4,
    InsufficientLiquidity = 5,
    RepaymentShort = 6,
    CallbackRejected = 7,
    InvalidFee = 8,
    LengthMismatch = 9,
    DuplicateToken = 10,
    Overflow = 11,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Admin,
    FeeBps,
    Supported(Address),
    FeesCollected(Address),
    BorrowerFees(Address, Address),
    LoanCount,
}

/// The callbacks a flash loan receiver implements.
#[contractclient(name = "ReceiverInterfaceClient")]
pub trait FlashLoanReceiverInterface {
    /// Called once the principal of a single-token loan has arrived. The
    /// receiver must transfer `amount + fee` back to `provider` and return
    /// [`CALLBACK_SUCCESS`].
    fn on_flash_loan(
        env: Env,
        provider: Address,
        borrower: Address,
        token: Address,
        amount: i128,
        fee: i128,
        data: Bytes,
    ) -> Symbol;

    /// Called once every principal of a multi-token loan has arrived.
    fn on_flash_loan_multi(
        env: Env,
        provider: Address,
        borrower: Address,
        tokens: Vec<Address>,
        amounts: Vec<i128>,
        fees: Vec<i128>,
        data: Bytes,
    ) -> Symbol;
}

#[contract]
pub struct FlashLoanProvider;

fn admin_of(env: &Env) -> Result<Address, FlashLoanError> {
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(FlashLoanError::NotInitialized)
}

fn fee_bps_of(env: &Env) -> Result<u32, FlashLoanError> {
    env.storage()
        .instance()
        .get(&DataKey::FeeBps)
        .ok_or(FlashLoanError::NotInitialized)
}

fn is_supported(env: &Env, token: &Address) -> bool {
    env.storage()
        .persistent()
        .get(&DataKey::Supported(token.clone()))
        .unwrap_or(false)
}

fn add_to(env: &Env, key: &DataKey, amount: i128) -> Result<(), FlashLoanError> {
    let current: i128 = env.storage().persistent().get(key).unwrap_or(0);
    let next = current
        .checked_add(amount)
        .ok_or(FlashLoanError::Overflow)?;
    env.storage().persistent().set(key, &next);
    Ok(())
}

/// Checks a loan request and returns the fee it owes.
fn check_loan(env: &Env, token: &Address, amount: i128) -> Result<i128, FlashLoanError> {
    if amount <= 0 {
        return Err(FlashLoanError::InvalidAmount);
    }
    if !is_supported(env, token) {
        return Err(FlashLoanError::UnsupportedToken);
    }
    let liquidity = token::Client::new(env, token).balance(&env.current_contract_address());
    if amount > liquidity {
        return Err(FlashLoanError::InsufficientLiquidity);
    }
    compute_fee(amount, fee_bps_of(env)?).ok_or(FlashLoanError::Overflow)
}

/// Records a repaid loan's fee against its token and its borrower.
fn attribute_fee(
    env: &Env,
    borrower: &Address,
    token: &Address,
    fee: i128,
) -> Result<(), FlashLoanError> {
    add_to(env, &DataKey::FeesCollected(token.clone()), fee)?;
    add_to(
        env,
        &DataKey::BorrowerFees(borrower.clone(), token.clone()),
        fee,
    )
}

fn bump_loan_count(env: &Env) {
    let count: u32 = env
        .storage()
        .instance()
        .get(&DataKey::LoanCount)
        .unwrap_or(0);
    env.storage()
        .instance()
        .set(&DataKey::LoanCount, &(count + 1));
}

#[contractimpl]
impl FlashLoanProvider {
    /// Set the admin and the fee rate in basis points.
    pub fn initialize(env: Env, admin: Address, fee_bps: u32) -> Result<(), FlashLoanError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(FlashLoanError::AlreadyInitialized);
        }
        if fee_bps > MAX_FEE_BPS {
            return Err(FlashLoanError::InvalidFee);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
        Ok(())
    }

    /// Allow loans of `token` (admin only).
    pub fn add_token(env: Env, token: Address) -> Result<(), FlashLoanError> {
        admin_of(&env)?.require_auth();
        env.storage()
            .persistent()
            .set(&DataKey::Supported(token.clone()), &true);
        env.events().publish((symbol_short!("add_token"),), token);
        Ok(())
    }

    /// Change the fee rate (admin only).
    pub fn set_fee_bps(env: Env, fee_bps: u32) -> Result<(), FlashLoanError> {
        admin_of(&env)?.require_auth();
        if fee_bps > MAX_FEE_BPS {
            return Err(FlashLoanError::InvalidFee);
        }
        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
        Ok(())
    }

    /// Add liquidity: move `amount` of `token` from `from` into the pool.
    pub fn deposit(
        env: Env,
        from: Address,
        token: Address,
        amount: i128,
    ) -> Result<(), FlashLoanError> {
        from.require_auth();
        if amount <= 0 {
            return Err(FlashLoanError::InvalidAmount);
        }
        if !is_supported(&env, &token) {
            return Err(FlashLoanError::UnsupportedToken);
        }
        token::Client::new(&env, &token).transfer(&from, env.current_contract_address(), &amount);
        env.events()
            .publish((symbol_short!("deposit"), from, token), amount);
        Ok(())
    }

    /// The most that can be borrowed of `token` right now.
    pub fn max_flash_loan(env: Env, token: Address) -> i128 {
        if !is_supported(&env, &token) {
            return 0;
        }
        token::Client::new(&env, &token).balance(&env.current_contract_address())
    }

    /// The fee owed on borrowing `amount`.
    pub fn flash_fee(env: Env, amount: i128) -> Result<i128, FlashLoanError> {
        if amount <= 0 {
            return Err(FlashLoanError::InvalidAmount);
        }
        compute_fee(amount, fee_bps_of(&env)?).ok_or(FlashLoanError::Overflow)
    }

    /// Lend `amount` of `token` to `receiver` for the duration of this call.
    ///
    /// `borrower` authorizes the loan and is charged the fee. The receiver
    /// must repay `amount + fee` inside its callback; otherwise this returns
    /// an error and the host reverts every effect of the call.
    pub fn flash_loan(
        env: Env,
        borrower: Address,
        receiver: Address,
        token: Address,
        amount: i128,
        data: Bytes,
    ) -> Result<i128, FlashLoanError> {
        borrower.require_auth();
        let fee = check_loan(&env, &token, amount)?;

        let this = env.current_contract_address();
        let client = token::Client::new(&env, &token);
        let before = client.balance(&this);
        client.transfer(&this, &receiver, &amount);

        let reply = ReceiverInterfaceClient::new(&env, &receiver)
            .on_flash_loan(&this, &borrower, &token, &amount, &fee, &data);
        if reply != CALLBACK_SUCCESS {
            return Err(FlashLoanError::CallbackRejected);
        }
        let owed = before.checked_add(fee).ok_or(FlashLoanError::Overflow)?;
        if client.balance(&this) < owed {
            return Err(FlashLoanError::RepaymentShort);
        }

        attribute_fee(&env, &borrower, &token, fee)?;
        bump_loan_count(&env);
        env.events()
            .publish((symbol_short!("flash"), borrower, token), (amount, fee));
        Ok(fee)
    }

    /// Lend several tokens at once with a single receiver callback.
    pub fn flash_loan_multi(
        env: Env,
        borrower: Address,
        receiver: Address,
        tokens: Vec<Address>,
        amounts: Vec<i128>,
        data: Bytes,
    ) -> Result<Vec<i128>, FlashLoanError> {
        borrower.require_auth();
        if tokens.is_empty() || tokens.len() != amounts.len() {
            return Err(FlashLoanError::LengthMismatch);
        }
        let mut fees: Vec<i128> = Vec::new(&env);
        let mut before: Vec<i128> = Vec::new(&env);
        let this = env.current_contract_address();
        for (index, token) in tokens.iter().enumerate() {
            for earlier in tokens.iter().take(index) {
                if earlier == token {
                    return Err(FlashLoanError::DuplicateToken);
                }
            }
            let amount = amounts.get_unchecked(index as u32);
            fees.push_back(check_loan(&env, &token, amount)?);
            before.push_back(token::Client::new(&env, &token).balance(&this));
        }

        for (index, token) in tokens.iter().enumerate() {
            token::Client::new(&env, &token).transfer(
                &this,
                &receiver,
                &amounts.get_unchecked(index as u32),
            );
        }
        let reply = ReceiverInterfaceClient::new(&env, &receiver)
            .on_flash_loan_multi(&this, &borrower, &tokens, &amounts, &fees, &data);
        if reply != CALLBACK_SUCCESS {
            return Err(FlashLoanError::CallbackRejected);
        }
        for (index, token) in tokens.iter().enumerate() {
            let fee = fees.get_unchecked(index as u32);
            let owed = before
                .get_unchecked(index as u32)
                .checked_add(fee)
                .ok_or(FlashLoanError::Overflow)?;
            if token::Client::new(&env, &token).balance(&this) < owed {
                return Err(FlashLoanError::RepaymentShort);
            }
            attribute_fee(&env, &borrower, &token, fee)?;
        }
        bump_loan_count(&env);
        env.events()
            .publish((symbol_short!("flash_mul"), borrower), tokens.len());
        Ok(fees)
    }

    /// Total fees collected in `token`.
    pub fn fees_collected(env: Env, token: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::FeesCollected(token))
            .unwrap_or(0)
    }

    /// Fees `borrower` has paid in `token`.
    pub fn borrower_fees(env: Env, borrower: Address, token: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::BorrowerFees(borrower, token))
            .unwrap_or(0)
    }

    /// Number of successful loans.
    pub fn loan_count(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::LoanCount)
            .unwrap_or(0)
    }
}

/// How the example receiver behaves in its callback.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RepayMode {
    /// Repay principal plus fee.
    Full,
    /// Repay principal plus fee minus the given amount.
    ShortBy(i128),
    /// Repay only the principal.
    PrincipalOnly,
    /// Refuse the loan by returning something other than `CALLBACK_SUCCESS`.
    Reject,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ReceiverError {
    NotConfigured = 1,
    UnknownLender = 2,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReceiverKey {
    Lender,
    Mode,
    Loans,
}

#[contract]
pub struct FlashLoanReceiver;

fn repayment(env: &Env, amount: i128, fee: i128) -> Option<i128> {
    let mode: RepayMode = env
        .storage()
        .instance()
        .get(&ReceiverKey::Mode)
        .unwrap_or(RepayMode::Full);
    match mode {
        RepayMode::Full => Some(amount + fee),
        RepayMode::ShortBy(shortfall) => Some(amount + fee - shortfall),
        RepayMode::PrincipalOnly => Some(amount),
        RepayMode::Reject => None,
    }
}

/// Only the configured lender may drive the callback, and it must be the
/// contract that invoked this one.
fn accept_lender(env: &Env, provider: &Address) {
    let lender: Address = env
        .storage()
        .instance()
        .get(&ReceiverKey::Lender)
        .unwrap_or_else(|| panic_with_error!(env, ReceiverError::NotConfigured));
    if &lender != provider {
        panic_with_error!(env, ReceiverError::UnknownLender);
    }
    provider.require_auth();
    let loans: u32 = env
        .storage()
        .instance()
        .get(&ReceiverKey::Loans)
        .unwrap_or(0);
    env.storage()
        .instance()
        .set(&ReceiverKey::Loans, &(loans + 1));
}

#[contractimpl]
impl FlashLoanReceiver {
    /// Choose the lender this receiver trusts and how it repays.
    pub fn configure(env: Env, lender: Address, mode: RepayMode) {
        env.storage().instance().set(&ReceiverKey::Lender, &lender);
        env.storage().instance().set(&ReceiverKey::Mode, &mode);
    }

    /// Callbacks this receiver has accepted.
    pub fn loans_received(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&ReceiverKey::Loans)
            .unwrap_or(0)
    }

    pub fn on_flash_loan(
        env: Env,
        provider: Address,
        _borrower: Address,
        token: Address,
        amount: i128,
        fee: i128,
        _data: Bytes,
    ) -> Symbol {
        accept_lender(&env, &provider);
        let Some(repay) = repayment(&env, amount, fee) else {
            return symbol_short!("reject");
        };
        token::Client::new(&env, &token).transfer(
            &env.current_contract_address(),
            &provider,
            &repay,
        );
        CALLBACK_SUCCESS
    }

    pub fn on_flash_loan_multi(
        env: Env,
        provider: Address,
        _borrower: Address,
        tokens: Vec<Address>,
        amounts: Vec<i128>,
        fees: Vec<i128>,
        _data: Bytes,
    ) -> Symbol {
        accept_lender(&env, &provider);
        let this = env.current_contract_address();
        for (index, token) in tokens.iter().enumerate() {
            let index = index as u32;
            let Some(repay) = repayment(
                &env,
                amounts.get_unchecked(index),
                fees.get_unchecked(index),
            ) else {
                return symbol_short!("reject");
            };
            token::Client::new(&env, &token).transfer(&this, &provider, &repay);
        }
        CALLBACK_SUCCESS
    }
}
