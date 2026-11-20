//! Strategies for multi-signature threshold policies.
//!
//! A multi-sig wallet, a governance council and a threshold escrow all describe
//! the same shape: a set of signers, a weight per signer, and a threshold the
//! combined weight of an approving subset has to reach. Writing that by hand is
//! error-prone — a generated threshold above the total weight can never be met,
//! and a "failing" subset that accidentally clears the threshold turns a
//! threshold-enforcement test into a false positive.
//!
//! [`multisig_policy`] generates a policy whose threshold is reachable by
//! construction: it draws `count` weighted account signers and then a threshold
//! between one and the total weight. [`valid_signer_subset`] and
//! [`insufficient_signer_subset`] derive subsets from a policy and *repair* them
//! rather than filtering, so every draw satisfies its invariant and shrinking
//! keeps it that way.
//!
//! ```text
//! proptest! {
//!     #[test]
//!     fn a_wallet_accepts_every_approving_subset(
//!         (policy, approving) in multisig_with_valid_subset(3..10),
//!     ) {
//!         assert!(policy.approves(&approving));
//!         assert!(wallet.verify_approval(&approving));
//!     }
//! }
//! ```

use std::ops::Range;

use proptest::prelude::*;

use crate::strategies::{account_address, AddressBytes};

/// The largest weight a generated signer carries.
///
/// Weights are small so that a printed case stays readable and so that a
/// threshold drawn from the total weight lands inside it often enough for a
/// property to explore both approving and failing subsets.
pub const MAX_SIGNER_WEIGHT: u32 = 100;

/// One signer in a multi-signature policy: an address and the weight its
/// approval contributes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Signer {
    address: AddressBytes,
    weight: u32,
}

impl Signer {
    /// A signer with a positive weight.
    ///
    /// # Panics
    ///
    /// Panics when `weight` is zero: a zero-weight signer can never help an
    /// approval reach the threshold, which makes it a policy bug rather than a
    /// value worth generating.
    pub fn new(address: AddressBytes, weight: u32) -> Self {
        assert!(weight > 0, "a signer weight must be positive");
        Self { address, weight }
    }

    /// The signer's address.
    pub fn address(&self) -> AddressBytes {
        self.address
    }

    /// The weight one approval from this signer contributes.
    pub fn weight(&self) -> u32 {
        self.weight
    }
}

/// A multi-signature policy: weighted signers and the weight an approving
/// subset must reach.
///
/// The policy is valid by construction — at least one signer, every weight
/// positive, and a threshold between one and the total weight — so a test can
/// use the whole set as an approving subset and the empty set as a failing one
/// without checking either.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultisigPolicy {
    signers: Vec<Signer>,
    threshold: u64,
}

impl MultisigPolicy {
    /// Build a policy, panicking when it could never be satisfied.
    ///
    /// # Panics
    ///
    /// Panics when there are no signers, when any weight is zero, or when the
    /// threshold is zero or above the total weight.
    pub fn new(signers: Vec<Signer>, threshold: u64) -> Self {
        assert!(!signers.is_empty(), "a policy needs at least one signer");
        let total: u64 = signers.iter().map(|signer| u64::from(signer.weight)).sum();
        assert!(
            total >= threshold && threshold > 0,
            "a threshold of {threshold} must be between 1 and the total weight {total}"
        );
        Self { signers, threshold }
    }

    /// The weighted signers, in the order the policy lists them.
    pub fn signers(&self) -> &[Signer] {
        &self.signers
    }

    /// How much combined weight an approving subset must reach.
    pub fn threshold(&self) -> u64 {
        self.threshold
    }

    /// The combined weight of every signer.
    pub fn total_weight(&self) -> u64 {
        self.signers
            .iter()
            .map(|signer| u64::from(signer.weight))
            .sum()
    }

    /// The number of signers in the policy.
    pub fn len(&self) -> usize {
        self.signers.len()
    }

    /// Whether the policy lists no signers. Always false for a valid policy.
    pub fn is_empty(&self) -> bool {
        self.signers.is_empty()
    }

