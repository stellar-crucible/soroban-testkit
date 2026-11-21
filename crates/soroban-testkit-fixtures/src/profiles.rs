//! Network environment profiles for `TestContextBuilder`.
//!
//! Soroban's test environment is unmetered by default, which can hide problems
//! that only appear on a real network: transactions that exceed the CPU/memory
//! budget, run out of ledger entry capacity, or pay rent. A [`NetworkProfile`]
//! configures the host budget and ledger parameters so tests fail the same way
//! they would on Testnet or Mainnet.

use soroban_env_host::InvocationResourceLimits;
use soroban_sdk::testutils::cost_estimate::NetworkInvocationResourceLimits;
use soroban_sdk::testutils::Ledger as _;
use soroban_sdk::Env;

/// CPU instruction ceiling used by Stellar Testnet.
pub const TESTNET_CPU_INSTRUCTIONS: i64 = 100_000_000;
/// Memory ceiling used by Stellar Testnet (40 MiB).
pub const TESTNET_MEM_BYTES: i64 = 40 * 1024 * 1024;
/// CPU instruction ceiling used by Stellar Mainnet.
pub const MAINNET_CPU_INSTRUCTIONS: i64 = 400_000_000;
/// Memory ceiling used by Stellar Mainnet (40 MiB).
pub const MAINNET_MEM_BYTES: i64 = 40 * 1024 * 1024;

/// Base reserve on Stellar Mainnet/Testnet, in stroops (0.5 XLM).
pub const NETWORK_BASE_RESERVE_STROOPS: u32 = 5_000_000;
/// Minimum temporary entry TTL, in ledgers.
pub const NETWORK_MIN_TEMP_ENTRY_TTL: u32 = 16;
/// Minimum persistent entry TTL, in ledgers.
pub const NETWORK_MIN_PERSISTENT_ENTRY_TTL: u32 = 120;
/// Maximum entry TTL, in ledgers.
pub const NETWORK_MAX_ENTRY_TTL: u32 = 6_312_000;

/// The network conditions a test context should emulate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NetworkProfile {
    /// Unmetered local testing with every authorization mocked.
    #[default]
    Local,
    /// Testnet budget ceilings: 100M CPU instructions and 40 MiB of memory.
    Testnet,
    /// Mainnet budget ceilings, base reserves, and rent-bearing ledger limits.
    Mainnet,
}

