# Mocking Dependency Contracts

A contract that calls another contract — a token, an oracle, a registry —
normally needs a second, hand-written contract in its tests that implements
the dependency's whole interface. `soroban-testkit-mock` generates that stub
from a trait instead.

```toml
[dev-dependencies]
soroban-testkit-mock = "0.4"
```

## Quick start

```rust
use soroban_sdk::{testutils::Address as _, Address, Env};
use soroban_testkit_mock::mock_contract;

#[mock_contract]
pub trait TokenInterface {
    fn transfer(from: Address, to: Address, amount: i128);
    fn balance_of(account: Address) -> i128;
}

#[test]
fn pays_when_solvent() {
    let env = Env::default();
    let token = MockTokenInterface::new(&env);
    token.expect_balance_of().returning(|_| 1000);
    token.expect_transfer().once();

    // ...register the contract under test and give it `token.address()`...

    assert_eq!(token.transfer_calls().len(), 1);
    token.verify();
}
```

`MockTokenInterface::new` takes the `Env` to register the mock contract in.

## What the macro generates

For a trait `T`, `#[mock_contract]` keeps the trait as written and adds:

| Item | What it is |
|------|------------|
| `MockTContract` | A real `#[contract]` exporting every trait function under the same name and argument types, plus its generated `MockTContractClient` |
| `MockT` | The handle: `new(&env)`, `address()`, `expect_*()`, `calls()`, `*_calls()`, `call_count()`, `verify()`, `checkpoint()`, `reset()` |
| `MockTCall` | An enum with one variant per function, holding that call's named arguments; `function()` returns the function name |
| `MockT{Function}Expectation` | The builder returned by `expect_{function}()` |

Because the mock is an ordinary contract, the contract under test calls it
exactly as it would call the real dependency, through `env.invoke_contract`, a
`#[contractclient]` client or the generated `MockTContractClient`.

### The `#[contractclient]` shape

Interfaces that other contracts call usually take `env: Env` first, so the SDK
can generate a client from them. `#[mock_contract]` accepts that shape too, and
stacks with `#[contractclient]`:

```rust
#[mock_contract]
#[contractclient(name = "OracleClient")]
pub trait Oracle {
    fn price(env: Env, asset: Symbol) -> i128;
}
```

When **every** function takes `env: Env` first, the mock contract implements
the trait itself (`impl Oracle for MockOracleContract`), so the compiler checks
that the mock matches the interface. Otherwise it exports an inherent
`#[contractimpl]` with the same functions, injecting the `env` parameter.

## Programming return values

Each `expect_*()` call adds an expectation. Configure it with:

| Method | Effect |
|--------|--------|
| `returning(\|a, b, …\| value)` | Compute the return value from references to the arguments |
| `returns(value)` | Return a clone of `value` |
| `with(a, b, …)` | Only handle calls whose arguments equal these |
| `withf(\|a, b, …\| bool)` | Only handle calls the predicate accepts |
| `times(n)` / `once()` / `never()` | Expect exactly `n` matching calls; checked by `verify()` |

When a call arrives, expectations are tried in creation order. The first whose
matcher accepts the arguments and whose `times` limit is not yet reached
handles the call. If all matching expectations are saturated, the last matching
one handles it anyway and its count goes over, so `verify()` reports the extra
call.

```rust
oracle.expect_price().with(xlm.clone()).returns(12);
oracle.expect_price().with(usdc.clone()).returns(100);

oracle.expect_price().times(1).returns(1); // first call
oracle.expect_price().returns(2);          // every call after that
```

Functions returning `Result<T, E>` can return contract errors:

```rust
oracle.expect_checked_price().returns(Err(OracleError::Stale));
```

A function with a return value and **no matching expectation** panics inside
the mock. The host turns that into a failed invocation, so a `try_*` call
returns `Err` and a plain call fails the test with a message naming the
function. Functions returning `()` need no expectation: calls are recorded and
return.

## Inspecting calls

Every call is recorded before the expectation runs:

```rust
// Typed, per function: a Vec of argument tuples, oldest first.
assert_eq!(token.transfer_calls(), vec![(from.clone(), to.clone(), 200)]);
assert_eq!(token.balance_of_calls(), vec![(from.clone(),)]);

// Every call, in order, as an enum you can match on.
let calls = token.calls();
assert_eq!(calls[0].function(), "balance_of");
assert_eq!(calls[1], MockTokenInterfaceCall::Transfer { from, to, amount: 200 });
assert_eq!(token.call_count(), 2);
```

## Verifying expectations

`verify()` panics with a line per unmet `times` expectation:

```text
mock Oracle expectations not met:
  Oracle::price expectation #0: expected 2 call(s), got 1
```

`checkpoint()` verifies and then clears every expectation and recorded call,
for tests that run in phases; `reset()` clears without verifying.

## Requirements and limits

- Trait functions must not take `self`, be generic or be `async`; the trait may
  only contain functions.
- Argument types must be `Clone + Debug + PartialEq` (all Soroban contract
  types are) and return types `Clone`, because calls are stored and compared.
- The generated code uses `std` — thread-local state and boxed closures — so
  use it from integration tests or a `#[cfg(test)]` module. To use it from a
  `#![no_std]` contract's unit tests, add `extern crate std;` to the test module.
- Each mock handle owns its state; dropping the handle forgets it, and any
  later call to that mock contract fails. Several mocks of the same trait in one
  environment are independent.

## How it works

The handle registers `MockTContract` with a constructor argument holding a
unique id, stored in the contract's instance storage. Expectations and call
history live in a thread-local registry keyed by that id. Each exported
function reads its id, records the call, picks an expectation, releases the
registry and then runs your closure, so a `returning` closure can safely read
the mock's state.
