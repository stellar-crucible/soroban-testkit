#![no_std]

//! A multi-party governance DAO with weighted voting, a quorum and a timelock.
//!
//! Members hold non-transferable voting power, minted by the admin at launch
//! and afterwards only by passed proposals. Any member can open a proposal;
//! every member votes once, weighted by their power. When the voting period
//! ends, anyone can `queue` the proposal: it passes if turnout reaches the
//! quorum (a share of the supply snapshotted when the proposal opened) and the
//! votes for outnumber the votes against. A passed proposal waits out the
//! timelock and then anyone can `execute` it.
//!
//! ```text
//! propose ──► Pending ──(voting period ends)──► queue ─┬─► Queued ──(timelock)──► execute ──► Executed
//!               │                                      └─► Defeated
//!               └─(proposer)──► cancel ──► Cancelled
//! ```
//!
//! Storage tiers:
//!
//! - instance: configuration, total supply, the proposal counter;
//! - persistent: voting power, proposals, vote receipts.

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, Address, Env, Symbol,
};

/// Basis-point denominator for the quorum.
pub const MAX_BPS: u32 = 10_000;

/// Persistent entries are extended to this many ledgers whenever written.
const PERSISTENT_TTL: u32 = 518_400;
const PERSISTENT_THRESHOLD: u32 = PERSISTENT_TTL / 2;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Admin,
    QuorumBps,
    VotingPeriod,
    Timelock,
    TotalSupply,
    NextProposalId,
    Power(Address),
    Proposal(u64),
    Receipt(u64, Address),
}

/// Where a proposal is in its life cycle.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProposalStatus {
    /// Open for votes until `end`.
    Pending,
    /// Passed; executable once the ledger reaches `eta`.
    Queued,
    /// Applied.
    Executed,
    /// Missed the quorum or lost the vote.
    Defeated,
    /// Withdrawn by its proposer while voting was open.
    Cancelled,
}

/// What a proposal does when executed.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    /// Change the quorum, in basis points of the supply.
    SetQuorum(u32),
    /// Change the voting period, in seconds.
    SetVotingPeriod(u64),
    /// Change the timelock, in seconds.
    SetTimelock(u64),
    /// Mint voting power to an address.
    Mint(Address, i128),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Proposal {
    pub id: u64,
    pub proposer: Address,
    pub title: Symbol,
    pub action: Action,
    /// Voting opens at this timestamp…
    pub start: u64,
    /// …and closes at this one (exclusive).
    pub end: u64,
    /// Earliest execution timestamp; zero until queued.
    pub eta: u64,
    pub votes_for: i128,
    pub votes_against: i128,
    /// Total supply when the proposal opened; the quorum is a share of it.
    pub supply_snapshot: i128,
    pub status: ProposalStatus,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum GovernanceError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    InvalidQuorum = 3,
    InvalidPeriod = 4,
    InvalidAmount = 5,
    NoVotingPower = 6,
    ProposalNotFound = 7,
    VotingClosed = 8,
    VotingOpen = 9,
    AlreadyVoted = 10,
    NotPending = 11,
    NotQueued = 12,
    TimelockNotElapsed = 13,
    NotProposer = 14,
}

#[contractevent]
pub struct Initialized {
    #[topic]
    pub admin: Address,
    pub quorum_bps: u32,
    pub voting_period: u64,
    pub timelock: u64,
}

#[contractevent]
pub struct Minted {
    #[topic]
    pub to: Address,
    pub amount: i128,
}

#[contractevent]
pub struct ProposalCreated {
    #[topic]
    pub proposer: Address,
    #[topic]
    pub id: u64,
    pub title: Symbol,
}

#[contractevent]
pub struct VoteCast {
    #[topic]
    pub voter: Address,
    #[topic]
    pub id: u64,
    pub support: bool,
    pub weight: i128,
}

#[contractevent]
pub struct ProposalQueued {
    #[topic]
    pub id: u64,
    pub eta: u64,
}

#[contractevent]
pub struct ProposalDefeated {
    #[topic]
    pub id: u64,
}

#[contractevent]
pub struct ProposalExecuted {
    #[topic]
    pub id: u64,
}

#[contractevent]
pub struct ProposalCancelled {
    #[topic]
    pub id: u64,
}

#[contract]
pub struct Governance;

#[contractimpl]
impl Governance {
    /// Configure the DAO. `quorum_bps` is in `1..=10_000`; `voting_period`
    /// must be non-zero; `timelock` may be zero.
    pub fn initialize(
        env: Env,
        admin: Address,
        quorum_bps: u32,
        voting_period: u64,
        timelock: u64,
    ) -> Result<(), GovernanceError> {
        let instance = env.storage().instance();
        if instance.has(&DataKey::Admin) {
            return Err(GovernanceError::AlreadyInitialized);
        }
        check_quorum(quorum_bps)?;
        check_period(voting_period)?;
        admin.require_auth();
        instance.set(&DataKey::Admin, &admin);
        instance.set(&DataKey::QuorumBps, &quorum_bps);
        instance.set(&DataKey::VotingPeriod, &voting_period);
        instance.set(&DataKey::Timelock, &timelock);
        instance.set(&DataKey::TotalSupply, &0i128);
        instance.set(&DataKey::NextProposalId, &0u64);
        Initialized {
            admin,
            quorum_bps,
            voting_period,
            timelock,
        }
        .publish(&env);
        Ok(())
    }

