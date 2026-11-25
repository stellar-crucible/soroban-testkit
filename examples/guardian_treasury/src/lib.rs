#![no_std]

//! A timelocked M-of-N treasury where a designated Guardian can veto a
//! threshold-approved withdrawal during its delay window.

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, Address, Env, Vec,
};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Signers,
    Guardian,
    Threshold,
    Timelock,
    Balance,
    NextProposalId,
    Proposal(u64),
}

#[contracttype]
#[derive(Clone)]
pub struct Proposal {
    pub id: u64,
    pub proposer: Address,
    pub recipient: Address,
    pub amount: i128,
    pub approvals: Vec<Address>,
    pub created_at: u64,
    pub ready_at: u64,
    pub executed: bool,
    pub vetoed: bool,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum TreasuryError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    EmptySigners = 3,
    InvalidThreshold = 4,
    InvalidTimelock = 5,
    DuplicateSigner = 6,
    Unauthorized = 7,
    GuardianUnauthorized = 8,
    InvalidAmount = 9,
    InsufficientBalance = 10,
    ProposalNotFound = 11,
    AlreadyApproved = 12,
    ThresholdNotMet = 13,
    TimelockNotElapsed = 14,
    AlreadyExecuted = 15,
    AlreadyVetoed = 16,
    VetoWindowClosed = 17,
    CounterOverflow = 18,
}

#[contractevent]
pub struct TreasuryInitialized {
    #[topic]
    pub admin: Address,
    pub guardian: Address,
    pub threshold: u32,
    pub timelock: u64,
}

#[contractevent]
pub struct Deposited {
    #[topic]
    pub from: Address,
    pub amount: i128,
}

#[contractevent]
pub struct WithdrawalProposed {
    #[topic]
    pub proposer: Address,
    pub proposal_id: u64,
    pub amount: i128,
}

#[contractevent]
pub struct WithdrawalApproved {
    #[topic]
    pub approver: Address,
    pub proposal_id: u64,
    pub approvals: u32,
}

#[contractevent]
pub struct WithdrawalVetoed {
    #[topic]
    pub guardian: Address,
    pub proposal_id: u64,
}

#[contractevent]
pub struct WithdrawalExecuted {
    #[topic]
    pub proposal_id: u64,
    pub recipient: Address,
    pub amount: i128,
}

#[contract]
pub struct GuardianTreasury;

#[contractimpl]
impl GuardianTreasury {
    pub fn initialize(
        env: Env,
        admin: Address,
        signers: Vec<Address>,
        threshold: u32,
        timelock: u64,
        guardian: Address,
    ) -> Result<(), TreasuryError> {
        admin.require_auth();
        if env.storage().instance().has(&DataKey::Threshold) {
            return Err(TreasuryError::AlreadyInitialized);
        }
        if signers.is_empty() {
            return Err(TreasuryError::EmptySigners);
        }
        if threshold == 0 || threshold > signers.len() {
            return Err(TreasuryError::InvalidThreshold);
        }
        if timelock == 0 {
            return Err(TreasuryError::InvalidTimelock);
        }
        ensure_unique_signers(&signers)?;

        env.storage().instance().set(&DataKey::Signers, &signers);
        env.storage().instance().set(&DataKey::Guardian, &guardian);
        env.storage()
            .instance()
            .set(&DataKey::Threshold, &threshold);
        env.storage().instance().set(&DataKey::Timelock, &timelock);
        env.storage().instance().set(&DataKey::Balance, &0i128);
        env.storage()
            .instance()
            .set(&DataKey::NextProposalId, &0u64);

        TreasuryInitialized {
            admin,
            guardian,
            threshold,
            timelock,
        }
        .publish(&env);
        Ok(())
    }

    pub fn deposit(env: Env, from: Address, amount: i128) -> Result<(), TreasuryError> {
        require_initialized(&env)?;
        from.require_auth();
        if amount <= 0 {
            return Err(TreasuryError::InvalidAmount);
        }
        let balance = read_balance(&env)
            .checked_add(amount)
            .ok_or(TreasuryError::InvalidAmount)?;
        env.storage().instance().set(&DataKey::Balance, &balance);
        Deposited { from, amount }.publish(&env);
        Ok(())
    }

    pub fn propose(
        env: Env,
        proposer: Address,
        recipient: Address,
        amount: i128,
    ) -> Result<u64, TreasuryError> {
        require_initialized(&env)?;
        proposer.require_auth();
        if !is_signer(&env, &proposer) {
            return Err(TreasuryError::Unauthorized);
        }
        if amount <= 0 {
            return Err(TreasuryError::InvalidAmount);
        }
        if amount > read_balance(&env) {
            return Err(TreasuryError::InsufficientBalance);
        }
        let id = next_proposal_id(&env);
        let next = id.checked_add(1).ok_or(TreasuryError::CounterOverflow)?;
        let proposal = Proposal {
            id,
            proposer: proposer.clone(),
            recipient,
            amount,
            approvals: Vec::new(&env),
            created_at: env.ledger().timestamp(),
            ready_at: 0,
            executed: false,
            vetoed: false,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Proposal(id), &proposal);
        env.storage()
            .instance()
            .set(&DataKey::NextProposalId, &next);
        WithdrawalProposed {
            proposer,
            proposal_id: id,
            amount,
        }
        .publish(&env);
        Ok(id)
    }

