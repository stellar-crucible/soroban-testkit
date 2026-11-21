//! Token conservation across a *sequence* of contract invocations.
//!
//! [`assert_balance_conserved!`](crate::assert_balance_conserved) wraps one
//! call. A real DeFi flow is several: an approval, a swap, a fee sweep, a
//! rebase. Checking only the end state hides a leak that a later call happens
//! to paper over, and a protocol fee that never reaches the treasury still
//! balances if the treasury is not part of the sum.
//!
//! [`assert_token_conservation_across_invocations!`](crate::assert_token_conservation_across_invocations)
//! snapshots every participant before the first call, after each call, and
//! reconciles:
//!
//! * token totals are unchanged by every individual invocation (fees are
//!   internal transfers to a treasury account that is part of the sum),
//! * each declared protocol fee is actually credited to its recipient,
//! * a mismatch prints a debit/credit table naming the account and invocation
//!   that leaked or minted tokens.
//!
//! ```rust
//! use soroban_sdk::{testutils::Address as _, token, Address, Env};
//! use soroban_testkit_assert::assert_token_conservation_across_invocations;
//!
//! let env = Env::default();
//! env.mock_all_auths();
//! let admin = Address::generate(&env);
//! let alice = Address::generate(&env);
//! let bob = Address::generate(&env);
//! let treasury = Address::generate(&env);
//!
//! let token_id = env.register_stellar_asset_contract_v2(admin.clone()).address();
//! let asset = token::StellarAssetClient::new(&env, &token_id);
//! let token = token::Client::new(&env, &token_id);
//! asset.mint(&alice, &1_000);
//!
//! assert_token_conservation_across_invocations!(
//!     accounts: [(&alice, "Alice"), (&bob, "Bob"), (&treasury, "Treasury")],
//!     balance: |address| token.balance(address),
//!     fees: [(&treasury, "Protocol fee", 10i128)],
//!     invocations: [
//!         {
//!             token.transfer(&alice, &bob, &100);
//!         },
//!         {
//!             token.transfer(&bob, &treasury, &10);
//!         },
//!     ],
//! );
//! ```
//!
//! The helper asserts conservation, so a mint or burn surfaces as a surplus or
//! leak. Use it for flows where the total supply is fixed (transfers, swaps,
//! fee splits), or account for the mint as an explicit treasury credit.

use std::fmt::Write as _;

use soroban_sdk::Address;

use crate::balance::{AccountBalanceEntry, TrackedAccount};

/// A protocol fee the flow promises to credit to `recipient`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeeClaim {
    /// Where the fee should land.
    pub recipient: Address,
    /// Human-readable label for reports.
    pub label: String,
    /// Amount that must be credited across the asserted flow.
    pub amount: i128,
}

impl FeeClaim {
    pub fn new(recipient: Address, label: impl Into<String>, amount: i128) -> Self {
        Self {
            recipient,
            label: label.into(),
            amount,
        }
    }
}

/// Anything that describes a declared protocol fee: `(address, amount)`,
/// `(address, label, amount)`, a reference to an address, or a [`FeeClaim`].
pub trait IntoFeeClaim {
    fn into_fee_claim(self) -> FeeClaim;
}

impl IntoFeeClaim for FeeClaim {
    fn into_fee_claim(self) -> FeeClaim {
        self
    }
}

impl IntoFeeClaim for (Address, i128) {
    fn into_fee_claim(self) -> FeeClaim {
        let label = format!("{:?}", self.0);
        FeeClaim::new(self.0, label, self.1)
    }
}

impl IntoFeeClaim for (&Address, i128) {
    fn into_fee_claim(self) -> FeeClaim {
        FeeClaim::new(self.0.clone(), format!("{:?}", self.0), self.1)
    }
}

impl<S: Into<String>> IntoFeeClaim for (Address, S, i128) {
    fn into_fee_claim(self) -> FeeClaim {
        FeeClaim::new(self.0, self.1, self.2)
    }
}

