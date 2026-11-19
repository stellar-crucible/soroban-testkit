//! Token balance conservation assertion macro and reconciliation tooling.
//!
//! Enforces balance conservation invariants across multi-user operations (e.g. transfers,
//! swaps, batch distributions, and liquidity pools). Formats an informative side-by-side
//! debit/credit reconciliation table on breach.

use soroban_sdk::Address;
use std::fmt::Write as _;

/// An account or contract reserve tracked during balance conservation assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedAccount {
    /// Human-readable label for debugging and table display.
    pub label: String,
    /// On-chain address of the account or contract.
    pub address: Address,
}

impl TrackedAccount {
    /// Create a new tracked account with an explicit label.
    pub fn new(address: Address, label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            address,
        }
    }
}

/// Conversion trait allowing addresses, references, and labeled pairs to be passed seamlessly.
pub trait IntoTrackedAccount {
    fn into_tracked_account(self) -> TrackedAccount;
}

impl IntoTrackedAccount for Address {
    fn into_tracked_account(self) -> TrackedAccount {
        let label = format!("{self:?}");
        TrackedAccount {
            label,
            address: self,
        }
    }
}

impl IntoTrackedAccount for &Address {
    fn into_tracked_account(self) -> TrackedAccount {
        let label = format!("{self:?}");
        TrackedAccount {
            label,
            address: self.clone(),
        }
    }
}

impl<S: Into<String>> IntoTrackedAccount for (Address, S) {
    fn into_tracked_account(self) -> TrackedAccount {
        TrackedAccount {
            label: self.1.into(),
            address: self.0,
        }
    }
}

impl<S: Into<String>> IntoTrackedAccount for (&Address, S) {
    fn into_tracked_account(self) -> TrackedAccount {
        TrackedAccount {
            label: self.1.into(),
            address: self.0.clone(),
        }
    }
}

impl IntoTrackedAccount for TrackedAccount {
    fn into_tracked_account(self) -> TrackedAccount {
        self
    }
}

/// A snapshot of an individual account's balance before and after an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountBalanceEntry {
    /// Account label or address.
    pub label: String,
    /// Balance prior to invocation.
    pub pre: i128,
    /// Balance following invocation.
    pub post: i128,
    /// Net delta (`post - pre`).
    pub delta: i128,
}

/// Reconciliation report summarizing the multi-account balance invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BalanceConservationReport {
    /// Per-account balance snapshots.
    pub accounts: Vec<AccountBalanceEntry>,
    /// Sum of all pre-invocation balances.
    pub pre_total: i128,
    /// Sum of all post-invocation balances.
    pub post_total: i128,
    /// Observed change across all tracked balances (`post_total - pre_total`).
    pub actual_delta: i128,
    /// Expected tokens minted during the block.
    pub mints: i128,
    /// Expected tokens burned during the block.
    pub burns: i128,
    /// Expected change (`mints - burns`).
    pub expected_delta: i128,
    /// Discrepancy between actual and expected change (`actual_delta - expected_delta`).
    pub discrepancy: i128,
}

impl BalanceConservationReport {
    /// Returns true if balances are strictly conserved according to expected mints/burns.
    pub fn is_conserved(&self) -> bool {
        self.discrepancy == 0
    }