    /// The combined weight of `subset`.
    ///
    /// Only addresses the policy knows count, and each signer counts once no
    /// matter how often the subset repeats it, so a duplicated address cannot
    /// inflate a subset over the threshold.
    pub fn subset_weight(&self, subset: &[AddressBytes]) -> u64 {
        self.signers
            .iter()
            .filter(|signer| subset.contains(&signer.address))
            .map(|signer| u64::from(signer.weight))
            .sum()
    }

    /// Whether `subset` meets the threshold.
    pub fn approves(&self, subset: &[AddressBytes]) -> bool {
        self.subset_weight(subset) >= self.threshold
    }

    /// The generated addresses of every signer.
    pub fn addresses(&self) -> Vec<AddressBytes> {
        self.signers.iter().map(|signer| signer.address).collect()
    }
}

/// Generates a weighted multi-signature policy with between `min_signers` and
/// `max_signers` signers.
///
/// The threshold is drawn between one and the sum of the generated weights, so
/// the whole signer set always approves and the empty set never does. Signers
/// are distinct account addresses produced by
/// [`account_address`](crate::strategies::account_address), which carries a
/// process-wide nonce.
///
/// # Panics
///
/// Panics when `min_signers` is zero or `max_signers` is below it.
///
/// ```text
/// proptest! {
///     #[test]
///     fn a_policy_is_reachable(policy in multisig_policy(2, 6)) {
///         assert!(policy.threshold() <= policy.total_weight());
///     }
/// }
/// ```
pub fn multisig_policy(
    min_signers: usize,
    max_signers: usize,
) -> impl Strategy<Value = MultisigPolicy> {
    assert!(min_signers >= 1, "a policy needs at least one signer");
    assert!(
        max_signers >= min_signers,
        "max_signers ({max_signers}) must be at least min_signers ({min_signers})"
    );

    (min_signers..=max_signers).prop_flat_map(|count| {
        prop::collection::vec((account_address(), 1u32..=MAX_SIGNER_WEIGHT), count).prop_flat_map(
            |entries| {
                let total: u64 = entries.iter().map(|(_, weight)| u64::from(*weight)).sum();
                (1u64..=total).prop_map(move |threshold| {
                    let signers = entries
                        .iter()
                        .map(|(address, weight)| Signer::new(*address, *weight))
                        .collect();
                    MultisigPolicy::new(signers, threshold)
                })
            },
        )
    })
}

/// Generates an approving subset of `policy`: every address it contains is one
/// of the policy's signers, and their combined weight is at least the
/// threshold.
///
/// The subset starts from a random mask and is then topped up with signers the
/// mask left out until the threshold is reached. Repairing rather than
/// filtering means every case is valid — no `prop_filter` rejections, no
/// `prop_assume!` skips — and shrinking the mask still yields an approving
/// subset.
///
/// # Panics
///
/// Panics when `policy` is not a reachable policy.
pub fn valid_signer_subset(policy: &MultisigPolicy) -> impl Strategy<Value = Vec<AddressBytes>> {
    let policy = policy.clone();
    assert_reachable(&policy);

    let count = policy.signers.len();
    prop::collection::vec(any::<bool>(), count).prop_map(move |mask| {
        let mut chosen = selected(&policy, &mask);
        let mut weight = policy.subset_weight(&chosen);

        // The mask can leave the subset short. Walk the policy in order and add
        // whatever is still missing; the total weight is at least the threshold,
        // so the whole set is always enough.
        for signer in &policy.signers {
            if weight >= policy.threshold {
                break;
            }
            if !chosen.contains(&signer.address) {
                chosen.push(signer.address);
                weight += u64::from(signer.weight);
            }
        }

        debug_assert!(policy.approves(&chosen));
        chosen
    })
}

