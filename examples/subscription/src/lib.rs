#![no_std]

//! A recurring subscription billing contract.
//!
//! A subscriber records a merchant, a token, a recurring fee and a billing
//! interval, then approves this contract as a spender on the token. Once per
//! interval the merchant may call [`SubscriptionContract::bill`], which draws
//! exactly the stored fee from the subscriber's allowance and pushes the next
//! billing time forward by one interval.
//!
//! The merchant never supplies an amount: the fee and interval are fixed when
//! the subscriber creates the subscription, and the subscriber can cancel at
//! any time to stop future deductions.

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, token, Address, Env,
};

/// Keys this contract writes. The subscription is persistent because it must
/// outlive the ledger close that created it; billing rewrites the same entry
/// rather than adding one per cycle.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Subscription(Address),
}

/// One subscriber's standing agreement with one merchant.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Subscription {
    pub subscriber: Address,
    pub merchant: Address,
    pub token: Address,
    /// The exact amount drawn each billing cycle.
    pub fee: i128,
    /// Seconds between billing cycles.
    pub interval: u64,
    /// The ledger timestamp at which the next cycle becomes payable.
    pub next_billing: u64,
    pub cycles_billed: u32,
    pub canceled: bool,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum SubscriptionError {
    /// The subscriber already has a live subscription.
    AlreadyActive = 1,
    /// The recurring fee must be greater than zero.
    InvalidFee = 2,
    /// The billing interval must be greater than zero.
    InvalidInterval = 3,
    /// The first billing time is in the past.
    InvalidSchedule = 4,
    SubscriptionNotFound = 5,
    Unauthorized = 6,
    AlreadyCanceled = 7,
    /// The billing interval has not elapsed yet.
    BillingNotDue = 8,
    /// Pushing the next billing time forward would overflow `u64`.
    TimestampOverflow = 9,
    /// The token refused the allowance transfer.
    PaymentFailed = 10,
    /// The cycle counter would overflow `u32`.
    CycleOverflow = 11,
}

#[contractevent(topics = ["subscription", "created"])]
#[derive(Clone)]
pub struct SubscriptionCreated {
    pub subscriber: Address,
    pub merchant: Address,
    pub token: Address,
    pub fee: i128,
    pub interval: u64,
    pub next_billing: u64,
}

#[contractevent(topics = ["subscription", "billed"])]
#[derive(Clone)]
pub struct Billed {
    pub subscriber: Address,
    pub merchant: Address,
    pub amount: i128,
    pub next_billing: u64,
    pub cycles_billed: u32,
}

#[contractevent(topics = ["subscription", "canceled"])]
#[derive(Clone)]
pub struct SubscriptionCanceled {
    pub subscriber: Address,
    pub merchant: Address,
}

#[contract]
pub struct SubscriptionContract;

#[contractimpl]
impl SubscriptionContract {
    /// Creates a subscription owned by `subscriber`.
    ///
    /// `next_billing` is the first timestamp at which `bill` may charge. The
    /// subscriber's authorization is required, and the token allowance this
    /// contract relies on is approved separately by the subscriber, so the
    /// merchant can never set or widen it.
    pub fn subscribe(
        env: Env,
        subscriber: Address,
        merchant: Address,
        token: Address,
        fee: i128,
        interval: u64,
        next_billing: u64,
    ) -> Result<(), SubscriptionError> {
        subscriber.require_auth();

        if fee <= 0 {
            return Err(SubscriptionError::InvalidFee);
        }
        if interval == 0 {
            return Err(SubscriptionError::InvalidInterval);
        }
        if next_billing < env.ledger().timestamp() {
            return Err(SubscriptionError::InvalidSchedule);
        }

        let key = DataKey::Subscription(subscriber.clone());
        if let Some(existing) = load_subscription(&env, &key) {
            if !existing.canceled {
                return Err(SubscriptionError::AlreadyActive);
            }
        }

        let subscription = Subscription {
            subscriber: subscriber.clone(),
            merchant,
            token,
            fee,
            interval,
            next_billing,
            cycles_billed: 0,
            canceled: false,
        };
        env.storage().persistent().set(&key, &subscription);

        SubscriptionCreated {
            subscriber,
            merchant: subscription.merchant.clone(),
            token: subscription.token.clone(),
            fee,
            interval,
            next_billing,
        }
        .publish(&env);

        Ok(())
    }

