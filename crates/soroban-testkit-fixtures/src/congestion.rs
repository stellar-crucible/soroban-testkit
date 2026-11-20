//! Simulated network congestion, fee surges, and transaction queue delay harness.
//!
//! Under heavy network traffic on Stellar, base transaction fees fluctuate, surge
//! pricing triggers during demand spikes, and transactions can be delayed across
//! multiple ledger closes in the validator queue. [`CongestionSimulator`] emulates
//! these dynamics so contracts with deadline checks (such as DEX slippage bounds or
//! auction bidding windows) can be verified against realistic network arrival delays.

use soroban_sdk::testutils::Ledger as _;
use soroban_sdk::Env;

/// Default baseline transaction fee in Stroops on Stellar Mainnet.
pub const DEFAULT_BASE_FEE_STROOPS: u32 = 100;

/// Default seconds elapsed per ledger close on Stellar.
pub const DEFAULT_SECONDS_PER_LEDGER: u64 = 5;

/// Outcome report produced when injecting a congestion delay into a test environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelayReport {
    /// Number of ledgers elapsed during queue delay.
    pub ledgers_delayed: u32,
    /// Number of seconds elapsed during queue delay.
    pub seconds_delayed: u64,
    /// Ledger sequence number before the delay.
    pub old_sequence: u32,
    /// Ledger sequence number after the delay.
    pub new_sequence: u32,
    /// Ledger timestamp before the delay.
    pub old_timestamp: u64,
    /// Ledger timestamp after the delay.
    pub new_timestamp: u64,
    /// The effective base fee rate in Stroops at transaction execution.
    pub base_fee_at_arrival: u32,
}

/// Harness that emulates variable network base fees, fee surges, and ledger delays.
#[derive(Debug, Clone)]
pub struct CongestionSimulator {
    base_fee: u32,
    min_base_fee: u32,
    max_base_fee: u32,
    current_base_fee: u32,
    surge_multiplier: u32,
    min_delay_ledgers: u32,
    max_delay_ledgers: u32,
    seconds_per_ledger: u64,
    prng_state: u64,
}

impl Default for CongestionSimulator {
    fn default() -> Self {
        Self {
            base_fee: DEFAULT_BASE_FEE_STROOPS,
            min_base_fee: DEFAULT_BASE_FEE_STROOPS,
            max_base_fee: 10_000,
            current_base_fee: DEFAULT_BASE_FEE_STROOPS,
            surge_multiplier: 5,
            min_delay_ledgers: 1,
            max_delay_ledgers: 10,
            seconds_per_ledger: DEFAULT_SECONDS_PER_LEDGER,
            prng_state: 0x853c49e6748fea9b,
        }
    }
}