    /// Formats a side-by-side debit/credit reconciliation table.
    pub fn format_table(&self) -> String {
        let mut out = String::new();

        let leak_or_surplus = if self.discrepancy < 0 {
            format!("{} (LEAK DETECTED)", self.discrepancy)
        } else if self.discrepancy > 0 {
            format!("+{} (SURPLUS DETECTED)", self.discrepancy)
        } else {
            "0 (CONSERVED)".to_string()
        };

        let _ = writeln!(
            out,
            "Balance Conservation Breach: Discrepancy = {leak_or_surplus}"
        );
        let _ = writeln!(
            out,
            "Expected net delta: {} (mints: +{}, burns: -{}) | Actual net delta: {}",
            self.expected_delta, self.mints, self.burns, self.actual_delta
        );
        out.push('\n');

        let col_acc_width = self
            .accounts
            .iter()
            .map(|a| a.label.len())
            .chain([
                "Account / Reserve".len(),
                "Total Tracked".len(),
                "External Mints (+)".len(),
                "External Burns (-)".len(),
                "Net Expected Delta".len(),
                "Discrepancy (Leak/Surplus)".len(),
            ])
            .max()
            .unwrap_or(20)
            .max(22);

        let col_pre_width = 14;
        let col_post_width = 14;
        let col_delta_width = 24;

        let total_table_width =
            col_acc_width + col_pre_width + col_post_width + col_delta_width + 13;
        let divider = "-".repeat(total_table_width);

        out.push_str(&divider);
        out.push('\n');
        let _ = writeln!(
            out,
            "| {:<w_acc$} | {:>w_pre$} | {:>w_post$} | {:>w_delta$} |",
            "Account / Reserve",
            "Pre-Balance",
            "Post-Balance",
            "Delta (Debit/Credit)",
            w_acc = col_acc_width,
            w_pre = col_pre_width,
            w_post = col_post_width,
            w_delta = col_delta_width,
        );
        out.push_str(&divider);
        out.push('\n');

        for acc in &self.accounts {
            let delta_str = if acc.delta > 0 {
                format!("+{}", acc.delta)
            } else {
                format!("{}", acc.delta)
            };

            let _ = writeln!(
                out,
                "| {:<w_acc$} | {:>w_pre$} | {:>w_post$} | {:>w_delta$} |",
                acc.label,
                acc.pre,
                acc.post,
                delta_str,
                w_acc = col_acc_width,
                w_pre = col_pre_width,
                w_post = col_post_width,
                w_delta = col_delta_width,
            );
        }

        out.push_str(&divider);
        out.push('\n');

        let total_delta_str = if self.actual_delta > 0 {
            format!("+{}", self.actual_delta)
        } else {
            format!("{}", self.actual_delta)
        };

        let _ = writeln!(
            out,
            "| {:<w_acc$} | {:>w_pre$} | {:>w_post$} | {:>w_delta$} |",
            "Total Tracked",
            self.pre_total,
            self.post_total,
            total_delta_str,
            w_acc = col_acc_width,
            w_pre = col_pre_width,
            w_post = col_post_width,
            w_delta = col_delta_width,
        );

        let _ = writeln!(
            out,
            "| {:<w_acc$} | {:>w_pre$} | {:>w_post$} | {:>w_delta$} |",
            "External Mints (+)",
            "",
            "",
            format!("+{}", self.mints),
            w_acc = col_acc_width,
            w_pre = col_pre_width,
            w_post = col_post_width,
            w_delta = col_delta_width,
        );

        let _ = writeln!(
            out,
            "| {:<w_acc$} | {:>w_pre$} | {:>w_post$} | {:>w_delta$} |",
            "External Burns (-)",
            "",
            "",
            format!("-{}", self.burns),
            w_acc = col_acc_width,
            w_pre = col_pre_width,
            w_post = col_post_width,
            w_delta = col_delta_width,
        );

        out.push_str(&divider);
        out.push('\n');

        let _ = writeln!(
            out,
            "| {:<w_acc$} | {:>w_pre$} | {:>w_post$} | {:>w_delta$} |",
            "Discrepancy",
            "",
            "",
            leak_or_surplus,
            w_acc = col_acc_width,
            w_pre = col_pre_width,
            w_post = col_post_width,
            w_delta = col_delta_width,
        );

        out.push_str(&divider);
        out.push('\n');

        out
    }
}

/// Internal execution helper for the `assert_balance_conserved!` macro.
pub fn assert_balance_conserved_internal<R, FBalance, FExec>(
    accounts: Vec<TrackedAccount>,
    mut balance_fn: FBalance,
    mints: i128,
    burns: i128,
    exec_fn: FExec,
) -> R
where
    FBalance: FnMut(&Address) -> i128,
    FExec: FnOnce() -> R,
{
    let pre_balances: Vec<i128> = accounts
        .iter()
        .map(|acc| balance_fn(&acc.address))
        .collect();
    let result = exec_fn();
    let post_balances: Vec<i128> = accounts
        .iter()
        .map(|acc| balance_fn(&acc.address))
        .collect();

    let mut pre_total: i128 = 0;
    let mut post_total: i128 = 0;
    let mut entries = Vec::with_capacity(accounts.len());

    for (i, acc) in accounts.into_iter().enumerate() {
        let pre = pre_balances[i];
        let post = post_balances[i];
        let delta = post - pre;
        pre_total += pre;
        post_total += post;
        entries.push(AccountBalanceEntry {
            label: acc.label,
            pre,
            post,
            delta,
        });
    }

    let actual_delta = post_total - pre_total;
    let expected_delta = mints - burns;
    let discrepancy = actual_delta - expected_delta;

    let report = BalanceConservationReport {
        accounts: entries,
        pre_total,
        post_total,
        actual_delta,
        mints,
        burns,
        expected_delta,
        discrepancy,
    };

    if !report.is_conserved() {
        panic!(
            "\nBalance conservation invariant breached!\n\n{}",
            report.format_table()
        );
    }

    result
}

