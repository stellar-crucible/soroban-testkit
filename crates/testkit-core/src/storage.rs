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