impl NetworkProfile {
    /// Human-readable profile name.
    pub fn name(self) -> &'static str {
        match self {
            NetworkProfile::Local => "local",
            NetworkProfile::Testnet => "testnet",
            NetworkProfile::Mainnet => "mainnet",
        }
    }

    /// Whether this profile enforces resource limits during invocations.
    pub fn is_metered(self) -> bool {
        !matches!(self, NetworkProfile::Local)
    }

    /// CPU instruction limit, or `None` for the unmetered local profile.
    pub fn cpu_instructions(self) -> Option<i64> {
        match self {
            NetworkProfile::Local => None,
            NetworkProfile::Testnet => Some(TESTNET_CPU_INSTRUCTIONS),
            NetworkProfile::Mainnet => Some(MAINNET_CPU_INSTRUCTIONS),
        }
    }

    /// Memory limit in bytes, or `None` for the unmetered local profile.
    pub fn mem_bytes(self) -> Option<i64> {
        match self {
            NetworkProfile::Local => None,
            NetworkProfile::Testnet => Some(TESTNET_MEM_BYTES),
            NetworkProfile::Mainnet => Some(MAINNET_MEM_BYTES),
        }
    }

    /// Ledger base reserve in stroops.
    pub fn base_reserve(self) -> u32 {
        match self {
            NetworkProfile::Local => 0,
            NetworkProfile::Testnet | NetworkProfile::Mainnet => NETWORK_BASE_RESERVE_STROOPS,
        }
    }

    /// Persistent storage rent rate, in stroops per 1 KiB per ledger.
    ///
    /// These are the rates used by [`CostEstimate::fee`] when estimating the
    /// cost of the last invocation.
    ///
    /// [`CostEstimate::fee`]: soroban_sdk::testutils::cost_estimate::CostEstimate::fee
    pub fn rent_fee_per_kib(self) -> u64 {
        match self {
            NetworkProfile::Local => 0,
            NetworkProfile::Testnet => 1_000,
            NetworkProfile::Mainnet => 12_000,
        }
    }

    /// The invocation resource limits enforced by this profile.
    pub fn invocation_limits(self) -> Option<InvocationResourceLimits> {
        match self {
            NetworkProfile::Local => None,
            NetworkProfile::Testnet => {
                let mut limits = InvocationResourceLimits::mainnet();
                limits.instructions = TESTNET_CPU_INSTRUCTIONS;
                limits.mem_bytes = TESTNET_MEM_BYTES;
                Some(limits)
            }
            NetworkProfile::Mainnet => Some(InvocationResourceLimits::mainnet()),
        }
    }

    /// Apply the profile to an environment.
    ///
    /// Local profiles disable invocation metering and make the budget
    /// unlimited. Metered profiles install the profile's ledger parameters and
    /// enforce its CPU, memory and ledger-entry limits. An invocation that
    /// exceeds a limit panics with a message describing the exceeded resource.
    pub fn apply(self, env: &Env) {
        let ledger = env.ledger();
        ledger.set_base_reserve(self.base_reserve());
        ledger.set_min_temp_entry_ttl(NETWORK_MIN_TEMP_ENTRY_TTL);
        ledger.set_min_persistent_entry_ttl(NETWORK_MIN_PERSISTENT_ENTRY_TTL);
        ledger.set_max_entry_ttl(NETWORK_MAX_ENTRY_TTL);

        match self.invocation_limits() {
            Some(limits) => env.cost_estimate().enforce_resource_limits(limits),
            None => env.cost_estimate().disable_resource_limits(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl, Env};

    #[contract]
    pub struct HeavyContract;

    #[contractimpl]
    impl HeavyContract {
        pub fn write_many(env: Env, count: u32) {
            for i in 0..count {
                env.storage().persistent().set(&i, &i);
            }
        }
    }

    fn remaining_cpu(profile: NetworkProfile) -> (u64, u64) {
        let env = Env::default();
        profile.apply(&env);
        let budget = env.host().budget_cloned();
        (
            budget.get_cpu_insns_remaining().unwrap(),
            budget.get_mem_bytes_remaining().unwrap(),
        )
    }

    #[test]
    fn local_profile_is_unmetered() {
        let profile = NetworkProfile::Local;
        assert!(!profile.is_metered());
        assert_eq!(profile.cpu_instructions(), None);
        assert_eq!(profile.mem_bytes(), None);
        let (cpu, mem) = remaining_cpu(profile);
        assert_eq!(cpu, u64::MAX);
        assert_eq!(mem, u64::MAX);
    }

    #[test]
    fn testnet_profile_sets_testnet_limits() {
        let profile = NetworkProfile::Testnet;
        assert!(profile.is_metered());
        let (cpu, mem) = remaining_cpu(profile);
        assert_eq!(cpu, TESTNET_CPU_INSTRUCTIONS as u64);
        assert_eq!(mem, TESTNET_MEM_BYTES as u64);
    }

    #[test]
    fn mainnet_profile_sets_mainnet_limits() {
        let profile = NetworkProfile::Mainnet;
        assert!(profile.is_metered());
        let (cpu, mem) = remaining_cpu(profile);
        assert_eq!(cpu, MAINNET_CPU_INSTRUCTIONS as u64);
        assert_eq!(mem, MAINNET_MEM_BYTES as u64);
    }

    #[test]
    fn metered_profiles_install_ledger_parameters() {
        for profile in [NetworkProfile::Testnet, NetworkProfile::Mainnet] {
            let env = Env::default();
            profile.apply(&env);
            let ledger = env.ledger().get();
            assert_eq!(ledger.base_reserve, NETWORK_BASE_RESERVE_STROOPS);
            assert_eq!(
                ledger.min_persistent_entry_ttl,
                NETWORK_MIN_PERSISTENT_ENTRY_TTL
            );
            assert_eq!(ledger.min_temp_entry_ttl, NETWORK_MIN_TEMP_ENTRY_TTL);
            assert!(profile.rent_fee_per_kib() > 0);
        }
    }

    #[test]
    fn local_profile_allows_heavy_calls() {
        let env = Env::default();
        NetworkProfile::Local.apply(&env);
        env.mock_all_auths();
        let id = env.register(HeavyContract, ());
        let client = HeavyContractClient::new(&env, &id);
        client.write_many(&300);
    }

    #[test]
    #[should_panic]
    fn mainnet_profile_rejects_heavy_calls() {
        let env = Env::default();
        NetworkProfile::Mainnet.apply(&env);
        env.mock_all_auths();
        let id = env.register(HeavyContract, ());
        let client = HeavyContractClient::new(&env, &id);
        client.write_many(&300);
    }
}
