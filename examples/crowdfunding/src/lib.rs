#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, token, Address, Env, Map};

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
enum DataKey {
    Campaign(u32),
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct Campaign {
    pub creator: Address,
    pub token: Address,
    pub goal: i128,
    pub deadline: u64,
    pub milestones: u32,
    pub released_milestones: u32,
    pub raised: i128,
    pub released: i128,
    pub backers: Map<Address, i128>,
    pub failed: bool,
}

fn key(id: u32) -> DataKey {
    DataKey::Campaign(id)
}
fn load(env: &Env, id: u32) -> Campaign {
    env.storage()
        .persistent()
        .get(&key(id))
        .expect("campaign missing")
}
fn save(env: &Env, id: u32, campaign: &Campaign) {
    env.storage().persistent().set(&key(id), campaign);
}

#[contract]
pub struct Crowdfunding;

#[contractimpl]
impl Crowdfunding {
    pub fn create(
        env: Env,
        id: u32,
        creator: Address,
        token: Address,
        goal: i128,
        deadline: u64,
        milestones: u32,
    ) {
        creator.require_auth();
        assert!(goal > 0, "goal must be positive");
        assert!(
            deadline > env.ledger().timestamp(),
            "deadline must be in the future"
        );
        assert!(milestones > 0, "milestones must be positive");
        assert!(
            !env.storage().persistent().has(&key(id)),
            "campaign already exists"
        );
        let campaign = Campaign {
            creator,
            token,
            goal,
            deadline,
            raised: 0,
            released: 0,
            milestones,
            released_milestones: 0,
            backers: Map::new(&env),
            failed: false,
        };
        save(&env, id, &campaign);
    }

    pub fn contribute(env: Env, id: u32, backer: Address, amount: i128) {
        backer.require_auth();
        assert!(amount > 0, "amount must be positive");
        let mut campaign = load(&env, id);
        assert!(
            env.ledger().timestamp() < campaign.deadline,
            "campaign ended"
        );
        assert!(!campaign.failed, "campaign failed");
        let old = campaign.backers.get(backer.clone()).unwrap_or(0);
        campaign.backers.set(backer.clone(), old + amount);
        campaign.raised += amount;
        token::Client::new(&env, &campaign.token).transfer(
            &backer,
            &env.current_contract_address(),
            &amount,
        );
        save(&env, id, &campaign);
    }

    /// Releases at most one equal milestone tranche after the goal is reached.
    pub fn release_milestone(env: Env, id: u32, creator: Address) -> i128 {
        creator.require_auth();
        let mut campaign = load(&env, id);
        assert_eq!(creator, campaign.creator, "not creator");
        assert!(campaign.raised >= campaign.goal, "goal not reached");
        assert!(
            campaign.released_milestones < campaign.milestones,
            "all milestones released"
        );
        let tranche = campaign.goal / campaign.milestones as i128;
        let amount = if campaign.released_milestones + 1 == campaign.milestones {
            campaign.raised - campaign.released
        } else {
            tranche
        };
        assert!(amount > 0, "all milestones released");
        campaign.released += amount;
        campaign.released_milestones += 1;
        token::Client::new(&env, &campaign.token).transfer(
            &env.current_contract_address(),
            &creator,
            &amount,
        );
        save(&env, id, &campaign);
        amount
    }

    pub fn finalize_failure(env: Env, id: u32) {
        let mut campaign = load(&env, id);
        assert!(
            env.ledger().timestamp() >= campaign.deadline,
            "deadline not reached"
        );
        assert!(campaign.raised < campaign.goal, "goal reached");
        campaign.failed = true;
        save(&env, id, &campaign);
    }

    pub fn claim_refund(env: Env, id: u32, backer: Address) -> i128 {
        backer.require_auth();
        let mut campaign = load(&env, id);
        assert!(campaign.failed, "campaign has not failed");
        let amount = campaign.backers.get(backer.clone()).unwrap_or(0);
        assert!(amount > 0, "nothing to refund");
        campaign.backers.set(backer.clone(), 0);
        token::Client::new(&env, &campaign.token).transfer(
            &env.current_contract_address(),
            &backer,
            &amount,
        );
        save(&env, id, &campaign);
        amount
    }

    pub fn campaign(env: Env, id: u32) -> Campaign {
        load(&env, id)
    }
}
