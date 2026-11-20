//! Liquidity-conservation harness for token-moving contracts (issue #111).
//!
//! The invariant: across a set of tracked holders (users, pools, vaults, fee
//! collectors), the sum of balances changes by exactly what was minted minus
//! what was burned. Any other difference is either a leak (value left the
//! tracked set or was destroyed) or phantom minting (value appeared from
//! nowhere) — and the check is exact, so a single stroop is a violation.
//!
//! Balances are read from the token contract itself (`balance(id)`), before
//! and after the workflow under test. Supply changes cannot be read the same
//! way, because the SEP-41 token interface exposes no total supply, so the test
//! records the mints and burns it intends ([`ConservationVerifier::record_mint`],
//! [`ConservationVerifier::record_burn`]). Anything the contract mints or burns
//! that the test did not declare shows up as a discrepancy, which is the point.
//!
//! ```ignore
//! let verifier = ConservationVerifier::new(&env, &token_id);
//! verifier.track_accounts(&[user1.clone(), user2.clone(), pool.clone()]);
//!
//! // ... run a multi-step workflow ...
//!
//! verifier.assert_conserved(); // panics with a reconciliation table on a leak
//! ```

use soroban_sdk::{token::TokenClient, Address, Env};
use std::cell::RefCell;
use std::fmt;

/// One tracked holder's line in the reconciliation table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationRow {
    pub account: Address,
    pub opening: i128,
    pub closing: i128,
}

impl ReconciliationRow {
    /// Credit (positive) or debit (negative) over the workflow.
    pub fn delta(&self) -> i128 {
        self.closing - self.opening
    }
}

/// The full accounting for one check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConservationReport {
    pub rows: Vec<ReconciliationRow>,
    /// Supply the test declared as minted since tracking started.
    pub minted: i128,
    /// Supply the test declared as burned since tracking started.
    pub burned: i128,
}

impl ConservationReport {
    /// Net change across every tracked balance.
    pub fn balance_delta(&self) -> i128 {
        self.rows.iter().map(ReconciliationRow::delta).sum()
    }

    /// What the tracked balances should have moved by: minted minus burned.
    pub fn expected_delta(&self) -> i128 {
        self.minted - self.burned
    }

    /// `balance_delta - expected_delta`. Negative is a leak, positive is
    /// phantom minting; zero is conserved.
    pub fn discrepancy(&self) -> i128 {
        self.balance_delta() - self.expected_delta()
    }

    pub fn is_conserved(&self) -> bool {
        self.discrepancy() == 0
    }

    /// Sum of all positive deltas.
    pub fn total_credits(&self) -> i128 {
        self.rows.iter().map(|r| r.delta().max(0)).sum()
    }

    /// Sum of all negative deltas, as a positive number.
    pub fn total_debits(&self) -> i128 {
        self.rows.iter().map(|r| (-r.delta()).max(0)).sum()
    }
}

impl fmt::Display for ConservationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let accounts: Vec<String> = self.rows.iter().map(|r| short(&r.account)).collect();
        let width = accounts.iter().map(String::len).max().unwrap_or(7).max(7);
        writeln!(
            f,
            "{:<width$} | {:>20} | {:>20} | {:>20} | {:>20}",
            "account", "opening", "closing", "credit", "debit"
        )?;
        writeln!(f, "{}", "-".repeat(width + 4 * 23))?;
        for (row, name) in self.rows.iter().zip(&accounts) {
            let delta = row.delta();
            writeln!(
                f,
                "{:<width$} | {:>20} | {:>20} | {:>20} | {:>20}",
                name,
                row.opening,
                row.closing,
                if delta > 0 {
                    delta.to_string()
                } else {
                    String::new()
                },
                if delta < 0 {
                    (-delta).to_string()
                } else {
                    String::new()
                },
            )?;
        }
        writeln!(f, "{}", "-".repeat(width + 4 * 23))?;
        writeln!(
            f,
            "{:<width$} | {:>20} | {:>20} | {:>20} | {:>20}",
            "total",
            self.rows.iter().map(|r| r.opening).sum::<i128>(),
            self.rows.iter().map(|r| r.closing).sum::<i128>(),
            self.total_credits(),
            self.total_debits(),
        )?;
        writeln!(f)?;
        writeln!(f, "net balance delta : {}", self.balance_delta())?;
        writeln!(
            f,
            "expected delta    : {} (minted {} - burned {})",
            self.expected_delta(),
            self.minted,
            self.burned
        )?;
        let d = self.discrepancy();
        let verdict = match d {
            0 => "conserved".to_string(),
            d if d < 0 => format!("LEAK: {} stroop(s) left the tracked set", -d),
            d => format!("PHANTOM MINT: {d} stroop(s) appeared in the tracked set"),
        };
        write!(f, "discrepancy       : {d} — {verdict}")
    }
}