    /// Mint voting power. Admin only.
    pub fn mint(env: Env, to: Address, amount: i128) -> Result<(), GovernanceError> {
        let admin = Self::admin(env.clone())?;
        admin.require_auth();
        mint_power(&env, &to, amount)
    }

    /// Open a proposal. The proposer must hold voting power.
    pub fn propose(
        env: Env,
        proposer: Address,
        title: Symbol,
        action: Action,
    ) -> Result<u64, GovernanceError> {
        ensure_initialized(&env)?;
        proposer.require_auth();
        if power_of(&env, &proposer) <= 0 {
            return Err(GovernanceError::NoVotingPower);
        }
        check_action(&action)?;

        let instance = env.storage().instance();
        let id: u64 = instance.get(&DataKey::NextProposalId).unwrap_or(0);
        instance.set(&DataKey::NextProposalId, &(id + 1));
        let voting_period: u64 = instance.get(&DataKey::VotingPeriod).unwrap_or(0);
        let now = env.ledger().timestamp();
        let proposal = Proposal {
            id,
            proposer: proposer.clone(),
            title: title.clone(),
            action,
            start: now,
            end: now.saturating_add(voting_period),
            eta: 0,
            votes_for: 0,
            votes_against: 0,
            supply_snapshot: total_supply(&env),
            status: ProposalStatus::Pending,
        };
        save_proposal(&env, &proposal);
        ProposalCreated {
            proposer,
            id,
            title,
        }
        .publish(&env);
        Ok(id)
    }

    /// Cast a weighted vote. Returns the weight counted.
    pub fn vote(env: Env, voter: Address, id: u64, support: bool) -> Result<i128, GovernanceError> {
        voter.require_auth();
        let mut proposal = load_proposal(&env, id)?;
        if proposal.status != ProposalStatus::Pending {
            return Err(GovernanceError::NotPending);
        }
        if env.ledger().timestamp() >= proposal.end {
            return Err(GovernanceError::VotingClosed);
        }
        let receipt = DataKey::Receipt(id, voter.clone());
        let persistent = env.storage().persistent();
        if persistent.has(&receipt) {
            return Err(GovernanceError::AlreadyVoted);
        }
        let weight = power_of(&env, &voter);
        if weight <= 0 {
            return Err(GovernanceError::NoVotingPower);
        }
        if support {
            proposal.votes_for += weight;
        } else {
            proposal.votes_against += weight;
        }
        persistent.set(&receipt, &weight);
        persistent.extend_ttl(&receipt, PERSISTENT_THRESHOLD, PERSISTENT_TTL);
        save_proposal(&env, &proposal);
        VoteCast {
            voter,
            id,
            support,
            weight,
        }
        .publish(&env);
        Ok(weight)
    }

    /// Close voting and either queue the proposal behind the timelock or mark
    /// it defeated. Anyone may call this once the voting period has ended.
    pub fn queue(env: Env, id: u64) -> Result<ProposalStatus, GovernanceError> {
        let mut proposal = load_proposal(&env, id)?;
        if proposal.status != ProposalStatus::Pending {
            return Err(GovernanceError::NotPending);
        }
        let now = env.ledger().timestamp();
        if now < proposal.end {
            return Err(GovernanceError::VotingOpen);
        }
        let turnout = proposal.votes_for + proposal.votes_against;
        let passed =
            turnout >= quorum_votes(&env, &proposal) && proposal.votes_for > proposal.votes_against;
        if passed {
            let timelock: u64 = env
                .storage()
                .instance()
                .get(&DataKey::Timelock)
                .unwrap_or(0);
            proposal.eta = now.saturating_add(timelock);
            proposal.status = ProposalStatus::Queued;
            ProposalQueued {
                id,
                eta: proposal.eta,
            }
            .publish(&env);
        } else {
            proposal.status = ProposalStatus::Defeated;
            ProposalDefeated { id }.publish(&env);
        }
        save_proposal(&env, &proposal);
        Ok(proposal.status)
    }

    /// Apply a queued proposal once its timelock has elapsed. Permissionless.
    pub fn execute(env: Env, id: u64) -> Result<(), GovernanceError> {
        let mut proposal = load_proposal(&env, id)?;
        if proposal.status != ProposalStatus::Queued {
            return Err(GovernanceError::NotQueued);
        }
        if env.ledger().timestamp() < proposal.eta {
            return Err(GovernanceError::TimelockNotElapsed);
        }
        let instance = env.storage().instance();
        match &proposal.action {
            Action::SetQuorum(bps) => instance.set(&DataKey::QuorumBps, bps),
            Action::SetVotingPeriod(period) => instance.set(&DataKey::VotingPeriod, period),
            Action::SetTimelock(timelock) => instance.set(&DataKey::Timelock, timelock),
            Action::Mint(to, amount) => mint_power(&env, to, *amount)?,
        }
        proposal.status = ProposalStatus::Executed;
        save_proposal(&env, &proposal);
        ProposalExecuted { id }.publish(&env);
        Ok(())
    }

