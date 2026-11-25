//! Shared setup: a provider with liquidity in two Stellar Asset Contract
//! tokens, and a receiver that holds enough of each to pay fees.
#![allow(dead_code)]

use soroban_sdk::testutils::Address as _;
use soroban_sdk::token::{StellarAssetClient, TokenClient};
use soroban_sdk::{Address, Bytes, Env};
use soroban_testkit_example_flash_loan::{
    FlashLoanProvider, FlashLoanProviderClient, FlashLoanReceiver, FlashLoanReceiverClient,
    RepayMode,
};

/// 0.09%, a common flash loan fee.
pub const FEE_BPS: u32 = 9;
/// Pool liquidity per token.
pub const LIQUIDITY: i128 = 1_000_000_000;
/// Balance the receiver starts with per token, to pay fees from.
pub const RECEIVER_FLOAT: i128 = 10_000_000;

pub struct Market {
    pub env: Env,
    pub admin: Address,
    pub borrower: Address,
    pub provider: Address,
    pub receiver: Address,
    pub tokens: [Address; 2],
}

impl Market {
    /// A market on a fresh environment that mocks every authorization.
    pub fn setup() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        Self::on(env)
    }

    /// A market on `env`, which must mock authorizations while it is built.
    pub fn on(env: Env) -> Self {
        let admin = Address::generate(&env);
        let borrower = Address::generate(&env);
        let provider = env.register(FlashLoanProvider, ());
        let receiver = env.register(FlashLoanReceiver, ());
        let tokens = [
            env.register_stellar_asset_contract_v2(admin.clone())
                .address(),
            env.register_stellar_asset_contract_v2(admin.clone())
                .address(),
        ];

        let lender = FlashLoanProviderClient::new(&env, &provider);
        lender.initialize(&admin, &FEE_BPS);
        for token in &tokens {
            lender.add_token(token);
            let asset = StellarAssetClient::new(&env, token);
            asset.mint(&provider, &LIQUIDITY);
            asset.mint(&receiver, &RECEIVER_FLOAT);
        }
        FlashLoanReceiverClient::new(&env, &receiver).configure(&provider, &RepayMode::Full);

        Self {
            env,
            admin,
            borrower,
            provider,
            receiver,
            tokens,
        }
    }

    pub fn lender(&self) -> FlashLoanProviderClient<'_> {
        FlashLoanProviderClient::new(&self.env, &self.provider)
    }

    pub fn borrower_contract(&self) -> FlashLoanReceiverClient<'_> {
        FlashLoanReceiverClient::new(&self.env, &self.receiver)
    }

    pub fn token(&self, index: usize) -> TokenClient<'_> {
        TokenClient::new(&self.env, &self.tokens[index])
    }

    pub fn balance(&self, index: usize, who: &Address) -> i128 {
        self.token(index).balance(who)
    }

    pub fn set_mode(&self, mode: RepayMode) {
        self.borrower_contract().configure(&self.provider, &mode);
    }

    pub fn data(&self) -> Bytes {
        Bytes::from_slice(&self.env, b"arbitrage")
    }

    /// Borrow `amount` of token `index` for the default borrower.
    pub fn borrow(&self, index: usize, amount: i128) -> i128 {
        self.lender().flash_loan(
            &self.borrower,
            &self.receiver,
            &self.tokens[index],
            &amount,
            &self.data(),
        )
    }
}