fn short(address: &Address) -> String {
    let s = format!("{}", address.to_string());
    if s.len() > 12 {
        format!("{}…{}", &s[..6], &s[s.len() - 4..])
    } else {
        s
    }
}

/// Tracks token balances across a workflow and checks they are conserved.
pub struct ConservationVerifier {
    env: Env,
    token: Address,
    state: RefCell<State>,
}

#[derive(Default)]
struct State {
    opening: Vec<(Address, i128)>,
    minted: i128,
    burned: i128,
}

impl ConservationVerifier {
    /// Verify conservation of `token` on `env`.
    pub fn new(env: &Env, token: &Address) -> Self {
        Self {
            env: env.clone(),
            token: token.clone(),
            state: RefCell::new(State::default()),
        }
    }

    /// Start tracking `accounts`, recording their current balances as the
    /// opening position. Accounts already tracked keep their opening balance.
    pub fn track_accounts(&self, accounts: &[Address]) {
        let client = TokenClient::new(&self.env, &self.token);
        let mut state = self.state.borrow_mut();
        for account in accounts {
            if state.opening.iter().any(|(a, _)| a == account) {
                continue;
            }
            state
                .opening
                .push((account.clone(), client.balance(account)));
        }
    }

    /// Declare supply the workflow is expected to mint into the tracked set.
    pub fn record_mint(&self, amount: i128) {
        self.state.borrow_mut().minted += amount;
    }

    /// Declare supply the workflow is expected to burn from the tracked set.
    pub fn record_burn(&self, amount: i128) {
        self.state.borrow_mut().burned += amount;
    }

    /// Re-read every tracked balance and build the reconciliation.
    pub fn report(&self) -> ConservationReport {
        let client = TokenClient::new(&self.env, &self.token);
        let state = self.state.borrow();
        let rows = state
            .opening
            .iter()
            .map(|(account, opening)| ReconciliationRow {
                account: account.clone(),
                opening: *opening,
                closing: client.balance(account),
            })
            .collect();
        ConservationReport {
            rows,
            minted: state.minted,
            burned: state.burned,
        }
    }

    /// The report when conserved, or the report of the violation.
    pub fn check(&self) -> Result<ConservationReport, ConservationReport> {
        let report = self.report();
        if report.is_conserved() {
            Ok(report)
        } else {
            Err(report)
        }
    }

    /// Panic with the reconciliation table unless balances are conserved.
    #[track_caller]
    pub fn assert_conserved(&self) -> ConservationReport {
        match self.check() {
            Ok(report) => report,
            Err(report) => panic!("conservation invariant violated:\n{report}"),
        }
    }

