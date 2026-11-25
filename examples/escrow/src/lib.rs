#![no_std]

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, token, Address, Env,
    Symbol, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum EscrowError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    InvalidAmount = 4,
    InvalidState = 5,
    MilestoneNotFound = 6,
    MilestoneAlreadyApproved = 7,
    NotExpired = 8,
    PayoutMismatch = 9,
    ExceedsTotal = 10,
    InsufficientDeposit = 11,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum EscrowStatus {
    Active = 0,
    Disputed = 1,
    Resolved = 2,
    Refunded = 3,
    Completed = 4,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum MilestoneStatus {
    Pending = 0,
    Approved = 1,
    Released = 2,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Milestone {
    pub id: u32,
    pub description: Symbol,
    pub amount: i128,
    pub status: MilestoneStatus,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowConfig {
    pub buyer: Address,
    pub seller: Address,
    pub arbiter: Address,
    pub token: Address,
    pub total_amount: i128,
    pub deposited_amount: i128,
    pub released_amount: i128,
    pub timeout_ledger: u32,
    pub status: EscrowStatus,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Config,
    Milestone(u32),
    MilestoneIds,
}

#[contractevent(topics = ["escrow", "initialized"])]
#[derive(Clone)]
pub struct EscrowInitialized {
    pub buyer: Address,
    pub seller: Address,
    pub arbiter: Address,
    pub total_amount: i128,
    pub timeout_ledger: u32,
}

#[contractevent(topics = ["escrow", "deposited"])]
#[derive(Clone)]
pub struct FundsDeposited {
    pub from: Address,
    pub amount: i128,
    pub total_deposited: i128,
}

#[contractevent(topics = ["escrow", "milestone_approved"])]
#[derive(Clone)]
pub struct MilestoneApproved {
    pub milestone_id: u32,
    pub amount: i128,
    pub approved_by: Address,
}

#[contractevent(topics = ["escrow", "disputed"])]
#[derive(Clone)]
pub struct DisputeRaised {
    pub raised_by: Address,
    pub reason: Symbol,
}

#[contractevent(topics = ["escrow", "resolved"])]
#[derive(Clone)]
pub struct DisputeResolved {
    pub arbiter: Address,
    pub buyer_payout: i128,
    pub seller_payout: i128,
}

#[contractevent(topics = ["escrow", "refunded"])]
#[derive(Clone)]
pub struct EscrowRefunded {
    pub buyer: Address,
    pub amount: i128,
}

#[contract]
pub struct EscrowContract;

#[contractimpl]
impl EscrowContract {
    pub fn initialize(
        env: Env,
        buyer: Address,
        seller: Address,
        arbiter: Address,
        token: Address,
        total_amount: i128,
        timeout_ledger: u32,
    ) -> Result<(), EscrowError> {
        if env.storage().instance().has(&DataKey::Config) {
            return Err(EscrowError::AlreadyInitialized);
        }
        if total_amount <= 0 {
            return Err(EscrowError::InvalidAmount);
        }

        let config = EscrowConfig {
            buyer: buyer.clone(),
            seller: seller.clone(),
            arbiter: arbiter.clone(),
            token,
            total_amount,
            deposited_amount: 0,
            released_amount: 0,
            timeout_ledger,
            status: EscrowStatus::Active,
        };

        env.storage().instance().set(&DataKey::Config, &config);
        let ids: Vec<u32> = Vec::new(&env);
        env.storage().instance().set(&DataKey::MilestoneIds, &ids);

        EscrowInitialized {
            buyer,
            seller,
            arbiter,
            total_amount,
            timeout_ledger,
        }
        .publish(&env);

        Ok(())
    }

    pub fn add_milestone(
        env: Env,
        caller: Address,
        id: u32,
        description: Symbol,
        amount: i128,
    ) -> Result<(), EscrowError> {
        caller.require_auth();
        let config: EscrowConfig = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(EscrowError::NotInitialized)?;

        if caller != config.buyer && caller != config.seller {
            return Err(EscrowError::Unauthorized);
        }
        if config.status != EscrowStatus::Active {
            return Err(EscrowError::InvalidState);
        }
        if amount <= 0 {
            return Err(EscrowError::InvalidAmount);
        }
        if env.storage().persistent().has(&DataKey::Milestone(id)) {
            return Err(EscrowError::AlreadyInitialized);
        }

        let milestone = Milestone {
            id,
            description,
            amount,
            status: MilestoneStatus::Pending,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Milestone(id), &milestone);
        let mut ids: Vec<u32> = env
            .storage()
            .instance()
            .get(&DataKey::MilestoneIds)
            .unwrap_or_else(|| Vec::new(&env));
        ids.push_back(id);
        env.storage().instance().set(&DataKey::MilestoneIds, &ids);

        Ok(())
    }

    pub fn deposit(env: Env, from: Address, amount: i128) -> Result<(), EscrowError> {
        from.require_auth();
        if amount <= 0 {
            return Err(EscrowError::InvalidAmount);
        }
        let mut config: EscrowConfig = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(EscrowError::NotInitialized)?;

        if config.status != EscrowStatus::Active {
            return Err(EscrowError::InvalidState);
        }
        if config.deposited_amount.saturating_add(amount) > config.total_amount {
            return Err(EscrowError::ExceedsTotal);
        }

        let contract_address = env.current_contract_address();
        let token_client = token::Client::new(&env, &config.token);
        token_client.transfer(&from, &contract_address, &amount);

        config.deposited_amount = config.deposited_amount.saturating_add(amount);
        env.storage().instance().set(&DataKey::Config, &config);

        FundsDeposited {
            from,
            amount,
            total_deposited: config.deposited_amount,
        }
        .publish(&env);

        Ok(())
    }

    pub fn approve_milestone(
        env: Env,
        caller: Address,
        milestone_id: u32,
    ) -> Result<(), EscrowError> {
        caller.require_auth();
        let mut config: EscrowConfig = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(EscrowError::NotInitialized)?;

        if caller != config.buyer && caller != config.arbiter {
            return Err(EscrowError::Unauthorized);
        }
        if config.status != EscrowStatus::Active && config.status != EscrowStatus::Disputed {
            return Err(EscrowError::InvalidState);
        }

        let mut milestone: Milestone = env
            .storage()
            .persistent()
            .get(&DataKey::Milestone(milestone_id))
            .ok_or(EscrowError::MilestoneNotFound)?;

        if milestone.status != MilestoneStatus::Pending {
            return Err(EscrowError::MilestoneAlreadyApproved);
        }

        let unreleased = config
            .deposited_amount
            .saturating_sub(config.released_amount);
        if milestone.amount > unreleased {
            return Err(EscrowError::InsufficientDeposit);
        }

        milestone.status = MilestoneStatus::Released;
        config.released_amount = config.released_amount.saturating_add(milestone.amount);

        let contract_address = env.current_contract_address();
        let token_client = token::Client::new(&env, &config.token);
        token_client.transfer(&contract_address, &config.seller, &milestone.amount);

        if config.released_amount >= config.total_amount {
            config.status = EscrowStatus::Completed;
        }

        env.storage()
            .persistent()
            .set(&DataKey::Milestone(milestone_id), &milestone);
        env.storage().instance().set(&DataKey::Config, &config);

        MilestoneApproved {
            milestone_id,
            amount: milestone.amount,
            approved_by: caller,
        }
        .publish(&env);

        Ok(())
    }

    pub fn raise_dispute(env: Env, caller: Address, reason: Symbol) -> Result<(), EscrowError> {
        caller.require_auth();
        let mut config: EscrowConfig = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(EscrowError::NotInitialized)?;

        if caller != config.buyer && caller != config.seller {
            return Err(EscrowError::Unauthorized);
        }
        if config.status != EscrowStatus::Active {
            return Err(EscrowError::InvalidState);
        }

        config.status = EscrowStatus::Disputed;
        env.storage().instance().set(&DataKey::Config, &config);

        DisputeRaised {
            raised_by: caller,
            reason,
        }
        .publish(&env);

        Ok(())
    }

    pub fn resolve_dispute(
        env: Env,
        caller: Address,
        buyer_payout: i128,
        seller_payout: i128,
    ) -> Result<(), EscrowError> {
        caller.require_auth();
        let mut config: EscrowConfig = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(EscrowError::NotInitialized)?;

        if caller != config.arbiter {
            return Err(EscrowError::Unauthorized);
        }
        if config.status != EscrowStatus::Disputed {
            return Err(EscrowError::InvalidState);
        }
        if buyer_payout < 0 || seller_payout < 0 {
            return Err(EscrowError::InvalidAmount);
        }

        let unreleased = config
            .deposited_amount
            .saturating_sub(config.released_amount);
        if buyer_payout.saturating_add(seller_payout) != unreleased {
            return Err(EscrowError::PayoutMismatch);
        }

        let contract_address = env.current_contract_address();
        let token_client = token::Client::new(&env, &config.token);
        if buyer_payout > 0 {
            token_client.transfer(&contract_address, &config.buyer, &buyer_payout);
        }
        if seller_payout > 0 {
            token_client.transfer(&contract_address, &config.seller, &seller_payout);
        }

        config.released_amount = config.released_amount.saturating_add(unreleased);
        config.status = EscrowStatus::Resolved;
        env.storage().instance().set(&DataKey::Config, &config);

        DisputeResolved {
            arbiter: caller,
            buyer_payout,
            seller_payout,
        }
        .publish(&env);

        Ok(())
    }

    pub fn claim_refund(env: Env, caller: Address) -> Result<i128, EscrowError> {
        caller.require_auth();
        let mut config: EscrowConfig = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(EscrowError::NotInitialized)?;

        if caller != config.buyer {
            return Err(EscrowError::Unauthorized);
        }
        if config.status != EscrowStatus::Active && config.status != EscrowStatus::Disputed {
            return Err(EscrowError::InvalidState);
        }
        if env.ledger().sequence() < config.timeout_ledger {
            return Err(EscrowError::NotExpired);
        }

        let remaining = config
            .deposited_amount
            .saturating_sub(config.released_amount);
        if remaining > 0 {
            let contract_address = env.current_contract_address();
            let token_client = token::Client::new(&env, &config.token);
            token_client.transfer(&contract_address, &config.buyer, &remaining);
        }

        config.released_amount = config.deposited_amount;
        config.status = EscrowStatus::Refunded;
        env.storage().instance().set(&DataKey::Config, &config);

        EscrowRefunded {
            buyer: config.buyer,
            amount: remaining,
        }
        .publish(&env);

        Ok(remaining)
    }

    pub fn get_config(env: Env) -> Result<EscrowConfig, EscrowError> {
        env.storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(EscrowError::NotInitialized)
    }

    pub fn get_status(env: Env) -> Result<EscrowStatus, EscrowError> {
        let config = Self::get_config(env)?;
        Ok(config.status)
    }

    pub fn get_milestone(env: Env, id: u32) -> Option<Milestone> {
        env.storage().persistent().get(&DataKey::Milestone(id))
    }

    pub fn get_remaining_funds(env: Env) -> Result<i128, EscrowError> {
        let config = Self::get_config(env)?;
        Ok(config
            .deposited_amount
            .saturating_sub(config.released_amount))
    }
}

#[cfg(test)]
mod test;