/// Assert that token balances across arbitrary user accounts and contract reserves remain conserved.
///
/// Captures pre-invocation balances, executes the provided closure, and verifies that total tokens
/// across all tracked accounts plus contract reserves are conserved, accounting for any external
/// mints or burns occurring within the operation.
///
/// On invariant violation, panics with a formatted debit/credit balance reconciliation table.
///
/// # Examples
///
/// ```rust,ignore
/// use soroban_testkit_assert::assert_balance_conserved;
///
/// // Verify transfer between Alice and Bob:
/// assert_balance_conserved!(
///     accounts: [&alice, &bob],
///     balance: |addr| token.balance(addr),
///     call: {
///         token.transfer(&alice, &bob, &100);
///     }
/// );
///
/// // Multi-party fee split with custom labels:
/// assert_balance_conserved!(
///     accounts: [(&alice, "Alice"), (&bob, "Bob"), (&treasury, "Treasury")],
///     balance: |addr| token.balance(addr),
///     call: {
///         swap_router.swap(&alice, &bob, &1000);
///     }
/// );
///
/// // Invariant with mint or burn accounting:
/// assert_balance_conserved!(
///     accounts: [&alice, &bob],
///     balance: |addr| token.balance(addr),
///     mint: 100,
///     burn: 0,
///     call: {
///         token.mint(&alice, &100);
///     }
/// );
/// ```
#[macro_export]
macro_rules! assert_balance_conserved {
    // accounts, balance, mint, burn, call: { ... }
    (
        accounts: $accounts:expr,
        balance: $balance_fn:expr,
        mint: $mint:expr,
        burn: $burn:expr,
        call: $call:block $(,)?
    ) => {{
        let tracked: ::std::vec::Vec<$crate::balance::TrackedAccount> = $accounts
            .into_iter()
            .map($crate::balance::IntoTrackedAccount::into_tracked_account)
            .collect();
        $crate::balance::assert_balance_conserved_internal(
            tracked,
            $balance_fn,
            $mint,
            $burn,
            || $call,
        )
    }};

    // accounts, balance, burn, mint, call: { ... }
    (
        accounts: $accounts:expr,
        balance: $balance_fn:expr,
        burn: $burn:expr,
        mint: $mint:expr,
        call: $call:block $(,)?
    ) => {{
        $crate::assert_balance_conserved!(
            accounts: $accounts,
            balance: $balance_fn,
            mint: $mint,
            burn: $burn,
            call: $call,
        )
    }};

    // accounts, balance, mint, call: { ... }
    (
        accounts: $accounts:expr,
        balance: $balance_fn:expr,
        mint: $mint:expr,
        call: $call:block $(,)?
    ) => {{
        $crate::assert_balance_conserved!(
            accounts: $accounts,
            balance: $balance_fn,
            mint: $mint,
            burn: 0i128,
            call: $call,
        )
    }};

    // accounts, balance, burn, call: { ... }
    (
        accounts: $accounts:expr,
        balance: $balance_fn:expr,
        burn: $burn:expr,
        call: $call:block $(,)?
    ) => {{
        $crate::assert_balance_conserved!(
            accounts: $accounts,
            balance: $balance_fn,
            mint: 0i128,
            burn: $burn,
            call: $call,
        )
    }};

    // accounts, balance, call: { ... }
    (
        accounts: $accounts:expr,
        balance: $balance_fn:expr,
        call: $call:block $(,)?
    ) => {{
        $crate::assert_balance_conserved!(
            accounts: $accounts,
            balance: $balance_fn,
            mint: 0i128,
            burn: 0i128,
            call: $call,
        )
    }};

    // accounts, balance, { ... }
    (
        accounts: $accounts:expr,
        balance: $balance_fn:expr,
        $call:block $(,)?
    ) => {{
        $crate::assert_balance_conserved!(
            accounts: $accounts,
            balance: $balance_fn,
            mint: 0i128,
            burn: 0i128,
            call: $call,
        )
    }};

    // accounts, balance, mint, burn, { ... }
    (
        accounts: $accounts:expr,
        balance: $balance_fn:expr,
        mint: $mint:expr,
        burn: $burn:expr,
        $call:block $(,)?
    ) => {{
        $crate::assert_balance_conserved!(
            accounts: $accounts,
            balance: $balance_fn,
            mint: $mint,
            burn: $burn,
            call: $call,
        )
    }};
}