    /// Charges one billing cycle and returns the new cycle count.
    ///
    /// Only the recorded merchant may call this. The fee is read from storage,
    /// so a caller cannot choose how much to withdraw, and the call is accepted
    /// only once the interval that started at `next_billing` has elapsed. Each
    /// successful call advances `next_billing` by exactly one interval, which
    /// lets a merchant catch up one cycle at a time after a gap but never
    /// charge twice for the same cycle.
    pub fn bill(env: Env, caller: Address, subscriber: Address) -> Result<u32, SubscriptionError> {
        caller.require_auth();

        let key = DataKey::Subscription(subscriber.clone());
        let mut subscription =
            load_subscription(&env, &key).ok_or(SubscriptionError::SubscriptionNotFound)?;

        if caller != subscription.merchant {
            return Err(SubscriptionError::Unauthorized);
        }
        if subscription.canceled {
            return Err(SubscriptionError::AlreadyCanceled);
        }
        if env.ledger().timestamp() < subscription.next_billing {
            return Err(SubscriptionError::BillingNotDue);
        }

        let next_billing = subscription
            .next_billing
            .checked_add(subscription.interval)
            .ok_or(SubscriptionError::TimestampOverflow)?;
        let cycles_billed = subscription
            .cycles_billed
            .checked_add(1)
            .ok_or(SubscriptionError::CycleOverflow)?;

        // Charge the cycle before the external call so a re-entrant token
        // cannot see the old schedule and bill the same cycle twice. The old
        // record is restored if the token refuses the transfer, because Soroban
        // keeps storage written by a call that returns an error value.
        let previous = subscription.clone();
        subscription.next_billing = next_billing;
        subscription.cycles_billed = cycles_billed;
        env.storage().persistent().set(&key, &subscription);

        let payment = token::Client::new(&env, &subscription.token).try_transfer_from(
            &env.current_contract_address(),
            &subscriber,
            &subscription.merchant,
            &subscription.fee,
        );
        if payment.is_err() {
            env.storage().persistent().set(&key, &previous);
            return Err(SubscriptionError::PaymentFailed);
        }

        Billed {
            subscriber,
            merchant: subscription.merchant.clone(),
            amount: subscription.fee,
            next_billing,
            cycles_billed,
        }
        .publish(&env);

        Ok(cycles_billed)
    }

    /// Cancels a subscription. Only the subscriber may cancel, and a canceled
    /// subscription can never be billed again.
    pub fn cancel(env: Env, caller: Address, subscriber: Address) -> Result<(), SubscriptionError> {
        caller.require_auth();

        let key = DataKey::Subscription(subscriber.clone());
        let mut subscription =
            load_subscription(&env, &key).ok_or(SubscriptionError::SubscriptionNotFound)?;

        if caller != subscription.subscriber {
            return Err(SubscriptionError::Unauthorized);
        }
        if subscription.canceled {
            return Err(SubscriptionError::AlreadyCanceled);
        }

        subscription.canceled = true;
        env.storage().persistent().set(&key, &subscription);

        SubscriptionCanceled {
            subscriber,
            merchant: subscription.merchant,
        }
        .publish(&env);

        Ok(())
    }

    /// The stored agreement for `subscriber`, if one exists.
    pub fn subscription(env: Env, subscriber: Address) -> Option<Subscription> {
        load_subscription(&env, &DataKey::Subscription(subscriber))
    }

    /// Whether a live (not canceled) subscription exists.
    pub fn is_active(env: Env, subscriber: Address) -> bool {
        load_subscription(&env, &DataKey::Subscription(subscriber))
            .map(|subscription| !subscription.canceled)
            .unwrap_or(false)
    }

    /// Whether the next cycle is payable right now.
    pub fn is_billing_due(env: Env, subscriber: Address) -> bool {
        match load_subscription(&env, &DataKey::Subscription(subscriber)) {
            Some(subscription) => {
                !subscription.canceled && env.ledger().timestamp() >= subscription.next_billing
            }
            None => false,
        }
    }
}

fn load_subscription(env: &Env, key: &DataKey) -> Option<Subscription> {
    env.storage().persistent().get(key)
}