    /// Make the current balances the new opening position and reset the
    /// declared mints and burns, to check the next step on its own.
    pub fn checkpoint(&self) {
        let client = TokenClient::new(&self.env, &self.token);
        let mut state = self.state.borrow_mut();
        for (account, opening) in state.opening.iter_mut() {
            *opening = client.balance(account);
        }
        state.minted = 0;
        state.burned = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::token::StellarAssetClient;
    use soroban_sdk::{contract, contractimpl};

    /// Pays `amount` from `from` out to three recipients in equal integer
    /// shares. `burn_dust` decides what happens to the rounding remainder:
    /// kept by the splitter (conserved, if the splitter is tracked) or burned
    /// (a deliberate rounding leak).
    #[contract]
    struct Splitter;

    #[contractimpl]
    impl Splitter {
        pub fn split(
            env: Env,
            token: Address,
            from: Address,
            to: soroban_sdk::Vec<Address>,
            amount: i128,
            burn_dust: bool,
        ) {
            from.require_auth();
            let client = TokenClient::new(&env, &token);
            let me = env.current_contract_address();
            client.transfer(&from, &me, &amount);
            let share = amount / to.len() as i128;
            for recipient in to.iter() {
                client.transfer(&me, &recipient, &share);
            }
            let dust = amount - share * to.len() as i128;
            if burn_dust && dust > 0 {
                client.burn(&me, &dust);
            }
        }
    }

    struct Setup {
        env: Env,
        token: Address,
        user: Address,
        recipients: [Address; 3],
        splitter: Address,
    }

    fn setup() -> Setup {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let token = env
            .register_stellar_asset_contract_v2(admin)
            .address()
            .clone();
        let user = Address::generate(&env);
        StellarAssetClient::new(&env, &token).mint(&user, &1_000);
        let recipients = [
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
        ];
        let splitter = env.register(Splitter, ());
        Setup {
            env,
            token,
            user,
            recipients,
            splitter,
        }
    }

    fn tracked(s: &Setup) -> Vec<Address> {
        let mut all = vec![s.user.clone(), s.splitter.clone()];
        all.extend(s.recipients.iter().cloned());
        all
    }

    fn split(s: &Setup, amount: i128, burn_dust: bool) {
        SplitterClient::new(&s.env, &s.splitter).split(
            &s.token,
            &s.user,
            &soroban_sdk::Vec::from_array(&s.env, s.recipients.clone()),
            &amount,
            &burn_dust,
        );
    }

    #[test]
    fn multi_step_workflow_is_conserved_when_dust_is_kept() {
        let s = setup();
        let verifier = ConservationVerifier::new(&s.env, &s.token);
        verifier.track_accounts(&tracked(&s));
        split(&s, 100, false);
        split(&s, 250, false);
        split(&s, 7, false);
        let report = verifier.assert_conserved();
        assert_eq!(report.discrepancy(), 0);
        assert_eq!(report.total_debits(), 357);
    }

    #[test]
    fn detects_a_one_stroop_rounding_leak() {
        let s = setup();
        let verifier = ConservationVerifier::new(&s.env, &s.token);
        verifier.track_accounts(&tracked(&s));
        split(&s, 100, true); // 100 / 3 = 33 each, 1 stroop of dust burned
        let report = verifier.check().unwrap_err();
        assert_eq!(report.discrepancy(), -1);
        let table = report.to_string();
        assert!(table.contains("LEAK: 1 stroop(s)"), "{table}");
        assert!(
            table.contains("opening") && table.contains("credit"),
            "{table}"
        );
    }

    #[test]
    #[should_panic(expected = "conservation invariant violated")]
    fn assert_conserved_panics_with_the_table() {
        let s = setup();
        let verifier = ConservationVerifier::new(&s.env, &s.token);
        verifier.track_accounts(&tracked(&s));
        split(&s, 101, true); // 2 stroops burned
        verifier.assert_conserved();
    }

    #[test]
    fn a_leak_out_of_the_tracked_set_is_caught() {
        let s = setup();
        let verifier = ConservationVerifier::new(&s.env, &s.token);
        // The splitter is not tracked, so dust it keeps has left the set.
        let mut accounts = vec![s.user.clone()];
        accounts.extend(s.recipients.iter().cloned());
        verifier.track_accounts(&accounts);
        split(&s, 100, false);
        assert_eq!(verifier.report().discrepancy(), -1);
    }

    #[test]
    fn declared_mints_and_burns_are_accounted_for() {
        let s = setup();
        let verifier = ConservationVerifier::new(&s.env, &s.token);
        verifier.track_accounts(&tracked(&s));
        StellarAssetClient::new(&s.env, &s.token).mint(&s.recipients[0], &500);
        verifier.record_mint(500);
        TokenClient::new(&s.env, &s.token).burn(&s.user, &40);
        verifier.record_burn(40);
        verifier.assert_conserved();
    }

    #[test]
    fn an_undeclared_mint_is_phantom_minting() {
        let s = setup();
        let verifier = ConservationVerifier::new(&s.env, &s.token);
        verifier.track_accounts(&tracked(&s));
        StellarAssetClient::new(&s.env, &s.token).mint(&s.user, &1);
        let report = verifier.check().unwrap_err();
        assert_eq!(report.discrepancy(), 1);
        assert!(report.to_string().contains("PHANTOM MINT: 1 stroop(s)"));
    }

    #[test]
    fn checkpoint_scopes_the_next_check() {
        let s = setup();
        let verifier = ConservationVerifier::new(&s.env, &s.token);
        verifier.track_accounts(&tracked(&s));
        split(&s, 100, true);
        assert!(verifier.check().is_err());
        verifier.checkpoint();
        split(&s, 99, true); // divides evenly, no dust burned
        verifier.assert_conserved();
    }
}
