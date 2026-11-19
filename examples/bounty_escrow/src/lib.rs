#![no_std]

use soroban_sdk::{
    contract, contractevent, contractimpl, contracttype, token, Address, Env, Map, Symbol, Vec,
};

/// Where a bounty is in its life: the creator's funds are locked from
/// `create` until exactly one of `released` or `refunded` happens.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BountyStatus {
    /// Funded and waiting for a contributor to claim it.
    Open,
    /// A contributor has submitted work, and reviewers are voting.
    Submitted,
    /// The reviewer threshold was reached and the escrow paid the contributor.
    Released,
    /// The deadline passed without a release and the creator took the funds back.
    Refunded,
}

/// One bounty's state.
///
/// The whole record lives under one persistent map entry, so a test can read
/// every field with one `get` and a storage snapshot sees two writes for the
/// same key rather than one per field.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct Bounty {
    /// Who funded the escrow.
    pub creator: Address,
    /// The SEP-41 token the escrow holds.
    pub token: Address,
    /// How much of `token` is locked.
    pub amount: i128,
    /// Who submitted work, once somebody has.
    pub contributor: Option<Address>,
    /// The addresses whose approvals count towards `threshold`.
    pub reviewers: Vec<Address>,
    /// The reviewers that have approved so far, in approval order.
    pub approvals: Vec<Address>,
    /// How many approvals release the funds.
    pub threshold: u32,
    /// Unix timestamp after which the creator may refund.
    pub deadline: u64,
    /// Current phase.
    pub status: BountyStatus,
}

/// A bounty was funded and its escrow opened.
#[contractevent(topics = ["bounty", "created"])]
#[derive(Clone)]
pub struct BountyCreated {
    pub id: u64,
    pub creator: Address,
    pub amount: i128,
    pub reviewers: u32,
    pub threshold: u32,
    pub deadline: u64,
}

/// A contributor claimed the bounty.
#[contractevent(topics = ["bounty", "submitted"])]
#[derive(Clone)]
pub struct BountySubmitted {
    pub id: u64,
    pub contributor: Address,
}

/// One reviewer approved, carrying the running count so a test can assert the
/// progress rather than only the last signature.
#[contractevent(topics = ["bounty", "approved"])]
#[derive(Clone)]
pub struct ReviewApproved {
    pub id: u64,
    pub reviewer: Address,
    pub approvals: u32,
    pub threshold: u32,
}

/// The threshold was reached and the escrow paid out.
#[contractevent(topics = ["bounty", "released"])]
#[derive(Clone)]
pub struct BountyReleased {
    pub id: u64,
    pub contributor: Address,
    pub amount: i128,
}

/// The deadline passed without a release and the creator was repaid.
#[contractevent(topics = ["bounty", "refunded"])]
#[derive(Clone)]
pub struct BountyRefunded {
    pub id: u64,
    pub creator: Address,
    pub amount: i128,
}

fn bounties_key(env: &Env) -> Symbol {
    Symbol::new(env, "bounties")
}

fn next_id_key(env: &Env) -> Symbol {
    Symbol::new(env, "next_id")
}

fn bounties(env: &Env) -> Map<u64, Bounty> {
    env.storage()
        .persistent()
        .get(&bounties_key(env))
        .unwrap_or_else(|| Map::new(env))
}

fn write_bounties(env: &Env, map: &Map<u64, Bounty>) {
    env.storage().persistent().set(&bounties_key(env), map);
}

fn read_bounty(env: &Env, id: u64) -> Bounty {
    bounties(env).get(id).expect("bounty not found")
}

fn store_bounty(env: &Env, id: u64, bounty: &Bounty) {
    let mut map = bounties(env);
    map.set(id, bounty.clone());
    write_bounties(env, &map);
}

fn next_id(env: &Env) -> u64 {
    env.storage().instance().get(&next_id_key(env)).unwrap_or(0)
}

fn has_reviewer(reviewers: &Vec<Address>, candidate: &Address) -> bool {
    reviewers.iter().any(|reviewer| reviewer == *candidate)
}

/// A bounty board whose escrow is the contract's own token balance.
///
/// A creator funds a task, a contributor claims it, and a quorum of named
/// reviewers releases the payment. If the quorum never forms before the
/// deadline, the creator takes the funds back instead.
#[contract]
pub struct BountyEscrowContract;

#[contractimpl]
impl BountyEscrowContract {
    /// Opens a bounty and locks `amount` of `token` in the contract.
    ///
    /// `reviewers` must be distinct addresses and `threshold` must fit inside
    /// them, because otherwise a "multi-signature release" could be satisfied by
    /// one key. The creator's authorization covers the token transfer as well.
    ///
    /// Returns the bounty's id, which every later call takes.
    pub fn create(
        env: Env,
        creator: Address,
        token: Address,
        amount: i128,
        reviewers: Vec<Address>,
        threshold: u32,
        deadline: u64,
    ) -> u64 {
        creator.require_auth();

        assert!(amount > 0, "amount must be positive");
        assert!(!reviewers.is_empty(), "at least one reviewer is required");
        assert!(
            threshold > 0 && threshold <= reviewers.len(),
            "threshold must be between 1 and the reviewer count"
        );
        assert!(
            deadline > env.ledger().timestamp(),
            "deadline must be in the future"
        );

        for index in 0..reviewers.len() {
            for other in (index + 1)..reviewers.len() {
                assert!(
                    reviewers.get(index).unwrap() != reviewers.get(other).unwrap(),
                    "reviewers must be unique"
                );
            }
        }

        let id = next_id(&env) + 1;
        env.storage().instance().set(&next_id_key(&env), &id);

        let bounty = Bounty {
            creator: creator.clone(),
            token: token.clone(),
            amount,
            contributor: None,
            reviewers: reviewers.clone(),
            approvals: Vec::new(&env),
            threshold,
            deadline,
            status: BountyStatus::Open,
        };
        store_bounty(&env, id, &bounty);

        // The escrow is the contract's own balance: the creator's funds leave
        // their account now, not when the release is approved.
        token::Client::new(&env, &token).transfer(
            &creator,
            env.current_contract_address(),
            &amount,
        );

        BountyCreated {
            id,
            creator,
            amount,
            reviewers: bounty.reviewers.len(),
            threshold,
            deadline,
        }
        .publish(&env);

        id
    }

