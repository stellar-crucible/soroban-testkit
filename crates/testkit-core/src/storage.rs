use soroban_sdk::{BytesN, Env};

#[derive(Debug, Clone)]
pub struct StorageEntry {
    pub key: String,
    pub tier: StorageTier,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StorageTier {
    Instance,
    Persistent,
    Temporary,
}

pub fn inspect_storage(env: &Env, contract_id: &BytesN<32>) -> Vec<StorageEntry> {
    let _ = (env, contract_id);
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::{inspect_storage, StorageEntry, StorageTier};
    use soroban_sdk::{BytesN, Env};

    #[test]
    fn tiers_compare_by_variant() {
        assert_eq!(StorageTier::Instance, StorageTier::Instance);
        assert_ne!(StorageTier::Instance, StorageTier::Persistent);
        assert_ne!(StorageTier::Persistent, StorageTier::Temporary);
    }

    #[test]
    fn entry_is_clonable() {
        let entry = StorageEntry {
            key: "counter".to_string(),
            tier: StorageTier::Persistent,
        };
        let copy = entry.clone();
        assert_eq!(copy.key, "counter");
        assert_eq!(copy.tier, StorageTier::Persistent);
    }

    #[test]
    fn inspection_is_a_stub_for_any_contract_id() {
        let env = Env::default();
        let all_zeros = BytesN::from_array(&env, &[0u8; 32]);
        let arbitrary = BytesN::from_array(&env, &[0xabu8; 32]);

        assert!(inspect_storage(&env, &all_zeros).is_empty());
        assert!(inspect_storage(&env, &arbitrary).is_empty());
    }
}