impl CongestionSimulator {
    /// Create a new congestion simulator with default network parameters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set an explicit seed for reproducible pseudo-random delay and fee sequences.
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.prng_state = if seed == 0 { 0x853c49e6748fea9b } else { seed };
        self
    }

    /// Set standard baseline transaction fee in Stroops.
    pub fn with_base_fee(mut self, base_fee: u32) -> Self {
        self.base_fee = base_fee;
        self.current_base_fee = base_fee;
        self
    }

    /// Set the allowed fluctuating range for base fees `[min, max]`.
    pub fn with_base_fee_range(mut self, min: u32, max: u32) -> Self {
        let (min, max) = if min <= max { (min, max) } else { (max, min) };
        self.min_base_fee = min;
        self.max_base_fee = max;
        self.current_base_fee = self.current_base_fee.clamp(min, max);
        self
    }

    /// Set the allowed range of ledger arrival delays `[min_ledgers, max_ledgers]`.
    pub fn with_delay_range(mut self, min_ledgers: u32, max_ledgers: u32) -> Self {
        let (min, max) = if min_ledgers <= max_ledgers {
            (min_ledgers, max_ledgers)
        } else {
            (max_ledgers, min_ledgers)
        };
        self.min_delay_ledgers = min.max(1);
        self.max_delay_ledgers = max.max(self.min_delay_ledgers);
        self
    }

    /// Set the surge pricing multiplier applied during congestion spikes.
    pub fn with_surge_multiplier(mut self, multiplier: u32) -> Self {
        self.surge_multiplier = multiplier.max(1);
        self
    }

    /// Set the number of seconds attributed per ledger advance.
    pub fn with_seconds_per_ledger(mut self, seconds: u64) -> Self {
        self.seconds_per_ledger = seconds.max(1);
        self
    }

    /// Return the current effective base fee rate in Stroops.
    pub fn current_base_fee(&self) -> u32 {
        self.current_base_fee
    }

    /// Calculate the total transaction fee in Stroops for `operations` operations.
    pub fn calculate_fee(&self, operations: u32) -> u64 {
        (self.current_base_fee as u64).saturating_mul(operations as u64)
    }

    /// Verify whether `offered_fee` meets or exceeds the required fee for `operations`.
    pub fn is_fee_sufficient(&self, offered_fee: u64, operations: u32) -> bool {
        offered_fee >= self.calculate_fee(operations)
    }

    /// Trigger a fee surge, multiplying the baseline fee by the surge multiplier.
    pub fn trigger_surge(&mut self) -> u32 {
        self.current_base_fee = self.base_fee.saturating_mul(self.surge_multiplier);
        self.current_base_fee
    }

    /// Reset current base fee back to baseline.
    pub fn reset_surge(&mut self) {
        self.current_base_fee = self.base_fee;
    }

    /// Simulate fee fluctuation, picking a new pseudo-random fee rate in `[min, max]`.
    pub fn fluctuate_base_fee(&mut self) -> u32 {
        let delta = self.max_base_fee.saturating_sub(self.min_base_fee);
        if delta == 0 {
            self.current_base_fee = self.min_base_fee;
        } else {
            let rand_val = (self.next_prng() % ((delta as u64) + 1)) as u32;
            self.current_base_fee = self.min_base_fee.saturating_add(rand_val);
        }
        self.current_base_fee
    }

    /// Sample the next pseudo-random ledger arrival delay within configured bounds.
    pub fn next_delay_ledgers(&mut self) -> u32 {
        let span = self
            .max_delay_ledgers
            .saturating_sub(self.min_delay_ledgers);
        if span == 0 {
            self.min_delay_ledgers
        } else {
            let offset = (self.next_prng() % ((span as u64) + 1)) as u32;
            self.min_delay_ledgers.saturating_add(offset)
        }
    }

    /// Advance `env`'s ledger sequence and timestamp by a pseudo-random arrival delay.
    pub fn advance_delay(&mut self, env: &Env) -> DelayReport {
        let delay_ledgers = self.next_delay_ledgers();
        self.fluctuate_base_fee();
        self.advance_fixed_delay(env, delay_ledgers)
    }

    /// Advance `env`'s ledger sequence and timestamp by an exact number of `ledgers`.
    pub fn advance_fixed_delay(&mut self, env: &Env, ledgers: u32) -> DelayReport {
        let ledger = env.ledger();
        let old_sequence = ledger.sequence();
        let old_timestamp = ledger.timestamp();

        let new_sequence = old_sequence.saturating_add(ledgers);
        let seconds_delayed = (ledgers as u64).saturating_mul(self.seconds_per_ledger);
        let new_timestamp = old_timestamp.saturating_add(seconds_delayed);

        ledger.set_sequence_number(new_sequence);
        ledger.set_timestamp(new_timestamp);

        DelayReport {
            ledgers_delayed: ledgers,
            seconds_delayed,
            old_sequence,
            new_sequence,
            old_timestamp,
            new_timestamp,
            base_fee_at_arrival: self.current_base_fee,
        }
    }

    /// Execute a contract invocation closure after applying a randomized arrival delay.
    pub fn execute_with_delay<T, F: FnOnce() -> T>(
        &mut self,
        env: &Env,
        call: F,
    ) -> (DelayReport, T) {
        let report = self.advance_delay(env);
        let result = call();
        (report, result)
    }

    /// Deterministic 64-bit XorShift pseudo-random generator.
    fn next_prng(&mut self) -> u64 {
        let mut x = self.prng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.prng_state = x;
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contracterror, contractimpl, symbol_short, Env};

    #[contracterror]
    #[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
    #[repr(u32)]
    pub enum ProtocolError {
        DeadlineExpired = 1,
        AuctionClosed = 2,
        InsufficientFee = 3,
    }

    // --- Mock DEX Swap Contract with Deadline Parameter ---
    #[contract]
    pub struct MockSwapContract;

    #[contractimpl]
    impl MockSwapContract {
        pub fn swap(
            env: Env,
            _amount_in: u64,
            _min_out: u64,
            deadline: u64,
        ) -> Result<u64, ProtocolError> {
            let current_time = env.ledger().timestamp();
            if current_time > deadline {
                return Err(ProtocolError::DeadlineExpired);
            }
            Ok(95) // Successful swap output
        }
    }

    // --- Mock Auction Contract with Bidding Window ---
    #[contract]
    pub struct MockAuctionContract;

    #[contractimpl]
    impl MockAuctionContract {
        pub fn init_auction(env: Env, closing_ledger: u32) {
            env.storage()
                .instance()
                .set(&symbol_short!("close_at"), &closing_ledger);
            env.storage()
                .instance()
                .set(&symbol_short!("hi_bid"), &0u64);
        }

        pub fn place_bid(env: Env, bid_amount: u64) -> Result<(), ProtocolError> {
            let closing: u32 = env
                .storage()
                .instance()
                .get(&symbol_short!("close_at"))
                .unwrap();
            let current_ledger = env.ledger().sequence();
            if current_ledger >= closing {
                return Err(ProtocolError::AuctionClosed);
            }
            env.storage()
                .instance()
                .set(&symbol_short!("hi_bid"), &bid_amount);
            Ok(())
        }
    }

    #[test]
    fn test_fee_fluctuations_and_surges() {
        let mut sim = CongestionSimulator::new()
            .with_base_fee(100)
            .with_base_fee_range(100, 500)
            .with_surge_multiplier(10);

        assert_eq!(sim.current_base_fee(), 100);
        assert_eq!(sim.calculate_fee(2), 200);
        assert!(sim.is_fee_sufficient(250, 2));
        assert!(!sim.is_fee_sufficient(150, 2));

        // Surge multiplier test
        sim.trigger_surge();
        assert_eq!(sim.current_base_fee(), 1000);
        assert_eq!(sim.calculate_fee(1), 1000);
        assert!(!sim.is_fee_sufficient(500, 1));

        sim.reset_surge();
        assert_eq!(sim.current_base_fee(), 100);

        // Fluctuate fee within range
        let fee = sim.fluctuate_base_fee();
        assert!((100..=500).contains(&fee));
    }

    #[test]
    fn test_arrival_delay_advances_sequence_and_clock() {
        let env = Env::default();
        env.ledger().set_sequence_number(100);
        env.ledger().set_timestamp(1_000);

        let mut sim = CongestionSimulator::new()
            .with_delay_range(2, 6)
            .with_seconds_per_ledger(5);

        let report = sim.advance_delay(&env);

        assert!(report.ledgers_delayed >= 2 && report.ledgers_delayed <= 6);
        assert_eq!(report.seconds_delayed, (report.ledgers_delayed as u64) * 5);
        assert_eq!(report.old_sequence, 100);
        assert_eq!(report.new_sequence, 100 + report.ledgers_delayed);
        assert_eq!(report.old_timestamp, 1_000);
        assert_eq!(report.new_timestamp, 1_000 + report.seconds_delayed);
    }

    #[test]
    fn test_swap_deadline_expiration_under_congestion() {
        let env = Env::default();
        env.ledger().set_timestamp(1_000);
        let contract_id = env.register(MockSwapContract, ());
        let client = MockSwapContractClient::new(&env, &contract_id);

        let deadline = 1_020; // 20 seconds from now

        // 1. Transaction executes in timely manner (before deadline)
        let mut sim = CongestionSimulator::new().with_seconds_per_ledger(5);
        sim.advance_fixed_delay(&env, 2); // 10s delay -> timestamp 1010
        assert_eq!(env.ledger().timestamp(), 1010);

        let res = client.swap(&100, &90, &deadline);
        assert_eq!(res, 95);

        // 2. Heavy queue congestion delays transaction past the deadline
        sim.advance_fixed_delay(&env, 3); // 15s delay -> timestamp 1025
        assert_eq!(env.ledger().timestamp(), 1025);

        let err = client.try_swap(&100, &90, &deadline);
        assert!(err.is_err());
        assert_eq!(err.unwrap_err().unwrap(), ProtocolError::DeadlineExpired);
    }

    #[test]
    fn test_auction_closing_under_queue_delay() {
        let env = Env::default();
        env.ledger().set_sequence_number(50);
        let contract_id = env.register(MockAuctionContract, ());
        let client = MockAuctionContractClient::new(&env, &contract_id);

        let closing_ledger = 60;
        client.init_auction(&closing_ledger);

        // Bidding succeeds while before the closing ledger
        let mut sim = CongestionSimulator::new().with_delay_range(1, 3);
        let (rep, res) = sim.execute_with_delay(&env, || client.try_place_bid(&500));
        assert!(res.is_ok());
        assert!(rep.new_sequence < closing_ledger);

        // Substantial queue delay pushes execution past closing ledger
        sim.advance_fixed_delay(&env, 10);
        assert!(env.ledger().sequence() >= closing_ledger);

        let bid_result = client.try_place_bid(&600);
        assert!(bid_result.is_err());
        assert_eq!(
            bid_result.unwrap_err().unwrap(),
            ProtocolError::AuctionClosed
        );
    }
}
