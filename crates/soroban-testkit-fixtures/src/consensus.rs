//! Simulated multi-validator consensus rounds and Stellar ledger close timing.
//!
//! On Stellar, the SCP (Stellar Consensus Protocol) closes a ledger every few
//! seconds, not on a fixed cadence: close intervals drift between roughly three
//! and eight seconds, and a validator only accepts a close whose interval sits
//! close to the one it nominated. Protocols that average over time — TWAP
//! oracles, streaming rates, funding accrual, timelocks — behave differently
//! under that jitter than they do against a metronome, and a test that closes
//! every ledger by exactly five seconds cannot tell the two apart.
//!
//! [`ConsensusSimulator`] closes ledgers with intervals drawn from a Gaussian
//! distribution around the configured midpoint, clamped to the configured
//! range, and records each validator's vote for the round. The samples are
//! pseudo-random but reproducible from a seed, so a jitter-sensitive test can
//! be replayed exactly.
//!
//! ```rust
//! use soroban_sdk::Env;
//! use soroban_testkit_fixtures::ConsensusSimulator;
//!
//! let env = Env::default();
//! env.ledger().set_timestamp(1_700_000_000);
//! let mut sim = ConsensusSimulator::new()
//!     .with_validators(5)
//!     .with_quorum(3)
//!     .with_close_interval(3_000, 8_000)
//!     .with_jitter_std_ms(1_200.0);
//!
//! let round = sim.run_round(&env);
//! assert!(round.close.sequence > 0);
//! assert_eq!(env.ledger().timestamp(), round.close.timestamp);
//! assert_eq!(round.votes_for + round.votes_against, round.validators);
//! ```

use soroban_sdk::testutils::Ledger as _;
use soroban_sdk::Env;

/// Validators the default simulator runs.
pub const DEFAULT_VALIDATORS: u32 = 5;

/// Votes the default simulator needs before it calls a round agreed.
pub const DEFAULT_QUORUM: u32 = 3;

/// Shortest simplified ledger close interval, in milliseconds.
pub const DEFAULT_MIN_CLOSE_INTERVAL_MS: u64 = 3_000;

/// Longest simplified ledger close interval, in milliseconds.
pub const DEFAULT_MAX_CLOSE_INTERVAL_MS: u64 = 8_000;

/// Standard deviation of the Gaussian jitter around the midpoint interval.
pub const DEFAULT_JITTER_STD_MS: f64 = 1_200.0;

/// Two standard deviations: the share of a Gaussian that sits inside the band
/// a validator treats as agreeing with the proposed close interval.
pub const QUORUM_ACCEPTANCE_SIGMA: f64 = 1.96;

/// Seed the default simulator starts from.
pub const DEFAULT_SEED: u64 = 0x2545_f491_4f6c_dd1d;

/// One ledger close: the height and time it produced, and the interval it took.
///
/// `sequence` and `timestamp` are the values the env reports after the close,
/// so a test can assert against the ledger header as well as against this
/// report without repeating the arithmetic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerClose {
    /// Ledger sequence before the close.
    pub prev_sequence: u32,
    /// Ledger sequence after the close.
    pub sequence: u32,
    /// Ledger timestamp before the close, in seconds.
    pub prev_timestamp: u64,
    /// Ledger timestamp after the close, in seconds.
    pub timestamp: u64,
    /// The close interval sampled for this ledger, in milliseconds.
    pub close_interval_ms: u64,
    /// The same interval rounded to the whole seconds a ledger header stores.
    pub seconds_elapsed: u64,
    /// How far this interval sat from the configured midpoint, in
    /// milliseconds. Negative when the close was faster than the midpoint.
    pub jitter_ms: i64,
}

/// One consensus round: the ledger close it agreed on and the validators that
/// backed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsensusRound {
    /// The close the round produced.
    pub close: LedgerClose,
    /// Validators that participated in the round.
    pub validators: u32,
    /// Validators whose nominated interval agreed with the close.
    pub votes_for: u32,
    /// Validators whose nominated interval disagreed with the close.
    pub votes_against: u32,
    /// Votes needed for the round to be considered agreed.
    pub quorum: u32,
    /// Whether `votes_for` reached `quorum`.
    pub quorum_reached: bool,
}

