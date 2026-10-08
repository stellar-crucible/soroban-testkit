use soroban_env_host::budget::Budget;

pub struct BudgetSnapshot {
    pub cpu_insns: u64,
    pub mem_bytes: u64,
}

impl BudgetSnapshot {
    pub fn capture(budget: &Budget) -> Self {
        Self {
            cpu_insns: budget.get_cpu_insns_consumed().unwrap_or(0),
            mem_bytes: budget.get_mem_bytes_consumed().unwrap_or(0),
        }
    }

    pub fn diff(&self, other: &BudgetSnapshot) -> BudgetSnapshot {
        BudgetSnapshot {
            cpu_insns: other.cpu_insns.saturating_sub(self.cpu_insns),
            mem_bytes: other.mem_bytes.saturating_sub(self.mem_bytes),
        }
    }
}