/// Generates a subset of `policy` whose combined weight is strictly below the
/// threshold.
///
/// The subset starts from a random mask and drops signers until it no longer
/// approves, so the empty subset — which is always below a positive threshold —
/// is the smallest case shrinking reaches. Every draw is a genuine failing
/// subset, which is what keeps a threshold-enforcement test from passing by
/// accident.
///
/// # Panics
///
/// Panics when `policy` is not a reachable policy.
pub fn insufficient_signer_subset(
    policy: &MultisigPolicy,
) -> impl Strategy<Value = Vec<AddressBytes>> {
    let policy = policy.clone();
    assert_reachable(&policy);

    let count = policy.signers.len();
    prop::collection::vec(any::<bool>(), count).prop_map(move |mask| {
        let mut chosen = selected(&policy, &mask);
        while !chosen.is_empty() && policy.approves(&chosen) {
            chosen.pop();
        }

        debug_assert!(!policy.approves(&chosen));
        chosen
    })
}

/// Generates a policy with a signer count in `signers` together with an
/// approving subset of it.
///
/// This is the shape a threshold-enforcement property wants: the policy and the
/// signatures that must satisfy the wallet come from the same draw, so the two
/// cannot disagree.
///
/// # Panics
///
/// Panics when the range is empty or starts below one.
///
/// ```text
/// proptest! {
///     #[test]
///     fn test_multisig_threshold(
///         (policy, approving_signers) in multisig_with_valid_subset(3..10),
///     ) {
///         assert!(wallet.verify_approval(&approving_signers));
///     }
/// }
/// ```
pub fn multisig_with_valid_subset(
    signers: Range<usize>,
) -> impl Strategy<Value = (MultisigPolicy, Vec<AddressBytes>)> {
    multisig_policy_of(&signers).prop_flat_map(|policy| {
        let paired = policy.clone();
        valid_signer_subset(&policy).prop_map(move |subset| (paired.clone(), subset))
    })
}

/// Generates a policy with a signer count in `signers` together with a
/// sub-threshold subset of it.
///
/// The counterpart of [`multisig_with_valid_subset`] for the branch that must
/// reject the approval.
///
/// # Panics
///
/// Panics when the range is empty or starts below one.
pub fn multisig_with_insufficient_subset(
    signers: Range<usize>,
) -> impl Strategy<Value = (MultisigPolicy, Vec<AddressBytes>)> {
    multisig_policy_of(&signers).prop_flat_map(|policy| {
        let paired = policy.clone();
        insufficient_signer_subset(&policy).prop_map(move |subset| (paired.clone(), subset))
    })
}

/// A policy whose signer count falls inside an exclusive range, one count per
/// draw so shrinking a case also shrinks the number of signers.
fn multisig_policy_of(signers: &Range<usize>) -> impl Strategy<Value = MultisigPolicy> {
    assert!(
        signers.start >= 1 && signers.end > signers.start,
        "the signer range must start at one or more and be non-empty"
    );
    let (min, max) = (signers.start, signers.end - 1);
    (min..=max).prop_flat_map(|count| multisig_policy(count, count))
}

/// The policy signers a mask selects, in policy order.
fn selected(policy: &MultisigPolicy, mask: &[bool]) -> Vec<AddressBytes> {
    policy
        .signers
        .iter()
        .enumerate()
        .filter(|(index, _)| mask.get(*index).copied().unwrap_or(false))
        .map(|(_, signer)| signer.address)
        .collect()
}

/// Check that a policy handed to a subset strategy can produce both branches.
fn assert_reachable(policy: &MultisigPolicy) {
    let total = policy.total_weight();
    assert!(
        !policy.signers.is_empty() && policy.threshold > 0 && policy.threshold <= total,
        "a policy must have at least one signer and a threshold between 1 and its total weight {total}"
    );
}

/// Generates a weighted signer for one of the policy's addresses.
///
/// Useful when a test builds a policy by hand and wants a fuzzed weight for a
/// signer it already named.
pub fn signer_weight() -> impl Strategy<Value = u32> {
    1u32..=MAX_SIGNER_WEIGHT
}

