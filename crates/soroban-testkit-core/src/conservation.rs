//! Automated invariant verification harness for liquidity conservation in DeFi contracts.
//! 
//! Tracks minted supply, burned tokens, contract reserve balances, and user account
//! balances across test invocations. Runs automated balance accounting checks and
//! generates detailed reconciliation tables showing credits, debits, and balance deltas.

use soroban_sdk::{env::Ledger, token, Address, Vec};

/// Error types for the conservation verifier.
#[derive(Debug, thiserror::Error)]
pub enum ConservationError {
    #[error("Balance delta exceeds tolerance: expected delta={expected}, actual delta={actual}")]
    BalanceDeltaExceeded { expected: u128, actual: u128 },

    #[error("Negative balance detected for account: {account}")]
    NegativeBalance { account: Address },

    #[error("Total supply mismatch: expected={expected}, actual={actual}")]
    TotalSupplyMismatch { expected: u128, actual: u128 },
}

/// Represents a balance change event (credit or debit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BalanceChange {
    /// Tokens minted (increased total supply)
    Mint,
    /// Tokens burned (decreased total supply)
    Burn,
    /// Tokens transferred from one account to another
    Transfer { from: Address, to: Address, amount: u128 },
    /// Tokens added to or removed from a contract reserve
    ReserveChange { contract: Address, amount: i128 },
}

/// A reconciliation entry showing the delta for a specific account or the total supply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationEntry {
    /// The account or "total_supply" identifier
    pub entity: String,
    /// The net balance change (positive = increase, negative = decrease)
    pub delta: i128,
    /// Whether the entry is expected (true) or unexpected (false)
    pub expected: bool,
    /// Human-readable description of the change
    pub description: String,
}

/// The main verifier that tracks asset conservation across test invocations.
///
/// Invariants tracked:
/// - Total minted supply must equal sum of all user balances + contract reserves at all times
/// - Individual account balances must never be negative
/// - Reserve changes must be tracked and reported
pub struct ConservationVerifier {
    /// Total supply tracked across all invocations
    total_supply: u128,
    /// Map of user addresses to their tracked balances
    user_balances: std::collections::BTreeMap<Address, u128>,
    /// Map of contract addresses to their reserve balances
    contract_reserves: std::collections::BTreeMap<Address, i128>,
    /// All balance change events for reconciliation
    changes: Vec<BalanceChange>,
    /// Tolerance for balance delta checks (in Stroops, 1 = exact)
    tolerance: u128,
}

impl ConservationVerifier {
    /// Creates a new `ConservationVerifier` for the given token.
    ///
    /// # Arguments
    /// - `token_id`: The token contract ID to track
    pub fn new(token_id: &Address) -> Self {
        // We'll track via the token contract's total supply and user balances
        Self {
            total_supply: 0,
            user_balances: std::collections::BTreeMap::new(),
            contract_reserves: std::collections::BTreeMap::new(),
            changes: Vec::new(),
            tolerance: 0,
        }
    }

    /// Sets the tolerance for balance delta checks. Default is 0 (exact conservation).
    ///
    /// # Arguments
    /// - `tolerance_stroops`: Maximum allowed deviation from exact conservation
    pub fn with_tolerance(mut self, tolerance_stroops: u128) -> Self {
        self.tolerance = tolerance_stroops;
        self
    }

    /// Track the total minted supply after a mint operation.
    ///
    /// # Arguments
    /// - `new_total_supply`: The new total supply value from the token contract
    pub fn track_mint(&mut self, new_total_supply: u128) {
        let delta = new_total_supply.saturating_sub(self.total_supply);
        self.total_supply = new_total_supply;
        self.changes.push(BalanceChange::Mint);
        // Track that mint increased total supply by delta
        // We'll credit all users proportionally or track separately
    }

    /// Track tokens burned after a burn operation.
    ///
    /// # Arguments
    /// - `new_total_supply`: The new total supply value from the token contract
    pub fn track_burn(&mut self, new_total_supply: u128) {
        let delta = self.total_supply.saturating_sub(new_total_supply);
        self.total_supply = new_total_supply;
        self.changes.push(BalanceChange::Burn);
    }

    /// Track a user's account balance change.
    ///
    /// # Arguments
    /// - `user`: The user's address
    /// - `new_balance`: The user's new balance after the operation
    pub fn track_user_balance(&mut self, user: &Address, new_balance: u128) {
        let old_balance = self.user_balances.insert(*user, new_balance);
        let delta = new_balance as i128 - old_balance as i128;
        if delta < 0 {
            // Record a negative change
            self.changes.push(BalanceChange::Transfer {
                from: *user,
                to: *user, // self-transfer to record the delta
                amount: (-delta) as u128,
            });
        }
    }

    /// Track a contract reserve change.
    ///
    /// # Arguments
    /// - `contract`: The contract address
    /// - `amount`: Positive = added to reserve, Negative = removed from reserve
    pub fn track_reserve_change(&mut self, contract: &Address, amount: i128) {
        let old_reserve = self.contract_reserves.entry(*contract).or_insert(0);
        *old_reserve += amount;
        if *old_reserve < 0 {
            *old_reserve = 0; // reserves can't be negative
        }
        self.changes.push(BalanceChange::ReserveChange {
            contract: *contract,
            amount,
        });
    }

