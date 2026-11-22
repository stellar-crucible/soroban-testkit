//! Deterministic contract deployment and CREATE2-style address prediction.
//!
//! Soroban derives a contract ID from the deployer address and a 32-byte salt.
//! [`DeterministicDeployer`] exposes that derivation before deployment so tests
//! can wire dependent contracts against a predicted address, then deploy and
//! assert the on-chain address matches.
//!
//! ```rust
//! use soroban_sdk::{contract, contractimpl, BytesN, Env};
//! use soroban_testkit_fixtures::deterministic_deployer::DeterministicDeployer;
//!
//! #[contract]
//! pub struct Child;
//!
//! #[contractimpl]
//! impl Child {
//!     pub fn ping() -> u32 {
//!         7
//!     }
//! }
//!
//! # fn main() {
//! let env = Env::default();
//! let wasm_hash = env.upload(Child);
//! let deployer = env.current_contract_address();
//! let salt = BytesN::from_array(&env, &[1u8; 32]);
//!
//! let predicted = DeterministicDeployer::predicted_address(&env, &deployer, &salt);
//! let deployed = DeterministicDeployer::deploy(&env, &deployer, &salt, &wasm_hash);
//! assert_eq!(predicted, deployed);
//! # }
//! ```

use soroban_sdk::{Address, BytesN, ContractExecutable, Env};

/// Computes deterministic contract addresses and deploys contracts with explicit salts.
#[derive(Debug, Clone, Default)]
pub struct DeterministicDeployer;

impl DeterministicDeployer {
    /// Predict the contract address for `deployer + salt` without deploying.
    ///
    /// This uses the host's deployer address derivation, matching
    /// `env.deployer().with_address(...).deployed_address()`.
    pub fn predicted_address(env: &Env, deployer: &Address, salt: &BytesN<32>) -> Address {
        env.deployer()
            .with_address(deployer.clone(), salt.clone())
            .deployed_address()
    }

    /// Deploy a contract from an uploaded WASM hash using `deployer + salt`.
    ///
    /// Returns the deployed contract address. The address is deterministic for
    /// the same deployer, salt, and network context; callers can compare it to
    /// [`Self::predicted_address`].
    pub fn deploy(
        env: &Env,
        deployer: &Address,
        salt: &BytesN<32>,
        wasm_hash: &BytesN<32>,
    ) -> Address {
        env.deployer()
            .with_address(deployer.clone(), salt.clone())
            .deploy_contract(ContractExecutable::Wasm(wasm_hash.clone()), ())
    }

    /// Predict, deploy, and assert the deployed address matches the prediction.
    pub fn deploy_checked(
        env: &Env,
        deployer: &Address,
        salt: &BytesN<32>,
        wasm_hash: &BytesN<32>,
    ) -> Address {
        let predicted = Self::predicted_address(env, deployer, salt);
        let deployed = Self::deploy(env, deployer, salt, wasm_hash);
        assert_eq!(
            predicted, deployed,
            "deterministic deploy address mismatch: predicted {predicted:?}, deployed {deployed:?}"
        );
        deployed
    }
}

/// Derives deterministic 32-byte salts for related deployments.
///
/// Useful when a test suite deploys multiple contracts from one factory/root and
/// needs stable, namespaced salts.
#[derive(Debug, Clone)]
pub struct SaltHierarchy {
    root: [u8; 32],
}

impl SaltHierarchy {
    /// Create a hierarchy from a root salt.
    pub fn new(root: [u8; 32]) -> Self {
        Self { root }
    }

    /// Create a hierarchy from a single-byte namespace, useful in tests.
    pub fn from_namespace(namespace: u8) -> Self {
        let mut root = [0u8; 32];
        root[0] = namespace;
        Self { root }
    }

    /// Return the root salt.
    pub fn root(&self) -> [u8; 32] {
        self.root
    }

    /// Derive a child salt from a path of indexes.
    ///
    /// Derivation is deterministic and order-sensitive. Each path element is
    /// mixed into the salt with a simple domain-separated XOR/rotation step so
    /// different hierarchies do not accidentally collide in tests.
    pub fn derive(&self, path: &[u32]) -> [u8; 32] {
        let mut out = self.root;
        for (depth, segment) in path.iter().enumerate() {
            let bytes = segment.to_be_bytes();
            for (i, b) in bytes.iter().enumerate() {
                let idx = (depth * 7 + i) % 32;
                out[idx] ^= b.wrapping_add((depth as u8).wrapping_mul(31));
                out[idx] = out[idx].rotate_left(1);
            }
            // domain separator to avoid trivial [1,2] vs [3] style collisions
            out[31] ^= (depth as u8).wrapping_add(0xA5);
        }
        out
    }