/// Emulates SCP ledger closes with Gaussian jitter and validator quorum votes.
///
/// The simulator holds only network parameters and PRNG state; the ledger it
/// moves is the one passed to [`ConsensusSimulator::close_ledger`], so the same
/// simulator can drive several environments in one test.
///
/// Sampling is deterministic for a seed: `with_seed` twice with the same value
/// produces the same sequence of close intervals, and every close consumes a
/// fixed number of PRNG draws.
#[derive(Debug, Clone)]
pub struct ConsensusSimulator {
    validators: u32,
    quorum: u32,
    min_close_ms: u64,
    max_close_ms: u64,
    jitter_std_ms: f64,
    prng_state: u64,
}

impl Default for ConsensusSimulator {
    fn default() -> Self {
        Self {
            validators: DEFAULT_VALIDATORS,
            quorum: DEFAULT_QUORUM,
            min_close_ms: DEFAULT_MIN_CLOSE_INTERVAL_MS,
            max_close_ms: DEFAULT_MAX_CLOSE_INTERVAL_MS,
            jitter_std_ms: DEFAULT_JITTER_STD_MS,
            prng_state: DEFAULT_SEED,
        }
    }
}

impl ConsensusSimulator {
    /// A simulator with five validators, a three-vote quorum, and close
    /// intervals of 3–8 seconds with 1.2 seconds of Gaussian jitter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set an explicit seed for reproducible interval and vote sequences.
    ///
    /// A zero seed falls back to [`DEFAULT_SEED`], so the generator never
    /// starts from a state that would emit zeros.
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.prng_state = if seed == 0 { DEFAULT_SEED } else { seed };
        self
    }

    /// Set how many validators vote in each round.
    ///
    /// At least one validator is required. Reducing the count also lowers a
    /// quorum that no longer fits.
    pub fn with_validators(mut self, validators: u32) -> Self {
        self.validators = validators.max(1);
        self.quorum = self.quorum.min(self.validators);
        self
    }

    /// Set how many agreeing votes a round needs.
    ///
    /// Clamped to `1..=validators`, so a quorum is always reachable in
    /// principle even when the votes are not.
    pub fn with_quorum(mut self, quorum: u32) -> Self {
        self.quorum = quorum.clamp(1, self.validators);
        self
    }

    /// Set the shortest and longest close interval, in milliseconds.
    ///
    /// The arguments are ordered for you; both are at least one millisecond.
    pub fn with_close_interval(mut self, min_ms: u64, max_ms: u64) -> Self {
        let (min, max) = if min_ms <= max_ms {
            (min_ms, max_ms)
        } else {
            (max_ms, min_ms)
        };
        self.min_close_ms = min.max(1);
        self.max_close_ms = max.max(self.min_close_ms);
        self
    }

    /// Set the standard deviation of the Gaussian jitter, in milliseconds.
    ///
    /// A non-finite or negative deviation is treated as zero, which makes every
    /// close take the midpoint interval.
    pub fn with_jitter_std_ms(mut self, std_ms: f64) -> Self {
        self.jitter_std_ms = if std_ms.is_finite() && std_ms > 0.0 {
            std_ms
        } else {
            0.0
        };
        self
    }

    /// Validators that vote in each round.
    pub fn validators(&self) -> u32 {
        self.validators
    }

    /// Votes a round needs to be considered agreed.
    pub fn quorum(&self) -> u32 {
        self.quorum
    }

    /// The configured close interval range, in milliseconds.
    pub fn close_interval_range(&self) -> (u64, u64) {
        (self.min_close_ms, self.max_close_ms)
    }

    /// The configured jitter standard deviation, in milliseconds.
    pub fn jitter_std_ms(&self) -> f64 {
        self.jitter_std_ms
    }

    /// The midpoint of the configured close interval range, in milliseconds.
    pub fn mean_close_interval_ms(&self) -> u64 {
        self.min_close_ms + (self.max_close_ms - self.min_close_ms) / 2
    }

    /// How far a validator's nominated interval may sit from the close and
    /// still vote for it, in milliseconds.
    pub fn acceptance_band_ms(&self) -> u64 {
        (self.jitter_std_ms * QUORUM_ACCEPTANCE_SIGMA).round() as u64
    }

    /// Sample the next close interval, in milliseconds.
    ///
    /// The value is Gaussian around the range midpoint and clamped to the
    /// configured range, so a close never falls outside the range the test
    /// declared even when the tail of the distribution would.
    pub fn next_close_interval_ms(&mut self) -> u64 {
        let midpoint =
            self.min_close_ms as f64 + (self.max_close_ms - self.min_close_ms) as f64 / 2.0;
        let sample = midpoint + self.next_gaussian() * self.jitter_std_ms;
        let low = self.min_close_ms as f64;
        let high = self.max_close_ms as f64;
        sample.clamp(low, high).round() as u64
    }

    /// Close one ledger, advancing the env's sequence by one and its timestamp
    /// by the sampled interval.
    ///
    /// The ledger header stores whole seconds, so the millisecond interval is
    /// rounded up to the next second before it is added. The close is reported
    /// in both units: `close_interval_ms` is the sample, `seconds_elapsed` is
    /// what the header moved by.
    ///
    /// Both counters saturate: closing at `u32::MAX` or `u64::MAX` stays there
    /// instead of wrapping into the past.
    pub fn close_ledger(&mut self, env: &Env) -> LedgerClose {
        let close_interval_ms = self.next_close_interval_ms();
        let seconds_elapsed = close_interval_ms.div_ceil(1_000).max(1);

        let ledger = env.ledger();
        let prev_sequence = ledger.sequence();
        let prev_timestamp = ledger.timestamp();
        let sequence = prev_sequence.saturating_add(1);
        let timestamp = prev_timestamp.saturating_add(seconds_elapsed);
        ledger.set_sequence_number(sequence);
        ledger.set_timestamp(timestamp);

        LedgerClose {
            prev_sequence,
            sequence,
            prev_timestamp,
            timestamp,
            close_interval_ms,
            seconds_elapsed,
            jitter_ms: close_interval_ms as i64 - self.mean_close_interval_ms() as i64,
        }
    }

    /// Close `count` ledgers, returning each close in order.
    pub fn advance_ledgers(&mut self, env: &Env, count: u32) -> Vec<LedgerClose> {
        (0..count).map(|_| self.close_ledger(env)).collect()
    }

    /// Run one consensus round: close a ledger, then collect every validator's
    /// vote on the interval the close used.
    ///
    /// Each validator nominates its own interval. A validator votes for the
    /// close when its sample is within [`ConsensusSimulator::acceptance_band_ms`]
    /// of the closed interval and against it otherwise, which is how a round
    /// can close a ledger yet miss quorum: the ledger is produced by whichever
    /// interval the proposer drew, and the others compare their own draws to
    /// it.
    ///
    /// Every validator votes exactly once, so `votes_for + votes_against`
    /// always equals `validators`.
    pub fn run_round(&mut self, env: &Env) -> ConsensusRound {
        let close = self.close_ledger(env);
        let band = self.acceptance_band_ms();

        let mut votes_for = 0;
        for _ in 0..self.validators {
            let nominated = self.next_close_interval_ms();
            if nominated.abs_diff(close.close_interval_ms) <= band {
                votes_for += 1;
            }
        }

        ConsensusRound {
            close,
            validators: self.validators,
            votes_for,
            votes_against: self.validators - votes_for,
            quorum: self.quorum,
            quorum_reached: votes_for >= self.quorum,
        }
    }

    /// Advance the chain by one consensus round and then execute `call`, so the
    /// invocation observes the ledger the new close produced.
    ///
    /// This is the shape of a transaction that lands in the block a round
    /// closes: the contract sees the new height and timestamp, and the test
    /// still learns what the round decided.
    pub fn execute_with_consensus<T, F: FnOnce() -> T>(
        &mut self,
        env: &Env,
        call: F,
    ) -> (ConsensusRound, T) {
        let round = self.run_round(env);
        let result = call();
        (round, result)
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

    /// A uniform sample in `[0, 1)`, built from the top 53 bits of the state.
    fn next_unit(&mut self) -> f64 {
        ((self.next_prng() >> 11) as f64) / ((1u64 << 53) as f64)
    }

    /// One standard normal sample, by the Box–Muller transform.
    ///
    /// `u1` is floored at the smallest positive float so the logarithm cannot
    /// be taken of zero.
    fn next_gaussian(&mut self) -> f64 {
        let u1 = self.next_unit().max(f64::MIN_POSITIVE);
        let u2 = self.next_unit();
        (-2.0 * u1.ln()).sqrt() * (core::f64::consts::TAU * u2).cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl, symbol_short, Env};

    /// A time-weighted average price feed.
    ///
    /// `record` banks `price × (now − last record)` and `twap` divides by the
    /// time since the first record, which is exactly the arithmetic a jittered
    /// close interval perturbs.
    #[contract]
    pub struct TwapFeed;

    #[contractimpl]
    impl TwapFeed {
        pub fn record(env: Env, price: i128) -> i128 {
            let now = env.ledger().timestamp();
            let storage = env.storage().instance();
            if !storage.has(&symbol_short!("start")) {
                storage.set(&symbol_short!("start"), &now);
                storage.set(&symbol_short!("last"), &now);
                storage.set(&symbol_short!("acc"), &0i128);
            }

            let last: u64 = storage.get(&symbol_short!("last")).unwrap_or(now);
            let acc: i128 = storage.get(&symbol_short!("acc")).unwrap_or(0);
            let acc = acc.saturating_add(price.saturating_mul(now.saturating_sub(last) as i128));
            storage.set(&symbol_short!("last"), &now);
            storage.set(&symbol_short!("acc"), &acc);
            acc
        }

        pub fn twap(env: Env) -> i128 {
            let storage = env.storage().instance();
            let now = env.ledger().timestamp();
            let start: u64 = storage.get(&symbol_short!("start")).unwrap_or(now);
            let acc: i128 = storage.get(&symbol_short!("acc")).unwrap_or(0);
            let elapsed = now.saturating_sub(start);
            if elapsed == 0 {
                0
            } else {
                acc / elapsed as i128
            }
        }
    }

    fn feed(env: &Env) -> TwapFeedClient<'_> {
        let id = env.register(TwapFeed, ());
        TwapFeedClient::new(env, &id)
    }

    /// A ledger with a realistic starting clock.
    fn env() -> Env {
        let env = Env::default();
        env.ledger().set_sequence_number(1_000);
        env.ledger().set_timestamp(1_700_000_000);
        env
    }

    #[test]
    fn default_simulator_matches_the_documented_network_parameters() {
        let sim = ConsensusSimulator::new();

        assert_eq!(sim.validators(), DEFAULT_VALIDATORS);
        assert_eq!(sim.quorum(), DEFAULT_QUORUM);
        assert_eq!(
            sim.close_interval_range(),
            (
                DEFAULT_MIN_CLOSE_INTERVAL_MS,
                DEFAULT_MAX_CLOSE_INTERVAL_MS
            )
        );
        assert_eq!(sim.jitter_std_ms(), DEFAULT_JITTER_STD_MS);
        assert_eq!(sim.mean_close_interval_ms(), 5_500);
        assert_eq!(sim.acceptance_band_ms(), 2_352);
    }

    #[test]
    fn closing_a_ledger_advances_height_by_one_and_the_clock_by_the_interval() {
        let env = env();
        let mut sim = ConsensusSimulator::new();

        let close = sim.close_ledger(&env);

        assert_eq!(close.prev_sequence, 1_000);
        assert_eq!(close.sequence, 1_001);
        assert_eq!(close.prev_timestamp, 1_700_000_000);
        assert_eq!(
            close.timestamp,
            close.prev_timestamp + close.seconds_elapsed
        );
        assert_eq!(
            close.seconds_elapsed,
            close.close_interval_ms.div_ceil(1_000)
        );
        assert!((3..=8).contains(&close.seconds_elapsed));
        // The ledger header agrees with the report, not just the report with
        // itself.
        assert_eq!(env.ledger().sequence(), close.sequence);
        assert_eq!(env.ledger().timestamp(), close.timestamp);
    }

    #[test]
    fn sampled_intervals_stay_inside_the_configured_range() {
        let mut sim = ConsensusSimulator::new()
            .with_close_interval(2_000, 4_000)
            .with_jitter_std_ms(5_000.0);

        for _ in 0..500 {
            let interval = sim.next_close_interval_ms();
            assert!(
                (2_000..=4_000).contains(&interval),
                "interval {interval} escaped the configured range"
            );
        }
    }

    #[test]
    fn gaussian_jitter_reaches_both_ends_of_the_range_and_averages_near_the_midpoint() {
        let mut sim = ConsensusSimulator::new().with_jitter_std_ms(4_000.0);
        let mut samples = Vec::new();
        for _ in 0..2_000 {
            samples.push(sim.next_close_interval_ms());
        }

        let at_floor = samples.iter().filter(|&&s| s == 3_000).count();
        let at_ceiling = samples.iter().filter(|&&s| s == 8_000).count();
        assert!(at_floor > 0, "the lower tail was never clamped");
        assert!(at_ceiling > 0, "the upper tail was never clamped");

        let mean = samples.iter().map(|&s| s as i64).sum::<i64>() / samples.len() as i64;
        assert!(
            (mean - 5_500).abs() < 100,
            "clamped Gaussian mean drifted to {mean}"
        );
    }

    #[test]
    fn jitter_spreads_samples_around_the_mean_at_the_configured_scale() {
        let mut sim = ConsensusSimulator::new()
            .with_close_interval(3_000, 8_000)
            .with_jitter_std_ms(700.0);
        let samples: Vec<f64> = (0..2_000)
            .map(|_| sim.next_close_interval_ms() as f64)
            .collect();
        let mean = samples.iter().sum::<f64>() / samples.len() as f64;
        let variance = samples
            .iter()
            .map(|sample| (sample - mean).powi(2))
            .sum::<f64>()
            / samples.len() as f64;
        let std = variance.sqrt();

        assert!(
            (std - 700.0).abs() < 100.0,
            "sample deviation {std} did not track the configured 700ms"
        );
    }

    #[test]
    fn the_same_seed_replays_the_same_closes_and_different_seeds_diverge() {
        let env = env();
        let mut first = ConsensusSimulator::new().with_seed(7);
        let mut replay = ConsensusSimulator::new().with_seed(7);
        let mut other = ConsensusSimulator::new().with_seed(8);

        let first: Vec<u64> = (0..20).map(|_| first.next_close_interval_ms()).collect();
        let replay: Vec<u64> = (0..20).map(|_| replay.next_close_interval_ms()).collect();
        let other: Vec<u64> = (0..20).map(|_| other.next_close_interval_ms()).collect();

        assert_eq!(first, replay);
        assert_ne!(first, other);

        // A round consumes a fixed number of draws, so seeded rounds replay
        // their votes too.
        let mut a = ConsensusSimulator::new().with_seed(11);
        let mut b = ConsensusSimulator::new().with_seed(11);
        assert_eq!(a.run_round(&env), b.run_round(&env));
    }

    #[test]
    fn a_round_counts_every_validator_exactly_once() {
        let env = env();
        let mut sim = ConsensusSimulator::new().with_validators(7).with_quorum(5);

        for _ in 0..50 {
            let round = sim.run_round(&env);
            assert_eq!(round.validators, 7);
            assert_eq!(round.votes_for + round.votes_against, 7);
            assert_eq!(round.quorum, 5);
            assert_eq!(round.quorum_reached, round.votes_for >= 5);
            assert!(round.votes_for <= 7);
        }
    }

    #[test]
    fn a_round_misses_quorum_when_the_validators_nominate_different_intervals() {
        // A 100ms deviation with a 3-8 second range spreads the nominations
        // across the range, so a unanimous quorum is rarely reached.
        let env = env();
        let mut sim = ConsensusSimulator::new()
            .with_validators(5)
            .with_quorum(5)
            .with_jitter_std_ms(100.0);

        let missed = (0..200).any(|_| {
            let round = sim.run_round(&env);
            assert!(!round.quorum_reached || round.votes_for == 5);
            !round.quorum_reached
        });
        assert!(missed, "a strict unanimous quorum never failed");
    }

    #[test]
    fn consensus_rounds_stay_agreed_when_the_band_covers_the_jitter() {
        let env = env();
        // A 100ms range with a 500ms deviation means every nomination sits
        // inside the acceptance band by construction, so agreement cannot be
        // left to a lucky sample.
        let mut sim = ConsensusSimulator::new()
            .with_close_interval(3_000, 3_100)
            .with_jitter_std_ms(500.0)
            .with_quorum(5);

        for _ in 0..20 {
            let round = sim.run_round(&env);
            assert!(round.quorum_reached);
            assert_eq!(round.votes_for, round.validators);
        }
    }

    #[test]
    fn advancing_ledgers_moves_height_and_time_monotonically() {
        let env = env();
        let mut sim = ConsensusSimulator::new();

        let closes = sim.advance_ledgers(&env, 100);

        assert_eq!(closes.len(), 100);
        for (index, close) in closes.iter().enumerate() {
            assert_eq!(close.sequence, 1_001 + index as u32);
            assert!(close.timestamp > close.prev_timestamp);
            assert!((3..=8).contains(&close.seconds_elapsed));
            if index > 0 {
                assert_eq!(close.prev_sequence, closes[index - 1].sequence);
                assert_eq!(close.prev_timestamp, closes[index - 1].timestamp);
            }
        }
        assert_eq!(env.ledger().sequence(), 1_100);
        assert_eq!(env.ledger().timestamp(), closes.last().unwrap().timestamp);
    }

    #[test]
    fn a_zero_jitter_simulator_closes_on_the_midpoint_every_time() {
        let env = env();
        let mut sim = ConsensusSimulator::new().with_jitter_std_ms(0.0);

        let closes = sim.advance_ledgers(&env, 10);

        for close in closes {
            assert_eq!(close.close_interval_ms, 5_500);
            assert_eq!(close.seconds_elapsed, 6);
            assert_eq!(close.jitter_ms, 0);
        }
    }

    #[test]
    fn closing_at_the_top_of_the_counters_saturates() {
        let env = Env::default();
        env.ledger().set_sequence_number(u32::MAX);
        env.ledger().set_timestamp(u64::MAX);
        let mut sim = ConsensusSimulator::new().with_jitter_std_ms(0.0);

        let close = sim.close_ledger(&env);

        assert_eq!(close.sequence, u32::MAX);
        assert_eq!(close.timestamp, u64::MAX);
    }

    #[test]
    fn the_builder_clamps_impossible_parameters() {
        let sim = ConsensusSimulator::new()
            .with_validators(0)
            .with_close_interval(8_000, 3_000)
            .with_jitter_std_ms(f64::NAN);

        assert_eq!(sim.validators(), 1);
        assert_eq!(sim.quorum(), 1);
        assert_eq!(sim.close_interval_range(), (3_000, 8_000));
        assert_eq!(sim.jitter_std_ms(), 0.0);

        let sim = ConsensusSimulator::new().with_validators(8).with_quorum(11);
        assert_eq!(sim.quorum(), 8);
    }

    #[test]
    fn a_contract_sees_the_ledger_the_round_closed() {
        let env = env();
        let client = feed(&env);
        let mut sim = ConsensusSimulator::new();

        let (round, height) = sim.execute_with_consensus(&env, || {
            env.ledger().sequence()
        });

        assert_eq!(height, round.close.sequence);
        assert_eq!(env.ledger().sequence(), round.close.sequence);
        assert_eq!(client.twap(), 0);
    }

    #[test]
    fn twap_of_a_constant_price_is_exact_under_jitter() {
        let env = env();
        let client = feed(&env);
        let mut sim = ConsensusSimulator::new().with_seed(1);

        client.record(&1_000_000);
        for _ in 0..50 {
            sim.run_round(&env);
            client.record(&1_000_000);
        }

        // With one price there is no weighting to get wrong: any time-weighted
        // average of a constant is that constant, whatever the intervals were.
        assert_eq!(client.twap(), 1_000_000);
        assert!(env.ledger().timestamp() > 1_700_000_000);
    }

    #[test]
    fn twap_weights_each_price_by_the_time_it_was_live() {
        let env = env();
        let client = feed(&env);
        let mut sim = ConsensusSimulator::new().with_seed(2);

        client.record(&100);
        let first: Vec<LedgerClose> = (0..20)
            .map(|_| {
                let close = sim.close_ledger(&env);
                client.record(&100);
                close
            })
            .collect();
        let switch = env.ledger().timestamp();
        let second: Vec<LedgerClose> = (0..20)
            .map(|_| {
                let close = sim.close_ledger(&env);
                client.record(&200);
                close
            })
            .collect();

        let low_seconds = switch - first.first().unwrap().prev_timestamp;
        let high_seconds = second.last().unwrap().timestamp - switch;
        let expected = (100 * low_seconds as i128 + 200 * high_seconds as i128)
            / (low_seconds as i128 + high_seconds as i128);

        assert_eq!(client.twap(), expected);
        assert!((100..=200).contains(&client.twap()));
    }

    #[test]
    fn twap_stays_stable_across_irregular_block_intervals() {
        // Twenty jittered closes price the asset at 100, the next twenty at
        // 200. However the intervals fall, the time-weighted average stays
        // between the two prices and near their midpoint, which is what a
        // contract that confuses heights with time fails to do.
        for seed in 1..=24 {
            let env = env();
            let client = feed(&env);
            let mut sim = ConsensusSimulator::new().with_seed(seed);

            client.record(&100);
            for _ in 0..20 {
                sim.close_ledger(&env);
                client.record(&100);
            }
            for _ in 0..20 {
                sim.close_ledger(&env);
                client.record(&200);
            }

            let twap = client.twap();
            assert!(
                (100..=200).contains(&twap),
                "seed {seed} produced a TWAP of {twap} outside the price range"
            );
            assert!(
                (twap - 150).abs() <= 20,
                "seed {seed} moved the TWAP to {twap}, more than jitter should"
            );
        }
    }
}