#[cfg(test)]
mod tests {
    use soroban_sdk::{testutils::Address as _, Address, Env};
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[test]
    fn test_balanced_two_party_transfer() {
        let env = Env::default();
        let alice = Address::generate(&env);
        let bob = Address::generate(&env);

        let balances = RefCell::new(BTreeMap::new());
        balances.borrow_mut().insert(alice.clone(), 1000i128);
        balances.borrow_mut().insert(bob.clone(), 200i128);

        let result = assert_balance_conserved!(
            accounts: [(&alice, "Alice"), (&bob, "Bob")],
            balance: |addr| *balances.borrow().get(addr).unwrap_or(&0),
            call: {
                *balances.borrow_mut().get_mut(&alice).unwrap() -= 150;
                *balances.borrow_mut().get_mut(&bob).unwrap() += 150;
                "transfer_ok"
            }
        );

        assert_eq!(result, "transfer_ok");
        assert_eq!(*balances.borrow().get(&alice).unwrap(), 850);
        assert_eq!(*balances.borrow().get(&bob).unwrap(), 350);
    }

    #[test]
    fn test_fee_split_multi_party_conservation() {
        let env = Env::default();
        let user = Address::generate(&env);
        let recipient = Address::generate(&env);
        let treasury = Address::generate(&env);
        let pool = Address::generate(&env);

        let balances = RefCell::new(BTreeMap::new());
        balances.borrow_mut().insert(user.clone(), 10_000i128);
        balances.borrow_mut().insert(recipient.clone(), 0i128);
        balances.borrow_mut().insert(treasury.clone(), 500i128);
        balances.borrow_mut().insert(pool.clone(), 25_000i128);

        assert_balance_conserved!(
            accounts: [
                (&user, "User"),
                (&recipient, "Recipient"),
                (&treasury, "Treasury"),
                (&pool, "Pool Reserve")
            ],
            balance: |addr| *balances.borrow().get(addr).unwrap_or(&0),
            call: {
                // User pays 1000: 970 to recipient, 20 to pool, 10 to treasury
                *balances.borrow_mut().get_mut(&user).unwrap() -= 1000;
                *balances.borrow_mut().get_mut(&recipient).unwrap() += 970;
                *balances.borrow_mut().get_mut(&pool).unwrap() += 20;
                *balances.borrow_mut().get_mut(&treasury).unwrap() += 10;
            }
        );
    }

    #[test]
    fn test_external_mint_and_burn_accounting() {
        let env = Env::default();
        let alice = Address::generate(&env);
        let vault = Address::generate(&env);

        let balances = RefCell::new(BTreeMap::new());
        balances.borrow_mut().insert(alice.clone(), 500i128);
        balances.borrow_mut().insert(vault.clone(), 1000i128);

        // Explicit mint of 300 to alice and burn of 100 from vault
        assert_balance_conserved!(
            accounts: [&alice, &vault],
            balance: |addr| *balances.borrow().get(addr).unwrap_or(&0),
            mint: 300,
            burn: 100,
            call: {
                *balances.borrow_mut().get_mut(&alice).unwrap() += 300;
                *balances.borrow_mut().get_mut(&vault).unwrap() -= 100;
            }
        );
    }

    #[test]
    #[should_panic(expected = "Balance conservation invariant breached!")]
    fn test_fractional_rounding_leak_detected() {
        let env = Env::default();
        let alice = Address::generate(&env);
        let bob = Address::generate(&env);
        let fee_recipient = Address::generate(&env);

        let balances = RefCell::new(BTreeMap::new());
        balances.borrow_mut().insert(alice.clone(), 1000i128);
        balances.borrow_mut().insert(bob.clone(), 0i128);
        balances.borrow_mut().insert(fee_recipient.clone(), 0i128);

        // Flawed transfer: Alice is debited 100, Bob gets 97, fee recipient gets 2 (1 token leaked due to rounding bug)
        assert_balance_conserved!(
            accounts: [
                (&alice, "Alice"),
                (&bob, "Bob"),
                (&fee_recipient, "Fee Collector")
            ],
            balance: |addr| *balances.borrow().get(addr).unwrap_or(&0),
            call: {
                *balances.borrow_mut().get_mut(&alice).unwrap() -= 100;
                *balances.borrow_mut().get_mut(&bob).unwrap() += 97;
                *balances.borrow_mut().get_mut(&fee_recipient).unwrap() += 2;
            }
        );
    }

    #[test]
    #[should_panic(expected = "Balance conservation invariant breached!")]
    fn test_unexpected_mint_leak_fails() {
        let env = Env::default();
        let alice = Address::generate(&env);

        let balances = RefCell::new(BTreeMap::new());
        balances.borrow_mut().insert(alice.clone(), 100i128);

        // Alice's balance increased with no declared mint
        assert_balance_conserved!(
            accounts: [&alice],
            balance: |addr| *balances.borrow().get(addr).unwrap_or(&0),
            call: {
                *balances.borrow_mut().get_mut(&alice).unwrap() += 10;
            }
        );
    }
}