    /// Derive a child salt as a Soroban `BytesN<32>`.
    pub fn derive_bytesn(&self, env: &Env, path: &[u32]) -> BytesN<32> {
        BytesN::from_array(env, &self.derive(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{contract, contractimpl, symbol_short, Env, Symbol};

    #[contract]
    pub struct ChildContract;

    #[contractimpl]
    impl ChildContract {
        pub fn ping(_env: Env) -> Symbol {
            symbol_short!("pong")
        }
    }

    #[contract]
    pub struct Factory;

    #[contractimpl]
    impl Factory {
        #[allow(deprecated)]
        pub fn deploy_child(env: Env, salt: BytesN<32>, wasm_hash: BytesN<32>) -> Address {
            env.deployer().with_current_contract(salt).deploy(wasm_hash)
        }
    }

    fn setup() -> (Env, BytesN<32>) {
        let env = Env::default();
        env.mock_all_auths();
        let wasm_hash = env.upload(ChildContract);
        (env, wasm_hash)
    }

    #[test]
    fn predicted_address_matches_deployed_address() {
        let (env, wasm_hash) = setup();
        let deployer = Address::generate(&env);
        let salt = BytesN::from_array(&env, &[7u8; 32]);

        let predicted = DeterministicDeployer::predicted_address(&env, &deployer, &salt);
        let deployed = DeterministicDeployer::deploy(&env, &deployer, &salt, &wasm_hash);

        assert_eq!(predicted, deployed);
    }

    #[test]
    fn deploy_checked_returns_matching_address() {
        let (env, wasm_hash) = setup();
        let deployer = Address::generate(&env);
        let salt = BytesN::from_array(&env, &[9u8; 32]);

        let deployed = DeterministicDeployer::deploy_checked(&env, &deployer, &salt, &wasm_hash);
        let predicted = DeterministicDeployer::predicted_address(&env, &deployer, &salt);
        assert_eq!(predicted, deployed);
    }

    #[test]
    fn same_deployer_and_salt_predict_same_address() {
        let env = Env::default();
        let deployer = Address::generate(&env);
        let salt = BytesN::from_array(&env, &[3u8; 32]);

        let a = DeterministicDeployer::predicted_address(&env, &deployer, &salt);
        let b = DeterministicDeployer::predicted_address(&env, &deployer, &salt);
        assert_eq!(a, b);
    }

    #[test]
    fn different_salts_predict_different_addresses() {
        let env = Env::default();
        let deployer = Address::generate(&env);
        let salt_a = BytesN::from_array(&env, &[1u8; 32]);
        let salt_b = BytesN::from_array(&env, &[2u8; 32]);

        let a = DeterministicDeployer::predicted_address(&env, &deployer, &salt_a);
        let b = DeterministicDeployer::predicted_address(&env, &deployer, &salt_b);
        assert_ne!(a, b);
    }

    #[test]
    fn factory_deployment_can_be_predicted_from_current_contract() {
        let (env, wasm_hash) = setup();
        let factory_id = env.register(Factory, ());
        let client = FactoryClient::new(&env, &factory_id);
        let salt = BytesN::from_array(&env, &[11u8; 32]);

        let predicted = DeterministicDeployer::predicted_address(&env, &factory_id, &salt);
        let deployed = client.deploy_child(&salt, &wasm_hash);

        assert_eq!(predicted, deployed);
    }

    #[test]
    fn salt_hierarchy_is_deterministic_and_path_sensitive() {
        let hierarchy = SaltHierarchy::from_namespace(7);
        assert_eq!(hierarchy.derive(&[1, 2]), hierarchy.derive(&[1, 2]));
        assert_ne!(hierarchy.derive(&[1, 2]), hierarchy.derive(&[1, 3]));
        assert_ne!(hierarchy.derive(&[1, 2]), hierarchy.derive(&[2, 1]));
    }

    #[test]
    fn salt_hierarchy_bytesn_matches_array() {
        let env = Env::default();
        let hierarchy = SaltHierarchy::new([5u8; 32]);
        let path = [10u32, 20u32];
        let bytesn = hierarchy.derive_bytesn(&env, &path);
        assert_eq!(bytesn.to_array(), hierarchy.derive(&path));
    }
}