    /// Withdraw a proposal while voting is still open. Proposer only.
    pub fn cancel(env: Env, proposer: Address, id: u64) -> Result<(), GovernanceError> {
        proposer.require_auth();
        let mut proposal = load_proposal(&env, id)?;
        if proposal.proposer != proposer {
            return Err(GovernanceError::NotProposer);
        }
        if proposal.status != ProposalStatus::Pending {
            return Err(GovernanceError::NotPending);
        }
        proposal.status = ProposalStatus::Cancelled;
        save_proposal(&env, &proposal);
        ProposalCancelled { id }.publish(&env);
        Ok(())
    }

    pub fn admin(env: Env) -> Result<Address, GovernanceError> {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(GovernanceError::NotInitialized)
    }

    pub fn proposal(env: Env, id: u64) -> Result<Proposal, GovernanceError> {
        load_proposal(&env, id)
    }

    pub fn voting_power(env: Env, account: Address) -> i128 {
        power_of(&env, &account)
    }

    pub fn total_supply(env: Env) -> i128 {
        total_supply(&env)
    }

    pub fn quorum_bps(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::QuorumBps)
            .unwrap_or(0)
    }

    pub fn voting_period(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::VotingPeriod)
            .unwrap_or(0)
    }

    pub fn timelock(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::Timelock)
            .unwrap_or(0)
    }

    pub fn proposal_count(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::NextProposalId)
            .unwrap_or(0)
    }

    /// The weight `voter` cast on proposal `id`, if they voted.
    pub fn receipt(env: Env, id: u64, voter: Address) -> Option<i128> {
        env.storage().persistent().get(&DataKey::Receipt(id, voter))
    }

    /// Votes (for + against) proposal `id` needs to reach its quorum.
    pub fn quorum_votes(env: Env, id: u64) -> Result<i128, GovernanceError> {
        let proposal = load_proposal(&env, id)?;
        Ok(quorum_votes(&env, &proposal))
    }
}

fn ensure_initialized(env: &Env) -> Result<(), GovernanceError> {
    if env.storage().instance().has(&DataKey::Admin) {
        Ok(())
    } else {
        Err(GovernanceError::NotInitialized)
    }
}

fn check_quorum(bps: u32) -> Result<(), GovernanceError> {
    if bps == 0 || bps > MAX_BPS {
        Err(GovernanceError::InvalidQuorum)
    } else {
        Ok(())
    }
}

fn check_period(period: u64) -> Result<(), GovernanceError> {
    if period == 0 {
        Err(GovernanceError::InvalidPeriod)
    } else {
        Ok(())
    }
}

fn check_action(action: &Action) -> Result<(), GovernanceError> {
    match action {
        Action::SetQuorum(bps) => check_quorum(*bps),
        Action::SetVotingPeriod(period) => check_period(*period),
        Action::SetTimelock(_) => Ok(()),
        Action::Mint(_, amount) if *amount <= 0 => Err(GovernanceError::InvalidAmount),
        Action::Mint(_, _) => Ok(()),
    }
}

fn total_supply(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalSupply)
        .unwrap_or(0)
}

fn power_of(env: &Env, account: &Address) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::Power(account.clone()))
        .unwrap_or(0)
}

fn mint_power(env: &Env, to: &Address, amount: i128) -> Result<(), GovernanceError> {
    ensure_initialized(env)?;
    if amount <= 0 {
        return Err(GovernanceError::InvalidAmount);
    }
    let key = DataKey::Power(to.clone());
    let persistent = env.storage().persistent();
    persistent.set(&key, &(power_of(env, to) + amount));
    persistent.extend_ttl(&key, PERSISTENT_THRESHOLD, PERSISTENT_TTL);
    env.storage()
        .instance()
        .set(&DataKey::TotalSupply, &(total_supply(env) + amount));
    Minted {
        to: to.clone(),
        amount,
    }
    .publish(env);
    Ok(())
}

fn quorum_votes(env: &Env, proposal: &Proposal) -> i128 {
    let bps: u32 = env
        .storage()
        .instance()
        .get(&DataKey::QuorumBps)
        .unwrap_or(MAX_BPS);
    // Round up, so a quorum of 1 bps on a tiny supply still needs a vote.
    (proposal.supply_snapshot * bps as i128 + (MAX_BPS as i128 - 1)) / MAX_BPS as i128
}

fn load_proposal(env: &Env, id: u64) -> Result<Proposal, GovernanceError> {
    env.storage()
        .persistent()
        .get(&DataKey::Proposal(id))
        .ok_or(GovernanceError::ProposalNotFound)
}

fn save_proposal(env: &Env, proposal: &Proposal) {
    let key = DataKey::Proposal(proposal.id);
    let persistent = env.storage().persistent();
    persistent.set(&key, proposal);
    persistent.extend_ttl(&key, PERSISTENT_THRESHOLD, PERSISTENT_TTL);
}
