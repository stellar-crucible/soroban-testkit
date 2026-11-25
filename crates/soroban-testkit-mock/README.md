# soroban-testkit-mock

`#[mock_contract]` turns a trait describing a contract interface into a mock
Soroban contract you can register in a test environment, program per function,
and inspect afterwards.

```rust
use soroban_sdk::{Address, Env};
use soroban_testkit_mock::mock_contract;

#[mock_contract]
pub trait TokenInterface {
    fn transfer(from: Address, to: Address, amount: i128);
    fn balance_of(account: Address) -> i128;
}

let env = Env::default();
let mock = MockTokenInterface::new(&env);
mock.expect_balance_of().returning(|_| 1000);
// hand mock.address() to the contract under test, then:
mock.verify();
```

- Documentation: <https://stellar-crucible.github.io/soroban-testkit/guides/mocking.html>
- Source: [crates/soroban-testkit-mock](https://github.com/stellar-crucible/soroban-testkit/tree/main/crates/soroban-testkit-mock)
- Licence: Apache-2.0
