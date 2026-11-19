#![no_std]
#![allow(deprecated)]

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, symbol_short, Address,
    Bytes, Env, Symbol,
};

/// ERC-3156 callback success magic return value.
pub const CALLBACK_SUCCESS: Symbol = symbol_short!("success");

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum FlashMintError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    Unauthorized = 3,
    Paused = 4,
    ZeroAmount = 5,
    InsufficientBalance = 6,
    ExceedsMaxFlashLoan = 7,
    CallbackFailed = 8,
    DeficitAfterFlashMint = 9,
    ReentrancyGuard = 10,
    InvalidToken = 11,
    Overflow = 12,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Admin,
    Treasury,
    FeeBps,
    MaxFlashMint,
    Paused,
    TotalSupply,
    Balance(Address),
    IsLocked,
}

/// Interface that a flash borrower / receiver contract must implement.
#[contractclient(name = "FlashBorrowerClient")]
pub trait FlashBorrower {
    fn on_flash_loan(
        env: Env,
        initiator: Address,
        token: Address,
        amount: i128,
        fee: i128,
        data: Bytes,
    ) -> Symbol;
}

#[contract]
pub struct FlashMintToken;

#[contractimpl]
impl FlashMintToken {
    /// Initialize the flash mint token with admin, treasury, fee basis points, and initial borrow limit.
    pub fn initialize(
        env: Env,
        admin: Address,
        treasury: Address,
        fee_bps: u32,
        max_flash_mint: i128,
    ) -> Result<(), FlashMintError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(FlashMintError::AlreadyInitialized);
        }
        if max_flash_mint <= 0 {
            return Err(FlashMintError::ZeroAmount);
        }

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Treasury, &treasury);
        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
        env.storage()
            .instance()
            .set(&DataKey::MaxFlashMint, &max_flash_mint);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage().instance().set(&DataKey::TotalSupply, &0i128);

        Ok(())
    }

    /// Mint tokens directly to an account (admin only).
    pub fn mint(env: Env, to: Address, amount: i128) -> Result<(), FlashMintError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(FlashMintError::NotInitialized)?;
        admin.require_auth();

        Self::mint_internal(&env, &to, amount)
    }

    /// Burn tokens from an account.
    pub fn burn(env: Env, from: Address, amount: i128) -> Result<(), FlashMintError> {
        from.require_auth();
        Self::burn_internal(&env, &from, amount)
    }

    /// Transfer tokens between accounts.
    pub fn transfer(
        env: Env,
        from: Address,
        to: Address,
        amount: i128,
    ) -> Result<(), FlashMintError> {
        from.require_auth();
        if amount <= 0 {
            return Err(FlashMintError::ZeroAmount);
        }

        let from_bal = Self::balance(env.clone(), from.clone());
        if from_bal < amount {
            return Err(FlashMintError::InsufficientBalance);
        }

        env.storage()
            .persistent()
            .set(&DataKey::Balance(from.clone()), &(from_bal - amount));
        let to_bal = Self::balance(env.clone(), to.clone());
        env.storage()
            .persistent()
            .set(&DataKey::Balance(to.clone()), &(to_bal + amount));

        env.events()
            .publish((symbol_short!("transfer"), from, to), amount);

        Ok(())
    }

    /// Query the balance of an account.
    pub fn balance(env: Env, account: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::Balance(account))
            .unwrap_or(0i128)
    }

    /// Query current total token supply.
    pub fn total_supply(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::TotalSupply)
            .unwrap_or(0i128)
    }

    /// Query maximum amount eligible for flash minting.
    pub fn max_flash_loan(env: Env, token: Address) -> i128 {
        let is_paused: bool = env
            .storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false);
        if is_paused || token != env.current_contract_address() {
            return 0;
        }
        env.storage()
            .instance()
            .get(&DataKey::MaxFlashMint)
            .unwrap_or(0i128)
    }

    /// Calculate flash mint fee for a given token and amount.
    pub fn flash_fee(env: Env, token: Address, amount: i128) -> Result<i128, FlashMintError> {
        if token != env.current_contract_address() {
            return Err(FlashMintError::InvalidToken);
        }
        if amount <= 0 {
            return Err(FlashMintError::ZeroAmount);
        }
        let fee_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::FeeBps)
            .ok_or(FlashMintError::NotInitialized)?;

        let fee = amount
            .checked_mul(fee_bps as i128)
            .ok_or(FlashMintError::Overflow)?
            / 10_000i128;
        Ok(fee)
    }

    /// Execute an ERC-3156 compliant cross-contract Flash Mint.
    ///
    /// Mints `amount` unbacked tokens to `receiver`, invokes `receiver.on_flash_loan()`,
    /// and ensures `amount + fee` is retrieved and burned before invocation completion.
    pub fn flash_loan(
        env: Env,
        receiver: Address,
        token: Address,
        amount: i128,
        data: Bytes,
    ) -> Result<bool, FlashMintError> {
        let is_paused: bool = env
            .storage()
            .instance()
            .get(&DataKey::Paused)
            .ok_or(FlashMintError::NotInitialized)?;
        if is_paused {
            return Err(FlashMintError::Paused);
        }

        if token != env.current_contract_address() {
            return Err(FlashMintError::InvalidToken);
        }

        let max_loan = Self::max_flash_loan(env.clone(), token.clone());
        if amount <= 0 || amount > max_loan {
            return Err(FlashMintError::ExceedsMaxFlashLoan);
        }

        // Reentrancy guard
        let is_locked: bool = env
            .storage()
            .temporary()
            .get(&DataKey::IsLocked)
            .unwrap_or(false);
        if is_locked {
            return Err(FlashMintError::ReentrancyGuard);
        }
        env.storage().temporary().set(&DataKey::IsLocked, &true);

        let fee = Self::flash_fee(env.clone(), token.clone(), amount)?;
        let treasury: Address = env
            .storage()
            .instance()
            .get(&DataKey::Treasury)
            .ok_or(FlashMintError::NotInitialized)?;

        let initial_supply = Self::total_supply(env.clone());

        // 1. Flash mint temporary liquidity to receiver
        Self::mint_internal(&env, &receiver, amount)?;

        // 2. Invoke receiver cross-contract callback
        let borrower = FlashBorrowerClient::new(&env, &receiver);
        let callback_res = borrower.on_flash_loan(
            &env.current_contract_address(),
            &token,
            &amount,
            &fee,
            &data,
        );

        if callback_res != CALLBACK_SUCCESS {
            return Err(FlashMintError::CallbackFailed);
        }

        // 3. Verify borrower has returned/holds sufficient balance to cover principal + fee
        let total_due = amount.checked_add(fee).ok_or(FlashMintError::Overflow)?;
        let receiver_bal = Self::balance(env.clone(), receiver.clone());
        if receiver_bal < total_due {
            return Err(FlashMintError::DeficitAfterFlashMint);
        }

        // 4. Burn the principal amount from receiver
        Self::burn_internal(&env, &receiver, amount)?;

        // 5. Transfer fee to treasury
        if fee > 0 {
            let bal_after_burn = Self::balance(env.clone(), receiver.clone());
            env.storage()
                .persistent()
                .set(&DataKey::Balance(receiver.clone()), &(bal_after_burn - fee));

            let treasury_bal = Self::balance(env.clone(), treasury.clone());
            env.storage()
                .persistent()
                .set(&DataKey::Balance(treasury.clone()), &(treasury_bal + fee));

            env.events()
                .publish((symbol_short!("fee_paid"), receiver.clone(), treasury), fee);
        }

        // 6. Invariant check: total supply after flash mint matches initial supply
        let final_supply = Self::total_supply(env.clone());
        if final_supply != initial_supply {
            return Err(FlashMintError::DeficitAfterFlashMint);
        }

        // Clear reentrancy guard
        env.storage().temporary().remove(&DataKey::IsLocked);

        // Emit FlashMint completion event
        env.events()
            .publish((symbol_short!("flash_mnt"), receiver, token), (amount, fee));

        Ok(true)
    }

    /// Admin configuration: adjust fee basis points.
    pub fn set_fee_bps(env: Env, fee_bps: u32) -> Result<(), FlashMintError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(FlashMintError::NotInitialized)?;
        admin.require_auth();

        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
        env.events()
            .publish((symbol_short!("cfg_fee"), admin), fee_bps);
        Ok(())
    }

    /// Admin configuration: adjust max flash mint borrow limit.
    pub fn set_max_flash_mint(env: Env, max_flash_mint: i128) -> Result<(), FlashMintError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(FlashMintError::NotInitialized)?;
        admin.require_auth();

        if max_flash_mint <= 0 {
            return Err(FlashMintError::ZeroAmount);
        }

        env.storage()
            .instance()
            .set(&DataKey::MaxFlashMint, &max_flash_mint);
        env.events()
            .publish((symbol_short!("cfg_limit"), admin), max_flash_mint);
        Ok(())
    }

    /// Admin pause.
    pub fn pause(env: Env) -> Result<(), FlashMintError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(FlashMintError::NotInitialized)?;
        admin.require_auth();

        env.storage().instance().set(&DataKey::Paused, &true);
        env.events().publish((symbol_short!("paused"), admin), true);
        Ok(())
    }

    /// Admin unpause.
    pub fn unpause(env: Env) -> Result<(), FlashMintError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(FlashMintError::NotInitialized)?;
        admin.require_auth();

        env.storage().instance().set(&DataKey::Paused, &false);
        env.events()
            .publish((symbol_short!("unpaused"), admin), false);
        Ok(())
    }

    fn mint_internal(env: &Env, to: &Address, amount: i128) -> Result<(), FlashMintError> {
        if amount <= 0 {
            return Err(FlashMintError::ZeroAmount);
        }

        let bal = Self::balance(env.clone(), to.clone());
        env.storage()
            .persistent()
            .set(&DataKey::Balance(to.clone()), &(bal + amount));

        let supply = Self::total_supply(env.clone());
        let new_supply = supply.checked_add(amount).ok_or(FlashMintError::Overflow)?;
        env.storage()
            .instance()
            .set(&DataKey::TotalSupply, &new_supply);

        env.events()
            .publish((symbol_short!("mint"), to.clone()), amount);

        Ok(())
    }

    fn burn_internal(env: &Env, from: &Address, amount: i128) -> Result<(), FlashMintError> {
        if amount <= 0 {
            return Err(FlashMintError::ZeroAmount);
        }

        let bal = Self::balance(env.clone(), from.clone());
        if bal < amount {
            return Err(FlashMintError::InsufficientBalance);
        }

        let new_bal = bal - amount;
        if new_bal == 0 {
            env.storage()
                .persistent()
                .remove(&DataKey::Balance(from.clone()));
        } else {
            env.storage()
                .persistent()
                .set(&DataKey::Balance(from.clone()), &new_bal);
        }

        let supply = Self::total_supply(env.clone());
        let new_supply = supply.checked_sub(amount).ok_or(FlashMintError::Overflow)?;
        env.storage()
            .instance()
            .set(&DataKey::TotalSupply, &new_supply);

        env.events()
            .publish((symbol_short!("burn"), from.clone()), amount);

        Ok(())
    }
}