    /// Records `contributor` as the worker for an open bounty.
    ///
    /// Submission is refused once the deadline has passed, so a dead bounty
    /// cannot be claimed and then stranded between release and refund.
    pub fn submit(env: Env, id: u64, contributor: Address) {
        contributor.require_auth();

        let mut bounty = read_bounty(&env, id);
        assert!(
            bounty.status == BountyStatus::Open,
            "bounty is not open for submissions"
        );
        assert!(
            env.ledger().timestamp() < bounty.deadline,
            "bounty deadline has passed"
        );

        bounty.contributor = Some(contributor.clone());
        bounty.status = BountyStatus::Submitted;
        store_bounty(&env, id, &bounty);

        BountySubmitted { id, contributor }.publish(&env);
    }

    /// Adds `reviewer`'s signature and releases the escrow when the threshold
    /// is reached.
    ///
    /// Only a listed reviewer may sign, each may sign once, and a signature
    /// after the deadline is refused: past that point the creator's refund is
    /// the only way the funds move.
    pub fn approve(env: Env, id: u64, reviewer: Address) {
        reviewer.require_auth();

        let mut bounty = read_bounty(&env, id);
        assert!(
            bounty.status == BountyStatus::Submitted,
            "bounty is not awaiting review"
        );
        assert!(
            env.ledger().timestamp() < bounty.deadline,
            "bounty deadline has passed"
        );
        assert!(
            has_reviewer(&bounty.reviewers, &reviewer),
            "address is not a reviewer for this bounty"
        );
        assert!(
            !has_reviewer(&bounty.approvals, &reviewer),
            "reviewer has already approved"
        );

        bounty.approvals.push_back(reviewer.clone());
        let approvals = bounty.approvals.len();
        let reached = approvals >= bounty.threshold;

        if reached {
            bounty.status = BountyStatus::Released;
        }
        store_bounty(&env, id, &bounty);

        ReviewApproved {
            id,
            reviewer,
            approvals,
            threshold: bounty.threshold,
        }
        .publish(&env);

        if reached {
            Self::pay(&env, id, &bounty);
        }
    }

    /// Returns the escrow to the creator once the deadline has passed.
    ///
    /// Refused while the bounty can still be released, and refused once it has
    /// already paid out, so the same funds cannot be moved twice.
    pub fn refund(env: Env, id: u64) {
        let mut bounty = read_bounty(&env, id);
        bounty.creator.require_auth();

        assert!(
            bounty.status == BountyStatus::Open || bounty.status == BountyStatus::Submitted,
            "bounty has already been settled"
        );
        assert!(
            env.ledger().timestamp() >= bounty.deadline,
            "bounty deadline has not passed"
        );

        bounty.status = BountyStatus::Refunded;
        store_bounty(&env, id, &bounty);

        token::Client::new(&env, &bounty.token).transfer(
            &env.current_contract_address(),
            &bounty.creator,
            &bounty.amount,
        );

        BountyRefunded {
            id,
            creator: bounty.creator,
            amount: bounty.amount,
        }
        .publish(&env);
    }

    /// Everything recorded about a bounty.
    pub fn get(env: Env, id: u64) -> Bounty {
        read_bounty(&env, id)
    }

    /// The bounty's current phase.
    pub fn status(env: Env, id: u64) -> BountyStatus {
        read_bounty(&env, id).status
    }

    /// How many signatures have been collected.
    pub fn approval_count(env: Env, id: u64) -> u32 {
        read_bounty(&env, id).approvals.len()
    }

    /// The reviewers whose signatures have been collected, in approval order.
    pub fn approvals(env: Env, id: u64) -> Vec<Address> {
        read_bounty(&env, id).approvals
    }

    /// The addresses eligible to release this bounty.
    pub fn reviewers(env: Env, id: u64) -> Vec<Address> {
        read_bounty(&env, id).reviewers
    }

    /// The id the next `create` will assign. Ids start at 1.
    pub fn next_bounty_id(env: Env) -> u64 {
        next_id(&env) + 1
    }

    /// Move the escrow to the contributor and publish the release.
    ///
    /// The caller has already stored `Released`, so a re-entrant token could
    /// not be paid twice even if it called back: `approve` and `refund` both
    /// reject a settled bounty.
    fn pay(env: &Env, id: u64, bounty: &Bounty) {
        let contributor = bounty
            .contributor
            .clone()
            .expect("a released bounty always has a contributor");

        token::Client::new(env, &bounty.token).transfer(
            &env.current_contract_address(),
            &contributor,
            &bounty.amount,
        );

        BountyReleased {
            id,
            contributor,
            amount: bounty.amount,
        }
        .publish(env);
    }
}

#[cfg(test)]
mod test;