#[cfg(test)]
mod tests {
    use super::{
        insufficient_signer_subset, multisig_policy, multisig_with_insufficient_subset,
        multisig_with_valid_subset, signer_weight, valid_signer_subset, MultisigPolicy, Signer,
        MAX_SIGNER_WEIGHT,
    };
    use crate::strategies::account_address;
    use proptest::prelude::*;
    use proptest::test_runner::{TestError, TestRunner};

    fn fixed(variant: u8) -> crate::strategies::AddressBytes {
        let mut bytes = [0u8; 32];
        bytes[31] = variant;
        crate::strategies::AddressBytes::from_parts(crate::strategies::AddressType::Account, bytes)
    }

    fn three_of_five() -> MultisigPolicy {
        MultisigPolicy::new(
            vec![
                Signer::new(fixed(1), 1),
                Signer::new(fixed(2), 2),
                Signer::new(fixed(3), 1),
                Signer::new(fixed(4), 2),
                Signer::new(fixed(5), 1),
            ],
            4,
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn a_generated_policy_is_always_reachable(policy in multisig_policy(1, 8)) {
            prop_assert!(!policy.signers().is_empty());
            prop_assert!(policy.threshold() >= 1);
            prop_assert!(policy.threshold() <= policy.total_weight());
            prop_assert!(policy.signers().iter().all(|signer| signer.weight() >= 1));
        }

        #[test]
        fn the_whole_signer_set_always_approves(policy in multisig_policy(1, 8)) {
            let all = policy.addresses();
            prop_assert!(policy.approves(&all));
            prop_assert_eq!(policy.subset_weight(&all), policy.total_weight());
        }

        #[test]
        fn the_empty_subset_never_approves(policy in multisig_policy(1, 8)) {
            prop_assert!(!policy.approves(&[]));
        }

        #[test]
        fn a_valid_subset_is_a_real_subset(
            (policy, subset) in multisig_with_valid_subset(1..9)
        ) {
            let addresses = policy.addresses();
            prop_assert!(policy.approves(&subset));
            prop_assert!(subset.iter().all(|candidate| addresses.contains(candidate)));
            prop_assert!(policy.subset_weight(&subset) >= policy.threshold());
        }

        #[test]
        fn an_insufficient_subset_is_a_real_failing_subset(
            (policy, subset) in multisig_with_insufficient_subset(1..9)
        ) {
            let addresses = policy.addresses();
            prop_assert!(!policy.approves(&subset));
            prop_assert!(policy.subset_weight(&subset) < policy.threshold());
            prop_assert!(subset.iter().all(|candidate| addresses.contains(candidate)));
        }

        #[test]
        fn signer_counts_stay_inside_the_request(policy in multisig_policy(3, 6)) {
            prop_assert!((3..=6).contains(&policy.len()));
        }

        #[test]
        fn weights_stay_inside_the_published_maximum(policy in multisig_policy(1, 10)) {
            prop_assert!(policy.signers().iter().all(|signer| signer.weight() <= MAX_SIGNER_WEIGHT));
        }
    }

    #[test]
    fn subset_strategies_are_repaired_not_filtered() {
        // Every generated case must satisfy the strategy's contract, so a run
        // that ends without a failure is a statement about all draws rather than
        // about the ones a filter let through.
        let policy = three_of_five();
        let mut runner = TestRunner::new(ProptestConfig::with_cases(256));

        runner
            .run(&valid_signer_subset(&policy), |subset| {
                prop_assert!(policy.approves(&subset));
                Ok(())
            })
            .expect("an approving subset is always approving");
        runner
            .run(&insufficient_signer_subset(&policy), |subset| {
                prop_assert!(!policy.approves(&subset));
                Ok(())
            })
            .expect("a failing subset is always failing");
    }

    #[test]
    fn shrinking_an_insufficient_subset_keeps_it_below_the_threshold() {
        // Every generated case fails the property, so proptest shrinks to the
        // smallest subset it can build. Whatever it lands on must still be a
        // failing subset: shrinking may not repair it into an approving one.
        let policy = three_of_five();
        let mut runner = TestRunner::new(ProptestConfig {
            cases: 1,
            max_shrink_iters: 10_000,
            ..ProptestConfig::default()
        });

        let outcome = runner.run(&insufficient_signer_subset(&policy), |subset| {
            prop_assert!(
                policy.approves(&subset),
                "an insufficient subset of {} signers unexpectedly approves",
                subset.len()
            );
            Ok(())
        });

        match outcome {
            Err(TestError::Fail(_, smallest)) => {
                assert!(!policy.approves(&smallest));
                assert!(policy.subset_weight(&smallest) < policy.threshold());
            }
            other => panic!("expected a shrunk counterexample, got {other:?}"),
        }
    }

    #[test]
    fn shrinking_a_valid_subset_keeps_the_threshold_met() {
        // The mask shrinks towards "select nothing", and the top-up step is what
        // keeps the result approving: the smallest case is the first signer
        // whose weight reaches the threshold.
        let policy = three_of_five();
        let mut runner = TestRunner::new(ProptestConfig {
            cases: 1,
            max_shrink_iters: 10_000,
            ..ProptestConfig::default()
        });

        let outcome = runner.run(&valid_signer_subset(&policy), |subset| {
            // This property is deliberately false for every non-empty subset, so
            // proptest shrinks to the smallest approving subset it can build.
            prop_assert!(subset.is_empty(), "generated an approving subset");
            Ok(())
        });

        match outcome {
            Err(TestError::Fail(_, smallest)) => {
                assert!(!smallest.is_empty());
                assert!(policy.approves(&smallest));
                assert_eq!(policy.subset_weight(&smallest), 4);
            }
            other => panic!("expected a shrunk counterexample, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_addresses_do_not_inflate_a_subset() {
        let policy = three_of_five();
        let signer = policy.signers()[0].address();

        // One signer listed five times is still one signer's weight.
        let repeated = vec![signer; 5];
        assert_eq!(policy.subset_weight(&repeated), 1);
        assert!(!policy.approves(&repeated));
    }

    #[test]
    fn addresses_outside_the_policy_carry_no_weight() {
        let policy = three_of_five();
        let outsider = fixed(99);

        assert_eq!(policy.subset_weight(&[outsider]), 0);
        assert!(!policy.approves(&[outsider]));
    }

    #[test]
    fn an_unreachable_policy_is_refused() {
        let signers = vec![Signer::new(fixed(1), 1)];
        assert!(std::panic::catch_unwind(|| MultisigPolicy::new(signers, 2)).is_err());
    }

    #[test]
    fn a_zero_weight_signer_is_refused() {
        assert!(std::panic::catch_unwind(|| Signer::new(fixed(1), 0)).is_err());
    }

    #[test]
    fn the_generated_signers_are_distinct() {
        let mut runner = TestRunner::default();
        runner
            .run(
                &(multisig_policy(4, 4), account_address()),
                |(policy, extra)| {
                    let mut addresses = policy.addresses();
                    addresses.push(extra);
                    let unique: std::collections::HashSet<_> = addresses.iter().collect();
                    prop_assert_eq!(unique.len(), addresses.len());
                    Ok(())
                },
            )
            .unwrap();
    }

    #[test]
    fn a_hand_built_policy_keeps_its_terms() {
        let policy = three_of_five();

        assert_eq!(policy.len(), 5);
        assert_eq!(policy.threshold(), 4);
        assert_eq!(policy.total_weight(), 7);
        assert!(policy.approves(&[fixed(2), fixed(4)]));
        assert!(!policy.approves(&[fixed(1), fixed(2)]));
    }

    #[test]
    fn the_weight_strategy_stays_positive_and_bounded() {
        let mut runner = TestRunner::default();
        runner
            .run(&signer_weight(), |weight| {
                prop_assert!(weight >= 1);
                prop_assert!(weight <= MAX_SIGNER_WEIGHT);
                Ok(())
            })
            .unwrap();
    }
}
