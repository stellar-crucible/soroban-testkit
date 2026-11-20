#![no_std]
#![allow(deprecated)]

use soroban_sdk::token::TokenClient;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, String, Vec,
};

/// Basis points denominator (100% = 10,000 bps).
pub const BPS_DENOMINATOR: i128 = 10_000;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum NftError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    TokenAlreadyExists = 3,
    TokenNotFound = 4,
    Unauthorized = 5,
    ApprovalExpired = 6,
    InvalidRecipient = 7,
    InvalidRoyalty = 8,
    ArithmeticError = 9,
    InvalidBatchLength = 10,
    ZeroPrice = 11,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NftConfig {
    pub admin: Address,
    pub name: String,
    pub symbol: String,
    pub royalty_recipient: Address,
    pub royalty_bps: u32,
    pub total_supply: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalEntry {
    pub operator: Address,
    pub live_until_ledger: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Config,
    Owner(u32),
    Balance(Address),
    TokenApproval(u32),
    OperatorApproval(Address, Address),
    TokenUri(u32),
}

#[contract]
pub struct NftContract;

#[contractimpl]
impl NftContract {
    /// Initialize the NFT contract with metadata, admin, and royalty terms.
    pub fn initialize(
        env: Env,
        admin: Address,
        name: String,
        symbol: String,
        royalty_recipient: Address,
        royalty_bps: u32,
    ) {
        if env.storage().persistent().has(&DataKey::Config) {
            panic!("{:?}", NftError::AlreadyInitialized);
        }
        if royalty_bps > 10_000 {
            panic!("{:?}", NftError::InvalidRoyalty);
        }

        let config = NftConfig {
            admin,
            name,
            symbol,
            royalty_recipient,
            royalty_bps,
            total_supply: 0,
        };

        env.storage().persistent().set(&DataKey::Config, &config);
    }

    /// Mint a unique NFT to `to` with metadata URI.
    pub fn mint(env: Env, to: Address, token_id: u32, uri: String) -> u32 {
        let mut config: NftConfig = env
            .storage()
            .persistent()
            .get(&DataKey::Config)
            .unwrap_or_else(|| panic!("{:?}", NftError::NotInitialized));

        config.admin.require_auth();

        if env.storage().persistent().has(&DataKey::Owner(token_id)) {
            panic!("{:?}", NftError::TokenAlreadyExists);
        }

        env.storage()
            .persistent()
            .set(&DataKey::Owner(token_id), &to);
        env.storage()
            .persistent()
            .set(&DataKey::TokenUri(token_id), &uri);

        let balance = Self::balance_of(env.clone(), to.clone());
        let new_balance = balance
            .checked_add(1)
            .unwrap_or_else(|| panic!("{:?}", NftError::ArithmeticError));
        env.storage()
            .persistent()
            .set(&DataKey::Balance(to.clone()), &new_balance);

        config.total_supply = config
            .total_supply
            .checked_add(1)
            .unwrap_or_else(|| panic!("{:?}", NftError::ArithmeticError));
        env.storage().persistent().set(&DataKey::Config, &config);

        env.events().publish((symbol_short!("mint"), to), token_id);

        token_id
    }

    /// Mint a batch of NFTs to `to`.
    pub fn batch_mint(env: Env, to: Address, token_ids: Vec<u32>, uris: Vec<String>) -> u32 {
        if token_ids.len() != uris.len() {
            panic!("{:?}", NftError::InvalidBatchLength);
        }

        let count = token_ids.len();
        for i in 0..count {
            let id = token_ids.get(i).unwrap();
            let uri = uris.get(i).unwrap();
            Self::mint(env.clone(), to.clone(), id, uri);
        }

        count
    }

    /// Burn a token, destroying its metadata and decrementing balances.
    pub fn burn(env: Env, caller: Address, token_id: u32) {
        caller.require_auth();

        let owner = Self::owner_of(env.clone(), token_id)
            .unwrap_or_else(|| panic!("{:?}", NftError::TokenNotFound));

        if !Self::check_authorized(&env, &caller, token_id, &owner) {
            panic!("{:?}", NftError::Unauthorized);
        }

        env.storage().persistent().remove(&DataKey::Owner(token_id));
        env.storage()
            .persistent()
            .remove(&DataKey::TokenUri(token_id));
        env.storage()
            .persistent()
            .remove(&DataKey::TokenApproval(token_id));

        let balance = Self::balance_of(env.clone(), owner.clone());
        if balance > 0 {
            env.storage()
                .persistent()
                .set(&DataKey::Balance(owner.clone()), &(balance - 1));
        }

        let mut config: NftConfig = env.storage().persistent().get(&DataKey::Config).unwrap();
        if config.total_supply > 0 {
            config.total_supply -= 1;
            env.storage().persistent().set(&DataKey::Config, &config);
        }

        env.events()
            .publish((symbol_short!("burn"), owner), token_id);
    }

    /// Approve an operator to manage a single token ID until `live_until_ledger`.
    pub fn approve(
        env: Env,
        owner: Address,
        operator: Address,
        token_id: u32,
        live_until_ledger: u64,
    ) {
        owner.require_auth();

        let actual_owner = Self::owner_of(env.clone(), token_id)
            .unwrap_or_else(|| panic!("{:?}", NftError::TokenNotFound));
        if actual_owner != owner {
            panic!("{:?}", NftError::Unauthorized);
        }

        let entry = ApprovalEntry {
            operator: operator.clone(),
            live_until_ledger,
        };
        env.storage()
            .persistent()
            .set(&DataKey::TokenApproval(token_id), &entry);

        env.events().publish(
            (symbol_short!("approve"), owner),
            (token_id, operator, live_until_ledger),
        );
    }

    /// Revoke approval for a single token ID.
    pub fn revoke_approval(env: Env, owner: Address, token_id: u32) {
        owner.require_auth();

        let actual_owner = Self::owner_of(env.clone(), token_id)
            .unwrap_or_else(|| panic!("{:?}", NftError::TokenNotFound));
        if actual_owner != owner {
            panic!("{:?}", NftError::Unauthorized);
        }

        env.storage()
            .persistent()
            .remove(&DataKey::TokenApproval(token_id));

        env.events()
            .publish((symbol_short!("revoked"), owner), token_id);
    }

    /// Set or revoke operator approval for all tokens owned by `owner`.
    pub fn set_approval_for_all(
        env: Env,
        owner: Address,
        operator: Address,
        approved: bool,
        live_until_ledger: u64,
    ) {
        owner.require_auth();

        if approved {
            let entry = ApprovalEntry {
                operator: operator.clone(),
                live_until_ledger,
            };
            env.storage().persistent().set(
                &DataKey::OperatorApproval(owner.clone(), operator.clone()),
                &entry,
            );
        } else {
            env.storage()
                .persistent()
                .remove(&DataKey::OperatorApproval(owner.clone(), operator.clone()));
        }

        env.events().publish(
            (symbol_short!("appr_all"), owner),
            (operator, approved, live_until_ledger),
        );
    }

    /// Transfer token ID from its current owner to `to`.
    pub fn transfer(env: Env, caller: Address, to: Address, token_id: u32) {
        caller.require_auth();

        let owner = Self::owner_of(env.clone(), token_id)
            .unwrap_or_else(|| panic!("{:?}", NftError::TokenNotFound));

        if !Self::check_authorized(&env, &caller, token_id, &owner) {
            panic!("{:?}", NftError::Unauthorized);
        }

        // Clear token approval on transfer
        env.storage()
            .persistent()
            .remove(&DataKey::TokenApproval(token_id));

        // Update ownership and balances
        env.storage()
            .persistent()
            .set(&DataKey::Owner(token_id), &to);

        let owner_balance = Self::balance_of(env.clone(), owner.clone());
        if owner_balance > 0 {
            env.storage()
                .persistent()
                .set(&DataKey::Balance(owner.clone()), &(owner_balance - 1));
        }

        let to_balance = Self::balance_of(env.clone(), to.clone());
        env.storage()
            .persistent()
            .set(&DataKey::Balance(to.clone()), &(to_balance + 1));

        env.events()
            .publish((symbol_short!("transfer"), owner), (to, token_id));
    }

    /// Safe transfer ensuring destination is not the contract itself.
    pub fn safe_transfer(env: Env, caller: Address, to: Address, token_id: u32) {
        if to == env.current_contract_address() {
            panic!("{:?}", NftError::InvalidRecipient);
        }
        Self::transfer(env, caller, to, token_id);
    }

    /// Calculate secondary sale royalty split: `(recipient, royalty_amount)`.
    pub fn calculate_royalty(env: Env, sale_price: i128) -> (Address, i128) {
        let config: NftConfig = env
            .storage()
            .persistent()
            .get(&DataKey::Config)
            .unwrap_or_else(|| panic!("{:?}", NftError::NotInitialized));

        if sale_price <= 0 {
            return (config.royalty_recipient, 0);
        }

        let royalty = sale_price
            .checked_mul(config.royalty_bps as i128)
            .unwrap_or(0)
            / BPS_DENOMINATOR;

        (config.royalty_recipient, royalty)
    }

    /// Settle a secondary marketplace sale: transfers token to buyer, pays royalty
    /// split to creator, and forwards remainder to seller.
    pub fn secondary_sale(
        env: Env,
        seller: Address,
        buyer: Address,
        token_id: u32,
        payment_token: Address,
        price: i128,
    ) {
        buyer.require_auth();

        if price <= 0 {
            panic!("{:?}", NftError::ZeroPrice);
        }

        let owner = Self::owner_of(env.clone(), token_id)
            .unwrap_or_else(|| panic!("{:?}", NftError::TokenNotFound));

        if owner != seller {
            panic!("{:?}", NftError::Unauthorized);
        }

        let config: NftConfig = env
            .storage()
            .persistent()
            .get(&DataKey::Config)
            .unwrap_or_else(|| panic!("{:?}", NftError::NotInitialized));

        let royalty_amount =
            price.checked_mul(config.royalty_bps as i128).unwrap_or(0) / BPS_DENOMINATOR;
        let seller_amount = price
            .checked_sub(royalty_amount)
            .unwrap_or_else(|| panic!("{:?}", NftError::ArithmeticError));

        // Transfer funds: payment token from buyer to seller and royalty recipient
        let token_client = TokenClient::new(&env, &payment_token);
        token_client.transfer(&buyer, &seller, &seller_amount);

        if royalty_amount > 0 {
            token_client.transfer(&buyer, &config.royalty_recipient, &royalty_amount);
            env.events().publish(
                (symbol_short!("royalty"), config.royalty_recipient),
                (token_id, royalty_amount),
            );
        }

        // Transfer NFT ownership directly
        env.storage()
            .persistent()
            .remove(&DataKey::TokenApproval(token_id));
        env.storage()
            .persistent()
            .set(&DataKey::Owner(token_id), &buyer);

        let seller_balance = Self::balance_of(env.clone(), seller.clone());
        if seller_balance > 0 {
            env.storage()
                .persistent()
                .set(&DataKey::Balance(seller.clone()), &(seller_balance - 1));
        }

        let buyer_balance = Self::balance_of(env.clone(), buyer.clone());
        env.storage()
            .persistent()
            .set(&DataKey::Balance(buyer.clone()), &(buyer_balance + 1));

        env.events()
            .publish((symbol_short!("sale"), seller), (buyer, token_id, price));
    }

    /// Update metadata URI for an existing token.
    pub fn update_metadata(env: Env, admin: Address, token_id: u32, uri: String) {
        let config: NftConfig = env
            .storage()
            .persistent()
            .get(&DataKey::Config)
            .unwrap_or_else(|| panic!("{:?}", NftError::NotInitialized));

        admin.require_auth();
        if admin != config.admin {
            panic!("{:?}", NftError::Unauthorized);
        }

        if !env.storage().persistent().has(&DataKey::Owner(token_id)) {
            panic!("{:?}", NftError::TokenNotFound);
        }

        env.storage()
            .persistent()
            .set(&DataKey::TokenUri(token_id), &uri);

        env.events()
            .publish((symbol_short!("meta_upd"), admin), token_id);
    }

    /// Update royalty recipient and percentage.
    pub fn update_royalty(env: Env, admin: Address, new_recipient: Address, new_bps: u32) {
        let mut config: NftConfig = env
            .storage()
            .persistent()
            .get(&DataKey::Config)
            .unwrap_or_else(|| panic!("{:?}", NftError::NotInitialized));

        admin.require_auth();
        if admin != config.admin {
            panic!("{:?}", NftError::Unauthorized);
        }
        if new_bps > 10_000 {
            panic!("{:?}", NftError::InvalidRoyalty);
        }

        config.royalty_recipient = new_recipient;
        config.royalty_bps = new_bps;
        env.storage().persistent().set(&DataKey::Config, &config);
    }

    /// Retrieve owner of a token ID.
    pub fn owner_of(env: Env, token_id: u32) -> Option<Address> {
        env.storage().persistent().get(&DataKey::Owner(token_id))
    }

    /// Retrieve balance (number of tokens owned) of an address.
    pub fn balance_of(env: Env, owner: Address) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::Balance(owner))
            .unwrap_or(0)
    }

    /// Retrieve total supply of minted tokens.
    pub fn total_supply(env: Env) -> u32 {
        let config: Option<NftConfig> = env.storage().persistent().get(&DataKey::Config);
        config.map(|c| c.total_supply).unwrap_or(0)
    }

    /// Retrieve metadata URI of a token.
    pub fn token_uri(env: Env, token_id: u32) -> Option<String> {
        env.storage().persistent().get(&DataKey::TokenUri(token_id))
    }

    /// Retrieve approved operator for a single token ID.
    pub fn get_approved(env: Env, token_id: u32) -> Option<Address> {
        let entry: Option<ApprovalEntry> = env
            .storage()
            .persistent()
            .get(&DataKey::TokenApproval(token_id));
        if let Some(entry) = entry {
            let current_ledger = env.ledger().sequence() as u64;
            if current_ledger <= entry.live_until_ledger {
                return Some(entry.operator);
            }
        }
        None
    }

    /// Check if `operator` is approved for all tokens of `owner`.
    pub fn is_approved_for_all(env: Env, owner: Address, operator: Address) -> bool {
        let entry: Option<ApprovalEntry> = env
            .storage()
            .persistent()
            .get(&DataKey::OperatorApproval(owner, operator));
        if let Some(entry) = entry {
            let current_ledger = env.ledger().sequence() as u64;
            return current_ledger <= entry.live_until_ledger;
        }
        false
    }

    /// Retrieve contract configuration.
    pub fn get_config(env: Env) -> NftConfig {
        env.storage()
            .persistent()
            .get(&DataKey::Config)
            .unwrap_or_else(|| panic!("{:?}", NftError::NotInitialized))
    }

    fn check_authorized(env: &Env, caller: &Address, token_id: u32, owner: &Address) -> bool {
        if caller == owner {
            return true;
        }

        // Check single token approval
        if let Some(approved) = Self::get_approved(env.clone(), token_id) {
            if &approved == caller {
                return true;
            }
        }

        // Check operator approval for all
        Self::is_approved_for_all(env.clone(), owner.clone(), caller.clone())
    }
}
