#![no_std]

//! A time-delayed M-of-N multisig treasury vault.
//!
//! Signers can propose withdrawals, approve them until the configured
//! threshold is met, and execute them only after a mandatory timelock has
//! elapsed. Any signer may cancel a proposal until it is executed.

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, Address, Env, Vec,
};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Signers,
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
    pub cancelled: bool,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum VaultError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    InvalidThreshold = 3,
    InvalidTimelock = 4,
    EmptySigners = 5,
    DuplicateSigner = 6,
    Unauthorized = 7,
    InvalidAmount = 8,
    InsufficientBalance = 9,
    ProposalNotFound = 10,
    AlreadyApproved = 11,
    AlreadyExecuted = 12,
    AlreadyCancelled = 13,
    ThresholdNotMet = 14,
    TimelockNotElapsed = 15,
    InvalidRecipient = 16,
}

#[contractevent]
pub struct VaultInitialized {
    #[topic]
    pub admin: Address,
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
    pub recipient: Address,
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
pub struct WithdrawalExecuted {
    #[topic]
    pub proposal_id: u64,
    pub recipient: Address,
    pub amount: i128,
}

#[contractevent]
pub struct WithdrawalCancelled {
    #[topic]
    pub canceller: Address,
    pub proposal_id: u64,
}

#[contract]
pub struct MultisigVault;

#[contractimpl]
impl MultisigVault {
    pub fn initialize(
        env: Env,
        admin: Address,
        signers: Vec<Address>,
        threshold: u32,
        timelock: u64,
    ) -> Result<(), VaultError> {
        admin.require_auth();

        if env.storage().instance().has(&DataKey::Threshold) {
            return Err(VaultError::AlreadyInitialized);
        }
        if signers.is_empty() {
            return Err(VaultError::EmptySigners);
        }
        if threshold == 0 || threshold > signers.len() {
            return Err(VaultError::InvalidThreshold);
        }
        if timelock == 0 {
            return Err(VaultError::InvalidTimelock);
        }
        ensure_unique_signers(&signers)?;

        env.storage().instance().set(&DataKey::Signers, &signers);
        env.storage()
            .instance()
            .set(&DataKey::Threshold, &threshold);
        env.storage().instance().set(&DataKey::Timelock, &timelock);
        env.storage().instance().set(&DataKey::Balance, &0i128);
        env.storage()
            .instance()
            .set(&DataKey::NextProposalId, &0u64);

        VaultInitialized {
            admin,
            threshold,
            timelock,
        }
        .publish(&env);
        Ok(())
    }

    pub fn deposit(env: Env, from: Address, amount: i128) -> Result<(), VaultError> {
        require_initialized(&env)?;
        from.require_auth();
        if amount <= 0 {
            return Err(VaultError::InvalidAmount);
        }

        let balance = read_balance(&env)
            .checked_add(amount)
            .ok_or(VaultError::InvalidAmount)?;
        env.storage().instance().set(&DataKey::Balance, &balance);

        Deposited { from, amount }.publish(&env);
        Ok(())
    }

    pub fn propose_withdrawal(
        env: Env,
        proposer: Address,
        recipient: Address,
        amount: i128,
    ) -> Result<u64, VaultError> {
        require_initialized(&env)?;
        proposer.require_auth();
        if !is_authorized_signer(&env, &proposer) {
            return Err(VaultError::Unauthorized);
        }
        if amount <= 0 {
            return Err(VaultError::InvalidAmount);
        }
        if amount > read_balance(&env) {
            return Err(VaultError::InsufficientBalance);
        }

        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextProposalId)
            .unwrap_or(0);
        let proposal = Proposal {
            id,
            proposer: proposer.clone(),
            recipient: recipient.clone(),
            amount,
            approvals: Vec::new(&env),
            created_at: env.ledger().timestamp(),
            ready_at: 0,
            executed: false,
            cancelled: false,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Proposal(id), &proposal);
        env.storage()
            .instance()
            .set(&DataKey::NextProposalId, &(id + 1));

        WithdrawalProposed {
            proposer,
            proposal_id: id,
            recipient,
            amount,
        }
        .publish(&env);
        Ok(id)
    }

