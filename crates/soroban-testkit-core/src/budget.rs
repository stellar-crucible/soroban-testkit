use soroban_env_host::budget::Budget;

/// Anything Testkit can read consumed CPU and memory from.
///
/// Implemented for the host [`Budget`] and for the SDK budget returned by
/// `env.cost_estimate().budget()`, so a snapshot can be captured from either.
pub trait BudgetRead {
    fn cpu_insns(&self) -> u64;
    fn mem_bytes(&self) -> u64;
}

impl BudgetRead for Budget {
    fn cpu_insns(&self) -> u64 {
        self.get_cpu_insns_consumed().unwrap_or(0)
    }

    fn mem_bytes(&self) -> u64 {
        self.get_mem_bytes_consumed().unwrap_or(0)
    }
}

impl BudgetRead for soroban_sdk::testutils::budget::Budget {
    fn cpu_insns(&self) -> u64 {
        self.cpu_instruction_cost()
    }

    fn mem_bytes(&self) -> u64 {
        self.memory_bytes_cost()
    }
}

pub struct BudgetSnapshot {
    pub cpu_insns: u64,
    pub mem_bytes: u64,
}

impl BudgetSnapshot {
    pub fn capture(budget: &impl BudgetRead) -> Self {
        Self {
            cpu_insns: budget.cpu_insns(),
            mem_bytes: budget.mem_bytes(),
        }
    }

    pub fn diff(&self, other: &BudgetSnapshot) -> BudgetSnapshot {
        BudgetSnapshot {
            cpu_insns: other.cpu_insns.saturating_sub(self.cpu_insns),
            mem_bytes: other.mem_bytes.saturating_sub(self.mem_bytes),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BudgetSnapshot;
    use soroban_env_host::{budget::Budget, xdr::ContractCostType};

    fn charged_budget() -> Budget {
        let budget = Budget::default();
        budget.charge(ContractCostType::WasmInsnExec, None).unwrap();
        budget
    }

    #[test]
    fn capture_accepts_the_sdk_budget_of_an_untouched_environment() {
        let env = soroban_sdk::Env::default();
        let snapshot = BudgetSnapshot::capture(&env.cost_estimate().budget());

        // An environment that has not run a contract call estimates nothing;
        // examples/counter covers the case of a call that consumes budget.
        assert_eq!(snapshot.cpu_insns, 0);
        assert_eq!(snapshot.mem_bytes, 0);
    }

    #[test]
    fn capture_on_idle_budget_reports_zero() {
        let snapshot = BudgetSnapshot::capture(&Budget::default());
        assert_eq!(snapshot.cpu_insns, 0);
        assert_eq!(snapshot.mem_bytes, 0);
    }

    #[test]
    fn capture_reflects_consumed_resources() {
        let snapshot = BudgetSnapshot::capture(&charged_budget());
        assert!(
            snapshot.cpu_insns > 0,
            "charging the budget must register CPU consumption"
        );
    }

    #[test]
    fn diff_is_monotonic_between_captures() {
        let budget = Budget::default();
        let before = BudgetSnapshot::capture(&budget);
        for _ in 0..5 {
            budget.charge(ContractCostType::WasmInsnExec, None).unwrap();
        }
        let after = BudgetSnapshot::capture(&budget);

        let delta = before.diff(&after);
        assert_eq!(delta.cpu_insns, after.cpu_insns - before.cpu_insns);
        assert!(delta.cpu_insns > 0);
    }

    #[test]
    fn diff_saturates_when_the_later_snapshot_is_smaller() {
        let before = BudgetSnapshot {
            cpu_insns: 900,
            mem_bytes: 400,
        };
        let after = BudgetSnapshot {
            cpu_insns: 100,
            mem_bytes: 50,
        };

        let delta = before.diff(&after);
        assert_eq!(delta.cpu_insns, 0);
        assert_eq!(delta.mem_bytes, 0);
    }

    #[test]
    fn diff_of_identical_snapshots_is_zero() {
        let snapshot = BudgetSnapshot::capture(&charged_budget());
        let delta = snapshot.diff(&snapshot);
        assert_eq!(delta.cpu_insns, 0);
        assert_eq!(delta.mem_bytes, 0);
    }
}
