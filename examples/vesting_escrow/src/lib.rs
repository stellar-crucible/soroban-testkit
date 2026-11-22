#![no_std]
use soroban_sdk::{
    contract, contractevent, contractimpl, contracttype, symbol_short, token, Address, Env, Map,
    Symbol,
};

#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduleStatus {
    Active,
    Revoked,
    Completed,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VestingSchedule {
    pub admin: Address,
    pub beneficiary: Address,
    pub token: Address,
    pub total_amount: i128,
    pub claimed: i128,
    pub start: u64,
    pub cliff_duration: u64,
    pub duration: u64,
    pub revocable: bool,
    pub revoked_at: u64,
    pub returned: i128,
    pub status: ScheduleStatus,
}

#[contractevent(topics = ["vesting", "created"])]
pub struct ScheduleCreated {
    pub id: u64,
    pub admin: Address,
    pub beneficiary: Address,
    pub total_amount: i128,
    pub start: u64,
    pub cliff_duration: u64,
    pub duration: u64,
    pub revocable: bool,
}

#[contractevent(topics = ["vesting", "claimed"])]
pub struct TokensClaimed {
    pub id: u64,
    pub beneficiary: Address,
    pub amount: i128,
    pub total_claimed: i128,
}

#[contractevent(topics = ["vesting", "revoked"])]
pub struct ScheduleRevoked {
    pub id: u64,
    pub admin: Address,
    pub returned: i128,
    pub vested_at_revocation: i128,
}

#[contract]
pub struct VestingEscrow;

const SCHEDULES_KEY: Symbol = symbol_short!("sched");
const NEXT_ID_KEY: Symbol = symbol_short!("next");

fn schedules(env: &Env) -> Map<u64, VestingSchedule> {
    env.storage()
        .persistent()
        .get(&SCHEDULES_KEY)
        .unwrap_or_else(|| Map::new(env))
}

fn put_schedules(env: &Env, map: &Map<u64, VestingSchedule>) {
    env.storage().persistent().set(&SCHEDULES_KEY, map);
}

fn next_id(env: &Env) -> u64 {
    env.storage().instance().get(&NEXT_ID_KEY).unwrap_or(0u64)
}

fn set_next_id(env: &Env, id: u64) {
    env.storage().instance().set(&NEXT_ID_KEY, &id);
}

fn vested_at(schedule: &VestingSchedule, now: u64) -> i128 {
    if now < schedule.start {
        return 0;
    }
    let elapsed = now.saturating_sub(schedule.start);
    if elapsed < schedule.cliff_duration {
        return 0;
    }
    if elapsed >= schedule.duration {
        return schedule.total_amount;
    }
    schedule
        .total_amount
        .checked_mul(elapsed as i128)
        .and_then(|v| v.checked_div(schedule.duration as i128))
        .unwrap_or(schedule.total_amount)
}

fn schedule(env: &Env, id: u64) -> VestingSchedule {
    let map = schedules(env);
    map.get(id).unwrap_or_else(|| panic!("unknown schedule"))
}

#[contractimpl]
impl VestingEscrow {
    #[allow(clippy::too_many_arguments)]
    pub fn create_schedule(
        env: Env,
        admin: Address,
        beneficiary: Address,
        token: Address,
        total_amount: i128,
        start: u64,
        cliff_duration: u64,
        duration: u64,
        revocable: bool,
    ) -> u64 {
        admin.require_auth();
        if total_amount <= 0 {
            panic!("grant must be positive");
        }
        if duration == 0 {
            panic!("duration must be positive");
        }
        if cliff_duration > duration {
            panic!("cliff must not exceed duration");
        }

        let id = next_id(&env);
        set_next_id(&env, id + 1);

        let schedule = VestingSchedule {
            admin: admin.clone(),
            beneficiary: beneficiary.clone(),
            token: token.clone(),
            total_amount,
            claimed: 0,
            start,
            cliff_duration,
            duration,
            revocable,
            revoked_at: 0,
            returned: 0,
            status: ScheduleStatus::Active,
        };

        let mut map = schedules(&env);
        map.set(id, schedule);
        put_schedules(&env, &map);

        token::Client::new(&env, &token).transfer(
            &admin,
            env.current_contract_address(),
            &total_amount,
        );

        ScheduleCreated {
            id,
            admin,
            beneficiary,
            total_amount,
            start,
            cliff_duration,
            duration,
            revocable,
        }
        .publish(&env);

        id
    }