impl<S: Into<String>> IntoFeeClaim for (&Address, S, i128) {
    fn into_fee_claim(self) -> FeeClaim {
        FeeClaim::new(self.0.clone(), self.1, self.2)
    }
}

/// Per-invocation totals, captured between two calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvocationConservation {
    /// 1-based position in the asserted sequence.
    pub index: usize,
    /// Total across every tracked account before this invocation.
    pub pre_total: i128,
    /// Total after this invocation.
    pub post_total: i128,
    /// `post_total - pre_total`; must be zero for a closed system.
    pub delta: i128,
    /// Per-account balance change caused by this invocation.
    pub entries: Vec<AccountBalanceEntry>,
}

impl InvocationConservation {
    pub fn is_conserved(&self) -> bool {
        self.delta == 0
    }
}

/// Whether a declared fee reached its recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeeSettlement {
    pub label: String,
    pub recipient: Address,
    /// Declared fee, summed across the flow.
    pub declared: i128,
    /// Observed cumulative balance increase of the recipient.
    pub credited: i128,
}

impl FeeSettlement {
    pub fn is_satisfied(&self) -> bool {
        self.credited >= self.declared
    }
}

/// Full result of a multi-invocation conservation assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenConservationReport {
    /// One entry per invocation.
    pub invocations: Vec<InvocationConservation>,
    /// Overall pre/post/delta for every tracked account.
    pub accounts: Vec<AccountBalanceEntry>,
    /// Declared fees and what the recipients actually received.
    pub fees: Vec<FeeSettlement>,
    pub pre_total: i128,
    pub post_total: i128,
    /// `post_total - pre_total`; zero when nothing leaked or was minted.
    pub actual_delta: i128,
}

impl TokenConservationReport {
    /// True when every invocation conserved the total and every declared fee
    /// was credited.
    pub fn is_conserved(&self) -> bool {
        self.actual_delta == 0
            && self.invocations.iter().all(|entry| entry.is_conserved())
            && self.fees.iter().all(|fee| fee.is_satisfied())
    }

    /// The first invocation that leaked tokens, if any.
    pub fn first_breach(&self) -> Option<&InvocationConservation> {
        self.invocations.iter().find(|entry| !entry.is_conserved())
    }

    /// Side-by-side debit/credit reconciliation table plus the per-invocation
    /// and fee summaries.
    pub fn format_table(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "Token Conservation Breach: net delta = {} across {} invocation{}",
            self.actual_delta,
            self.invocations.len(),
            if self.invocations.len() == 1 { "" } else { "s" }
        );
        let _ = writeln!(
            out,
            "Total before: {} | Total after: {}",
            self.pre_total, self.post_total
        );
        out.push('\n');

        let width = self
            .accounts
            .iter()
            .map(|entry| entry.label.len())
            .chain(self.fees.iter().map(|fee| fee.label.len()))
            .chain([
                "Account / Reserve".len(),
                "Total Tracked".len(),
                "Declared Fee".len(),
                "Fee Credited".len(),
                "Fee Status".len(),
            ])
            .max()
            .unwrap_or(20)
            .max(20);

        let divider = "-".repeat(width + 14 + 14 + 22 + 13);
        let header = format!(
            "| {:<width$} | {:>12} | {:>12} | {:>20} |",
            "Account / Reserve",
            "Pre-Balance",
            "Post-Balance",
            "Delta (Debit/Credit)",
            width = width
        );

        out.push_str(&divider);
        out.push('\n');
        out.push_str(&header);
        out.push('\n');
        out.push_str(&divider);
        out.push('\n');

        for entry in &self.accounts {
            let _ = writeln!(
                out,
                "| {:<width$} | {:>12} | {:>12} | {:>20} |",
                entry.label,
                entry.pre,
                entry.post,
                signed(entry.delta),
                width = width
            );
        }

        out.push_str(&divider);
        out.push('\n');
        let _ = writeln!(
            out,
            "| {:<width$} | {:>12} | {:>12} | {:>20} |",
            "Total Tracked",
            self.pre_total,
            self.post_total,
            signed(self.actual_delta),
            width = width
        );