    pub fn approve(env: Env, approver: Address, proposal_id: u64) -> Result<(), VaultError> {
        require_initialized(&env)?;
        approver.require_auth();
        if !is_authorized_signer(&env, &approver) {
            return Err(VaultError::Unauthorized);
        }

        let mut proposal = load_proposal(&env, proposal_id)?;
        if proposal.executed {
            return Err(VaultError::AlreadyExecuted);
        }
        if proposal.cancelled {
            return Err(VaultError::AlreadyCancelled);
        }
        if contains_address(&proposal.approvals, &approver) {
            return Err(VaultError::AlreadyApproved);
        }

        proposal.approvals.push_back(approver.clone());
        let threshold = read_threshold(&env);
        if proposal.ready_at == 0 && proposal.approvals.len() >= threshold {
            let timelock = read_timelock(&env);
            proposal.ready_at = env.ledger().timestamp().saturating_add(timelock);
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

    pub fn execute(env: Env, executor: Address, proposal_id: u64) -> Result<(), VaultError> {
        require_initialized(&env)?;
        executor.require_auth();
        if !is_authorized_signer(&env, &executor) {
            return Err(VaultError::Unauthorized);
        }

        let mut proposal = load_proposal(&env, proposal_id)?;
        if proposal.executed {
            return Err(VaultError::AlreadyExecuted);
        }
        if proposal.cancelled {
            return Err(VaultError::AlreadyCancelled);
        }

        let threshold = read_threshold(&env);
        if proposal.approvals.len() < threshold {
            return Err(VaultError::ThresholdNotMet);
        }
        if proposal.ready_at == 0 {
            return Err(VaultError::ThresholdNotMet);
        }
        if env.ledger().timestamp() < proposal.ready_at {
            return Err(VaultError::TimelockNotElapsed);
        }

        let current_balance = read_balance(&env);
        if current_balance < proposal.amount {
            return Err(VaultError::InsufficientBalance);
        }
        let remaining = current_balance
            .checked_sub(proposal.amount)
            .ok_or(VaultError::InsufficientBalance)?;
        env.storage().instance().set(&DataKey::Balance, &remaining);

        proposal.executed = true;
        env.storage()
            .persistent()
            .set(&DataKey::Proposal(proposal_id), &proposal);

        WithdrawalExecuted {
            proposal_id,
            recipient: proposal.recipient.clone(),
            amount: proposal.amount,
        }
        .publish(&env);
        Ok(())
    }

    pub fn cancel(env: Env, canceller: Address, proposal_id: u64) -> Result<(), VaultError> {
        require_initialized(&env)?;
        canceller.require_auth();
        if !is_authorized_signer(&env, &canceller) {
            return Err(VaultError::Unauthorized);
        }

        let mut proposal = load_proposal(&env, proposal_id)?;
        if proposal.executed {
            return Err(VaultError::AlreadyExecuted);
        }
        if proposal.cancelled {
            return Err(VaultError::AlreadyCancelled);
        }

        proposal.cancelled = true;
        env.storage()
            .persistent()
            .set(&DataKey::Proposal(proposal_id), &proposal);

        WithdrawalCancelled {
            canceller,
            proposal_id,
        }
        .publish(&env);
        Ok(())
    }

    pub fn balance(env: Env) -> i128 {
        read_balance(&env)
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
        env.storage()
            .instance()
            .get(&DataKey::NextProposalId)
            .unwrap_or(0)
    }

    pub fn proposal(env: Env, proposal_id: u64) -> Option<Proposal> {
        env.storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
    }

    pub fn is_signer(env: Env, address: Address) -> bool {
        is_authorized_signer(&env, &address)
    }
}

fn require_initialized(env: &Env) -> Result<(), VaultError> {
    if env.storage().instance().has(&DataKey::Threshold) {
        Ok(())
    } else {
        Err(VaultError::NotInitialized)
    }
}

fn ensure_unique_signers(signers: &Vec<Address>) -> Result<(), VaultError> {
    for i in 0..signers.len() {
        let Some(candidate) = signers.get(i) else {
            continue;
        };
        for j in (i + 1)..signers.len() {
            if let Some(other) = signers.get(j) {
                if candidate == other {
                    return Err(VaultError::DuplicateSigner);
                }
            }
        }
    }
    Ok(())
}

fn contains_address(addresses: &Vec<Address>, needle: &Address) -> bool {
    for i in 0..addresses.len() {
        if let Some(candidate) = addresses.get(i) {
            if &candidate == needle {
                return true;
            }
        }
    }
    false
}

fn is_authorized_signer(env: &Env, address: &Address) -> bool {
    contains_address(&read_signers(env), address)
}

fn read_signers(env: &Env) -> Vec<Address> {
    env.storage()
        .instance()
        .get(&DataKey::Signers)
        .unwrap_or_else(|| Vec::new(env))
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

fn load_proposal(env: &Env, proposal_id: u64) -> Result<Proposal, VaultError> {
    env.storage()
        .persistent()
        .get(&DataKey::Proposal(proposal_id))
        .ok_or(VaultError::ProposalNotFound)
}
