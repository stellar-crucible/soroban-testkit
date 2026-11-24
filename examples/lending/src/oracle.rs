use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum OracleError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    InvalidPrice = 3,
    NoPrice = 4,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriceData {
    pub price: i128,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum OracleKey {
    Admin,
    Price,
}

#[contract]
pub struct LendingOracle;

#[contractimpl]
impl LendingOracle {
    pub fn initialize(env: Env, admin: Address) -> Result<(), OracleError> {
        if env.storage().instance().has(&OracleKey::Admin) {
            return Err(OracleError::AlreadyInitialized);
        }
        env.storage().instance().set(&OracleKey::Admin, &admin);
        Ok(())
    }

    pub fn set_price(env: Env, price: i128) -> Result<(), OracleError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&OracleKey::Admin)
            .ok_or(OracleError::NotInitialized)?;
        admin.require_auth();
        if price <= 0 {
            return Err(OracleError::InvalidPrice);
        }
        let data = PriceData {
            price,
            timestamp: env.ledger().timestamp(),
        };
        env.storage().instance().set(&OracleKey::Price, &data);
        env.events().publish((symbol_short!("price"),), price);
        Ok(())
    }

    pub fn price(env: Env) -> Result<PriceData, OracleError> {
        env.storage()
            .instance()
            .get(&OracleKey::Price)
            .ok_or(OracleError::NoPrice)
    }
}