        if !self.fees.is_empty() {
            out.push_str(&divider);
            out.push('\n');
            let _ = writeln!(
                out,
                "| {:<width$} | {:>12} | {:>12} | {:>20} |",
                "Declared Fee",
                "Amount",
                "Credited",
                "Status",
                width = width
            );
            for fee in &self.fees {
                let _ = writeln!(
                    out,
                    "| {:<width$} | {:>12} | {:>12} | {:>20} |",
                    fee.label,
                    fee.declared,
                    fee.credited,
                    if fee.is_satisfied() {
                        "credited".to_string()
                    } else {
                        format!("SHORT by {}", fee.declared - fee.credited)
                    },
                    width = width
                );
            }
        }

        if !self.invocations.is_empty() {
            out.push_str(&divider);
            out.push('\n');
            let _ = writeln!(
                out,
                "| {:<width$} | {:>12} | {:>12} | {:>20} |",
                "Invocation",
                "Pre-Total",
                "Post-Total",
                "Delta",
                width = width
            );
            for invocation in &self.invocations {
                let _ = writeln!(
                    out,
                    "| {:<width$} | {:>12} | {:>12} | {:>20} |",
                    format!("#{}", invocation.index),
                    invocation.pre_total,
                    invocation.post_total,
                    if invocation.is_conserved() {
                        signed(invocation.delta)
                    } else {
                        format!("{} (LEAK)", signed(invocation.delta))
                    },
                    width = width
                );
            }
        }

        out.push_str(&divider);
        out.push('\n');
        out
    }
}

fn signed(value: i128) -> String {
    if value >= 0 {
        format!("+{value}")
    } else {
        format!("{value}")
    }
}

/// Binds a balance closure to the higher-ranked signature the tracker expects.
///
/// Without this, a closure written inline in a `let` has no expected type and
/// inference pins its argument to one concrete lifetime, which the tracker's
/// `&mut dyn FnMut(&Address)` parameter cannot accept.
#[doc(hidden)]
pub fn as_balance_fn<F>(balance: F) -> F
where
    F: for<'a> FnMut(&'a Address) -> i128,
{
    balance
}

/// Accumulates snapshots between invocations. Prefer the macro; this is public
/// so a test can drive the assertion from a loop when the call sequence is not
/// known at macro-expansion time.
pub struct TokenConservationTracker {
    accounts: Vec<TrackedAccount>,
    fees: Vec<FeeClaim>,
    initial: Vec<i128>,
    previous: Vec<i128>,
    invocations: Vec<InvocationConservation>,
    executed: usize,
}

impl TokenConservationTracker {
    /// Track `accounts` and declare `fees`; fee recipients missing from
    /// `accounts` are added automatically so their balance is part of the sum.
    pub fn new(
        mut accounts: Vec<TrackedAccount>,
        mut fees: Vec<FeeClaim>,
        balance_fn: &mut dyn FnMut(&Address) -> i128,
    ) -> Self {
        for fee in &mut fees {
            if !accounts
                .iter()
                .any(|account| account.address == fee.recipient)
            {
                accounts.push(TrackedAccount::new(
                    fee.recipient.clone(),
                    format!("{} (fee recipient)", fee.label),
                ));
            }
        }
        let initial: Vec<i128> = accounts
            .iter()
            .map(|account| balance_fn(&account.address))
            .collect();
        Self {
            accounts,
            fees,
            previous: initial.clone(),
            initial,
            invocations: Vec::new(),
            executed: 0,
        }
    }

    /// Run one invocation, snapshotting every tracked balance around it.
    pub fn run(&mut self, balance_fn: &mut dyn FnMut(&Address) -> i128, invocation: impl FnOnce()) {
        let pre = self.previous.clone();
        invocation();
        let post: Vec<i128> = self
            .accounts
            .iter()
            .map(|account| balance_fn(&account.address))
            .collect();

        let pre_total = pre.iter().sum();
        let post_total = post.iter().sum();
        let entries = self
            .accounts
            .iter()
            .zip(pre.iter().zip(post.iter()))
            .map(|(account, (before, after))| AccountBalanceEntry {
                label: account.label.clone(),
                pre: *before,
                post: *after,
                delta: after - before,
            })
            .collect();

        self.executed += 1;
        self.invocations.push(InvocationConservation {
            index: self.executed,
            pre_total,
            post_total,
            delta: post_total - pre_total,
            entries,
        });
        self.previous = post;
    }