    pub fn approve(env: Env, approver: Address, proposal_id: u64) -> Result<(), TreasuryError> {
        require_initialized(&env)?;
        approver.require_auth();
        if !is_signer(&env, &approver) {
            return Err(TreasuryError::Unauthorized);
        }
        let mut proposal = load_proposal(&env, proposal_id)?;
        if proposal.executed {
            return Err(TreasuryError::AlreadyExecuted);
        }
        if proposal.vetoed {
            return Err(TreasuryError::AlreadyVetoed);
        }
        if contains(&proposal.approvals, &approver) {
            return Err(TreasuryError::AlreadyApproved);
        }
        proposal.approvals.push_back(approver.clone());
        if proposal.ready_at == 0 && proposal.approvals.len() >= read_threshold(&env) {
            proposal.ready_at = env.ledger().timestamp().saturating_add(read_timelock(&env));
        }
        let approvals = proposal.approvals.len();
        env.storage()
            .persistent()
            .set(&DataKey::Proposal(proposal_id), &proposal);
        WithdrawalApproved {
            approver,
            proposal_id,
            approvals,
        }
        .publish(&env);
        Ok(())
    }

    /// Guardian-only cancellation, accepted only after threshold and before
    /// the proposal's timelock expires.
    pub fn veto(env: Env, guardian: Address, proposal_id: u64) -> Result<(), TreasuryError> {
        require_initialized(&env)?;
        guardian.require_auth();
        if read_guardian(&env).as_ref() != Some(&guardian) {
            return Err(TreasuryError::GuardianUnauthorized);
        }
        let mut proposal = load_proposal(&env, proposal_id)?;
        if proposal.executed {
            return Err(TreasuryError::AlreadyExecuted);
        }
        if proposal.vetoed {
            return Err(TreasuryError::AlreadyVetoed);
        }
        if proposal.ready_at == 0 || env.ledger().timestamp() >= proposal.ready_at {
            return Err(TreasuryError::VetoWindowClosed);
        }
        proposal.vetoed = true;
        env.storage()
            .persistent()
            .set(&DataKey::Proposal(proposal_id), &proposal);
        WithdrawalVetoed {
            guardian,
            proposal_id,
        }
        .publish(&env);
        Ok(())
    }

    pub fn execute(env: Env, executor: Address, proposal_id: u64) -> Result<(), TreasuryError> {
        require_initialized(&env)?;
        executor.require_auth();
        if !is_signer(&env, &executor) {
            return Err(TreasuryError::Unauthorized);
        }
        let mut proposal = load_proposal(&env, proposal_id)?;
        if proposal.executed {
            return Err(TreasuryError::AlreadyExecuted);
        }
        if proposal.vetoed {
            return Err(TreasuryError::AlreadyVetoed);
        }
        if proposal.approvals.len() < read_threshold(&env) || proposal.ready_at == 0 {
            return Err(TreasuryError::ThresholdNotMet);
        }
        if env.ledger().timestamp() < proposal.ready_at {
            return Err(TreasuryError::TimelockNotElapsed);
        }
        let balance = read_balance(&env);
        let remaining = balance
            .checked_sub(proposal.amount)
            .ok_or(TreasuryError::InsufficientBalance)?;
        env.storage().instance().set(&DataKey::Balance, &remaining);
        proposal.executed = true;
        env.storage()
            .persistent()
            .set(&DataKey::Proposal(proposal_id), &proposal);
        WithdrawalExecuted {
            proposal_id,
            recipient: proposal.recipient,
            amount: proposal.amount,
        }
        .publish(&env);
        Ok(())
    }

    pub fn balance(env: Env) -> i128 {
        read_balance(&env)
    }

    pub fn guardian(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Guardian)
    }

    pub fn threshold(env: Env) -> u32 {
        read_threshold(&env)
    }

    pub fn timelock(env: Env) -> u64 {
        read_timelock(&env)
    }

    pub fn signers(env: Env) -> Vec<Address> {
        read_signers(&env)
    }

    pub fn next_proposal_id(env: Env) -> u64 {
        next_proposal_id(&env)
    }

    pub fn proposal(env: Env, proposal_id: u64) -> Option<Proposal> {
        env.storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
    }
}

fn require_initialized(env: &Env) -> Result<(), TreasuryError> {
    if env.storage().instance().has(&DataKey::Threshold) {
        Ok(())
    } else {
        Err(TreasuryError::NotInitialized)
    }
}

fn ensure_unique_signers(signers: &Vec<Address>) -> Result<(), TreasuryError> {
    for i in 0..signers.len() {
        for j in (i + 1)..signers.len() {
            if signers.get(i) == signers.get(j) {
                return Err(TreasuryError::DuplicateSigner);
            }
        }
    }
    Ok(())
}

fn contains(addresses: &Vec<Address>, needle: &Address) -> bool {
    (0..addresses.len()).any(|i| addresses.get(i).as_ref() == Some(needle))
}

fn is_signer(env: &Env, address: &Address) -> bool {
    contains(&read_signers(env), address)
}

fn read_signers(env: &Env) -> Vec<Address> {
    env.storage()
        .instance()
        .get(&DataKey::Signers)
        .unwrap_or_else(|| Vec::new(env))
}

fn read_guardian(env: &Env) -> Option<Address> {
    env.storage().instance().get(&DataKey::Guardian)
}

fn read_threshold(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::Threshold)
        .unwrap_or(0)
}

fn read_timelock(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::Timelock)
        .unwrap_or(0)
}

fn read_balance(env: &Env) -> i128 {
    env.storage().instance().get(&DataKey::Balance).unwrap_or(0)
}

fn next_proposal_id(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::NextProposalId)
        .unwrap_or(0)
}

fn load_proposal(env: &Env, id: u64) -> Result<Proposal, TreasuryError> {
    env.storage()
        .persistent()
        .get(&DataKey::Proposal(id))
        .ok_or(TreasuryError::ProposalNotFound)
}