    pub fn schedule_count(env: Env) -> u64 {
        next_id(&env)
    }

    pub fn schedule(env: Env, id: u64) -> VestingSchedule {
        schedule(&env, id)
    }

    pub fn vested(env: Env, id: u64) -> i128 {
        let sched = schedule(&env, id);
        let now = env.ledger().timestamp();
        let as_of = if sched.status == ScheduleStatus::Revoked {
            sched.revoked_at
        } else {
            now
        };
        vested_at(&sched, as_of)
    }

    pub fn claimable(env: Env, id: u64) -> i128 {
        let sched = schedule(&env, id);
        if sched.status == ScheduleStatus::Completed {
            return 0;
        }
        let now = env.ledger().timestamp();
        let as_of = if sched.status == ScheduleStatus::Revoked {
            sched.revoked_at
        } else {
            now
        };
        let vested = vested_at(&sched, as_of);
        let claimable = vested - sched.claimed;
        if claimable < 0 {
            0
        } else {
            claimable
        }
    }

    pub fn unvested(env: Env, id: u64) -> i128 {
        let sched = schedule(&env, id);
        if sched.status == ScheduleStatus::Revoked {
            return 0;
        }
        let now = env.ledger().timestamp();
        let vested = vested_at(&sched, now);
        let remaining = sched.total_amount - vested;
        if remaining < 0 {
            0
        } else {
            remaining
        }
    }

    pub fn returned(env: Env, id: u64) -> i128 {
        let sched = schedule(&env, id);
        sched.returned
    }

    pub fn claimed(env: Env, id: u64) -> i128 {
        schedule(&env, id).claimed
    }

    pub fn claim(env: Env, id: u64) -> i128 {
        let mut sched = schedule(&env, id);
        sched.beneficiary.require_auth();

        if sched.status == ScheduleStatus::Revoked {
            // Still allow claiming already-vested tokens.
        }
        if sched.status == ScheduleStatus::Completed {
            panic!("nothing to claim");
        }

        let amount = Self::claimable(env.clone(), id);
        if amount <= 0 {
            panic!("nothing to claim");
        }

        let token_client = token::Client::new(&env, &sched.token);
        token_client.transfer(&env.current_contract_address(), &sched.beneficiary, &amount);

        sched.claimed += amount;
        if sched.status != ScheduleStatus::Revoked && sched.claimed >= sched.total_amount {
            sched.status = ScheduleStatus::Completed;
        }
        let total_claimed = sched.claimed;
        let mut map = schedules(&env);
        map.set(id, sched.clone());
        put_schedules(&env, &map);

        TokensClaimed {
            id,
            beneficiary: sched.beneficiary.clone(),
            amount,
            total_claimed,
        }
        .publish(&env);

        amount
    }

    pub fn revoke(env: Env, id: u64) -> i128 {
        let mut sched = schedule(&env, id);
        sched.admin.require_auth();

        if !sched.revocable {
            panic!("schedule is not revocable");
        }
        if sched.status != ScheduleStatus::Active {
            panic!("schedule is not active");
        }

        let now = env.ledger().timestamp();
        let vested = vested_at(&sched, now);
        let unvested = sched.total_amount - vested;
        if unvested <= 0 {
            panic!("nothing to revoke");
        }

        let token_client = token::Client::new(&env, &sched.token);
        token_client.transfer(&env.current_contract_address(), &sched.admin, &unvested);

        sched.revoked_at = now;
        sched.returned = unvested;
        sched.status = ScheduleStatus::Revoked;

        let mut map = schedules(&env);
        map.set(id, sched.clone());
        put_schedules(&env, &map);

        ScheduleRevoked {
            id,
            admin: sched.admin.clone(),
            returned: unvested,
            vested_at_revocation: vested,
        }
        .publish(&env);

        unvested
    }
}