    /// Finish the assertion, panicking with the reconciliation table when any
    /// invocation leaked or a declared fee did not reach its recipient.
    #[track_caller]
    pub fn finish(self) -> TokenConservationReport {
        let final_balances = &self.previous;
        let pre_total = self.initial.iter().sum();
        let post_total = final_balances.iter().sum();
        let accounts = self
            .accounts
            .iter()
            .zip(self.initial.iter().zip(final_balances.iter()))
            .map(|(account, (before, after))| AccountBalanceEntry {
                label: account.label.clone(),
                pre: *before,
                post: *after,
                delta: after - before,
            })
            .collect::<Vec<_>>();

        let fees = resolve_fee_credits(&self.accounts, &accounts, &self.fees);

        let report = TokenConservationReport {
            invocations: self.invocations,
            accounts,
            fees,
            pre_total,
            post_total,
            actual_delta: post_total - pre_total,
        };

        assert!(
            report.is_conserved(),
            "\nToken conservation invariant breached across invocations!\n\n{}",
            report.format_table()
        );
        report
    }
}

/// Group declared fees by recipient and compare each group against that
/// recipient's observed balance increase.
pub fn resolve_fee_credits(
    tracked: &[TrackedAccount],
    accounts: &[AccountBalanceEntry],
    fees: &[FeeClaim],
) -> Vec<FeeSettlement> {
    let mut grouped: Vec<(Address, String, i128)> = Vec::new();
    for fee in fees {
        if let Some(entry) = grouped
            .iter_mut()
            .find(|(recipient, _, _)| *recipient == fee.recipient)
        {
            entry.2 += fee.amount;
        } else {
            grouped.push((fee.recipient.clone(), fee.label.clone(), fee.amount));
        }
    }

    grouped
        .into_iter()
        .map(|(recipient, label, declared)| {
            let credited = tracked
                .iter()
                .position(|account| account.address == recipient)
                .and_then(|index| {
                    let account_label = &tracked[index].label;
                    accounts.iter().find(|entry| &entry.label == account_label)
                })
                .map(|entry| entry.delta)
                .unwrap_or(0);
            FeeSettlement {
                label,
                recipient,
                declared,
                credited,
            }
        })
        .collect()
}

/// Assert that token totals are conserved across a sequence of invocations and
/// that every declared protocol fee is credited to its recipient.
///
/// On any breach, panics with a debit/credit reconciliation table that names
/// the leaking invocation and shows each fee's declared versus credited amount.
///
/// # Examples
///
/// ```rust,ignore
/// assert_token_conservation_across_invocations!(
///     accounts: [(&alice, "Alice"), (&bob, "Bob"), (&treasury, "Treasury")],
///     balance: |address| token.balance(address),
///     fees: [(&treasury, "Protocol fee", 10i128)],
///     invocations: [
///         { token.transfer(&alice, &bob, &1_000); },
///         { router.swap(&bob, &alice, &500); },
///     ],
/// );
/// ```
#[macro_export]
macro_rules! assert_token_conservation_across_invocations {
    (
        accounts: $accounts:expr,
        balance: $balance_fn:expr,
        fees: [ $($fee:expr),* $(,)? ],
        invocations: [ $($call:block),* $(,)? ] $(,)?
    ) => {{
        let tracked: ::std::vec::Vec<$crate::balance::TrackedAccount> = $accounts
            .into_iter()
            .map($crate::balance::IntoTrackedAccount::into_tracked_account)
            .collect();
        let fees: ::std::vec::Vec<$crate::token_conservation::FeeClaim> = ::std::vec![
            $( $crate::token_conservation::IntoFeeClaim::into_fee_claim($fee) ),*
        ];
        let mut balance_fn = $crate::token_conservation::as_balance_fn($balance_fn);
        let mut tracker = $crate::token_conservation::TokenConservationTracker::new(
            tracked,
            fees,
            &mut balance_fn,
        );
        $(
            tracker.run(&mut balance_fn, || $call);
        )*
        tracker.finish()
    }};

    (
        accounts: $accounts:expr,
        balance: $balance_fn:expr,
        invocations: [ $($call:block),* $(,)? ] $(,)?
    ) => {{
        $crate::assert_token_conservation_across_invocations!(
            accounts: $accounts,
            balance: $balance_fn,
            fees: [],
            invocations: [ $($call),* ],
        )
    }};
}