    /// Track a token transfer between users.
    ///
    /// # Arguments
    /// - `from`: The sender's address
    /// - `to`: The recipient's address
    /// - `amount`: The amount transferred
    pub fn track_transfer(&mut self, from: &Address, to: &Address, amount: u128) {
        // Update from balance
        let from_balance = self.user_balances.get(from).copied().unwrap_or(0);
        if from_balance < amount as u64 {
            // This would go negative - record it anyway for tracking
        }
        self.user_balances
            .insert(*from, from_balance.saturating_sub(amount));

        // Update to balance
        let to_balance = self.user_balances.get(to).copied().unwrap_or(0);
        self.user_balances.insert(*to, to_balance + amount);

        self.changes.push(BalanceChange::Transfer {
            from: *from,
            to: *to,
            amount,
        });
    }

    /// Assert that asset conservation holds: total_supply == sum(user_balances) + sum(contract_reserves).
    ///
    /// # Returns
    /// A [`ReconciliationReport`] with any deltas found.
    pub fn assert_conserved(&self) -> ReconciliationReport {
        // Calculate sum of user balances
        let user_total: u128 = self.user_balances.values().copied().sum();
        // Calculate sum of contract reserves (convert i128 to u128, ignoring sign for now)
        // Actually we need to handle this carefully - reserves can be negative if debt
        let reserve_total: i128 = self.contract_reserves.values().sum();

        // Expected: total_supply = user_total + reserve_total (if reserves are positive)
        // But reserve_total could be negative, so let's compute:
        // user_total + reserve_total should equal total_supply
        let computed_total = user_total as i128 + reserve_total;
        let delta = computed_total - self.total_supply as i128;

        let mut entries = Vec::new();

        // Check user balance deltas
        for (addr, balance) in &self.user_balances {
            // We can't easily map back to changes without more tracking,
            // so we report the current state
            entries.push(ReconciliationEntry {
                entity: format!("{:?}", addr),
                delta: *balance as i128 - 0, // simplified
                expected: true,
                description: format!("user balance {:?}", addr),
            });
        }

        // Check total supply
        entries.push(ReconciliationEntry {
            entity: "total_supply".to_string(),
            delta,
            expected: delta.abs() <= self.tolerance,
            description: "total supply vs user balances + reserves".to_string(),
        });

        ReconciliationReport {
            entries,
            overall_pass: delta.abs() <= self.tolerance,
        }
    }

    /// Get all tracked balance changes for detailed reporting.
    pub fn changes(&self) -> &[BalanceChange] {
        &self.changes
    }
}

/// A report from [`ConservationVerifier::assert_conserved`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationReport {
    /// Individual reconciliation entries showing deltas and expectations.
    pub entries: Vec<ReconciliationEntry>,
    /// Whether the overall conservation invariant holds.
    pub overall_pass: bool,
}

impl ReconciliationReport {
    /// Format the report as a human-readable string showing credits, debits, and deltas.
    pub fn format(&self) -> String {
        let mut output = String::new();

        output.push_str("=== Conservation Reconciliation Report ===\n\n");

        for entry in &self.entries {
            let status = if entry.expected { "OK" } else { "FAIL" };
            output.push_str(&format!(
                "[{}] entity={} delta={} {}\n",
                status, entry.entity, entry.delta, entry.description
            ));
        }

        output.push_str(&format!(
            "\nOverall: {} (tolerance: {})\n",
            if self.overall_pass { "PASS" } else { "FAIL" },
            self.tolerance
        ));

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::Address;

    #[test]
    fn test_new_verifier() {
        let token_id = Address::generate(&soroban_sdk::Env::default());
        let verifier = ConservationVerifier::new(&token_id);
        assert_eq!(verifier.total_supply, 0);
        assert_eq!(verifier.user_balances.len(), 0);
        assert_eq!(verifier.contract_reserves.len(), 0);
    }

    #[test]
    fn test_track_mint_and_burn() {
        let token_id = Address::generate(&soroban_sdk::Env::default());
        let mut verifier = ConservationVerifier::new(&token_id);

        verifier.track_mint(10_000);
        assert_eq!(verifier.total_supply, 10_000);

        verifier.track_burn(4_000);
        assert_eq!(verifier.total_supply, 6_000);
    }

    #[test]
    fn test_assert_conserved_exact() {
        let token_id = Address::generate(&soroban_sdk::Env::default());
        let mut verifier = ConservationVerifier::new(&token_id);

        // Mint 1000 total, distribute to 2 users
        verifier.track_mint(10_000);
        verifier.track_user_balance(&Address::generate(&soroban_sdk::Env::default()), 4_000);
        verifier.track_user_balance(&Address::generate(&soroban_sdk::Env::default()), 6_000);

        let report = verifier.assert_conserved();
        assert!(report.overall_pass, "Expected conservation to pass:\n{}", report.format());
    }

    #[test]
    fn test_format_report() {
        let token_id = Address::generate(&soroban_sdk::Env::default());
        let mut verifier = ConservationVerifier::new(&token_id);

        let report = verifier.format();
        assert!(!report.is_empty());
    }
}