#[cfg(test)]
mod tests {
    use super::{FeeClaim, IntoFeeClaim, TokenConservationTracker};
    use crate::balance::TrackedAccount;
    use soroban_sdk::{testutils::Address as _, Address, Env};
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    fn ledger() -> (
        Env,
        Address,
        Address,
        Address,
        RefCell<BTreeMap<Address, i128>>,
    ) {
        let env = Env::default();
        let alice = Address::generate(&env);
        let bob = Address::generate(&env);
        let treasury = Address::generate(&env);
        let balances = RefCell::new(BTreeMap::new());
        balances.borrow_mut().insert(alice.clone(), 1_000);
        balances.borrow_mut().insert(bob.clone(), 0);
        balances.borrow_mut().insert(treasury.clone(), 0);
        (env, alice, bob, treasury, balances)
    }

    fn tracked(address: Address, label: &str) -> TrackedAccount {
        TrackedAccount::new(address, label)
    }

    fn balance_fn(
        balances: &RefCell<BTreeMap<Address, i128>>,
    ) -> impl FnMut(&Address) -> i128 + '_ {
        move |address| *balances.borrow().get(address).unwrap_or(&0)
    }

    #[test]
    fn balanced_transfer_conserves_across_invocations() {
        let (_env, alice, bob, treasury, balances) = ledger();
        let mut balance = balance_fn(&balances);
        let mut tracker = TokenConservationTracker::new(
            vec![tracked(alice.clone(), "Alice"), tracked(bob.clone(), "Bob")],
            vec![],
            &mut balance,
        );

        tracker.run(&mut balance, || {
            let mut map = balances.borrow_mut();
            *map.get_mut(&alice).unwrap() -= 100;
            *map.get_mut(&bob).unwrap() += 100;
        });
        tracker.run(&mut balance, || {
            let mut map = balances.borrow_mut();
            *map.get_mut(&bob).unwrap() -= 40;
            *map.get_mut(&treasury).unwrap() += 40;
        });

        // The treasury is not tracked but is harmless only if it is declared;
        // this tracker still sees a 40-token drop, which must be reported.
        let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tracker.finish()));
        assert!(report.is_err(), "an untracked fee must break conservation");
    }

    #[test]
    fn fee_split_with_declared_treasury_is_conserved() {
        let (_env, alice, bob, treasury, balances) = ledger();
        let mut balance = balance_fn(&balances);
        let fee = FeeClaim::new(treasury.clone(), "Treasury", 10);
        let mut tracker = TokenConservationTracker::new(
            vec![tracked(alice.clone(), "Alice"), tracked(bob.clone(), "Bob")],
            vec![fee],
            &mut balance,
        );

        tracker.run(&mut balance, || {
            let mut map = balances.borrow_mut();
            *map.get_mut(&alice).unwrap() -= 100;
            *map.get_mut(&bob).unwrap() += 90;
            *map.get_mut(&treasury).unwrap() += 10;
        });

        let report = tracker.finish();
        assert!(report.is_conserved());
        assert_eq!(report.fees[0].credited, 10);
        assert_eq!(report.invocations.len(), 1);
    }

    #[test]
    fn a_fee_that_never_reaches_treasury_is_flagged() {
        let (_env, alice, bob, treasury, balances) = ledger();
        let mut balance = balance_fn(&balances);
        let mut tracker = TokenConservationTracker::new(
            vec![tracked(alice.clone(), "Alice"), tracked(bob.clone(), "Bob")],
            vec![FeeClaim::new(treasury, "Treasury", 10)],
            &mut balance,
        );

        tracker.run(&mut balance, || {
            let mut map = balances.borrow_mut();
            *map.get_mut(&alice).unwrap() -= 100;
            *map.get_mut(&bob).unwrap() += 100;
        });

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tracker.finish()));
        assert!(outcome.is_err());
    }

    #[test]
    fn a_rounding_leak_on_a_later_invocation_names_that_invocation() {
        let (_env, alice, bob, treasury, balances) = ledger();
        let mut balance = balance_fn(&balances);
        let mut tracker = TokenConservationTracker::new(
            vec![tracked(alice.clone(), "Alice"), tracked(bob.clone(), "Bob")],
            vec![FeeClaim::new(treasury.clone(), "Treasury", 1)],
            &mut balance,
        );

        tracker.run(&mut balance, || {
            let mut map = balances.borrow_mut();
            *map.get_mut(&alice).unwrap() -= 100;
            *map.get_mut(&bob).unwrap() += 99;
            *map.get_mut(&treasury).unwrap() += 1;
        });
        // Fractional rounding drops 1 token on this leg.
        tracker.run(&mut balance, || {
            let mut map = balances.borrow_mut();
            *map.get_mut(&bob).unwrap() -= 50;
            *map.get_mut(&alice).unwrap() += 49;
        });

        let Err(payload) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tracker.finish()))
        else {
            panic!("a leaky leg must fail the assertion");
        };
        let message = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|text| text.to_string()))
            .unwrap_or_default();
        assert!(message.contains("Invocation"), "message was: {message}");
        assert!(message.contains("#2"), "message was: {message}");
        assert!(message.contains("LEAK"), "message was: {message}");
    }

    #[test]
    fn the_macro_asserts_a_two_invocation_fee_flow() {
        let (_env, alice, bob, treasury, balances) = ledger();

        let report = crate::assert_token_conservation_across_invocations!(
            accounts: [(&alice, "Alice"), (&bob, "Bob")],
            balance: |address| *balances.borrow().get(address).unwrap_or(&0),
            fees: [(&treasury, "Treasury", 10i128)],
            invocations: [
                {
                    let mut map = balances.borrow_mut();
                    *map.get_mut(&alice).unwrap() -= 100;
                    *map.get_mut(&bob).unwrap() += 100;
                },
                {
                    let mut map = balances.borrow_mut();
                    *map.get_mut(&bob).unwrap() -= 10;
                    *map.get_mut(&treasury).unwrap() += 10;
                },
            ],
        );

        assert!(report.is_conserved());
        assert_eq!(report.invocations.len(), 2);
        assert_eq!(report.fees[0].credited, 10);
    }

    #[test]
    #[should_panic(expected = "Token conservation invariant breached across invocations!")]
    fn the_macro_reports_a_leaking_invocation() {
        let (_env, alice, bob, _treasury, balances) = ledger();

        crate::assert_token_conservation_across_invocations!(
            accounts: [(&alice, "Alice"), (&bob, "Bob")],
            balance: |address| *balances.borrow().get(address).unwrap_or(&0),
            invocations: [
                {
                    let mut map = balances.borrow_mut();
                    *map.get_mut(&alice).unwrap() -= 100;
                    *map.get_mut(&bob).unwrap() += 99;
                },
            ],
        );
    }

    #[test]
    fn fee_tuple_conversions_keep_labels_and_amounts() {
        let env = Env::default();
        let treasury = Address::generate(&env);
        assert_eq!((&treasury, 5i128).into_fee_claim().amount, 5);
        assert_eq!(
            (&treasury, "Protocol", 7i128).into_fee_claim().label,
            "Protocol"
        );
        assert_eq!(
            (treasury.clone(), 9i128).into_fee_claim().recipient,
            treasury
        );
    }
}
