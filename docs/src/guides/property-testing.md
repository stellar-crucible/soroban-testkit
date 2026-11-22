# Property Testing

Property-based testing checks invariants across hundreds of generated inputs, catching the edge cases example-based tests miss. Testkit's generators keep those inputs inside ranges Soroban actually accepts.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Crate</span><span class="tk-spec__value">soroban-testkit-generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Backend</span><span class="tk-spec__value">proptest 1.x</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Cases</span><span class="tk-spec__value">256 by default</span></div>
</div>

## Setup

Enable the `proptest` feature:

```toml
[dev-dependencies]
soroban-testkit-generators = { version = "0.4.1", features = ["proptest"] }
proptest = "1"
```

## Basic property test

```rust
use proptest::prelude::*;
use soroban_testkit_generators::strategies;

proptest! {
    #[test]
    fn balance_never_negative(amount in strategies::token_amount()) {
        // All generated amounts are >= 0 and within realistic bounds
        assert!(amount >= 0);
    }
}
```

## Combining with fixtures

```rust
proptest! {
    #[test]
    fn transfer_preserves_supply(
        amount_a in strategies::token_amount(),
        amount_b in strategies::token_amount(),
    ) {
        let ctx = TestContextBuilder::new().with_users(2).build();
        // ... initialize balances, perform transfers
        // ... assert total supply unchanged
    }
}
```

<div class="tk-callout">
  <span class="tk-callout__title">Mind the host</span>
  <p>Every generated case builds a real <code>Env</code> and executes on the host. If the suite slows down, reduce cases with <code>#![proptest_config(ProptestConfig::with_cases(64))]</code> before you narrow the strategy ranges.</p>
</div>

<div class="tk-callout">
  <span class="tk-callout__title">A runnable version of this</span>
  <p><code>examples/token/tests/property.rs</code> is a suite that follows every rule on this page: five cases over a live contract at 64 iterations, three strategy-only cases at 256, <code>prop_assume!</code> for the relations between generated amounts, and both address formats on the same call. <a href="./token-example.html">Worked Example: Token</a> walks through it.</p>
</div>

## Generating principals

Access-control properties need callers, and a caller in Soroban is an address rather than a string. `address()` generates a valid 32-byte principal in either format, `account_address()` and `contract_address()` pin one, and `address_of_type(AddressType::Account)` takes the format as an argument. Each yields an `AddressBytes`, which becomes a host value through `to_address(&env)`.

```rust
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_generators::strategies;

proptest! {
    #[test]
    fn only_the_caller_moves_their_balance(
        caller in strategies::account_address(),
        amount in strategies::token_amount(),
    ) {
        let ctx = TestContextBuilder::new().build();
        let caller = caller.to_address(&ctx.env);
        // ... register the contract, fund `caller`, then assert that the same
        //     call from a different address fails or records no authorization
    }
}
```

Generating the *contract* form is what catches an address-format assumption — a `C...` id where the code only ever saw a `G...` account. The full list with return types is in the [strategy reference](../crates/generators.html).

<div class="tk-callout">
  <span class="tk-callout__title">Uniqueness is per run, not per seed</span>
  <p>Every generated address carries a process-wide nonce, so two calls in one test run never collide even when their random prefixes do. The trade-off is that a value depends on generation order: judge a failing case by the bytes proptest prints rather than by replaying a seed alone.</p>
</div>

## Differential testing against a reference model

A property test asserts something you can state without computing it — supply is conserved, a balance never goes negative. When you *can* compute the answer, compare against it: write the rule once as a contract and once as plain Rust, and check that every generated input makes the two agree. `DifferentialHarness` runs both sides and shrinks the first disagreement down to a small input you can paste into a regression test.

```rust
use soroban_testkit_generators::differential::DifferentialHarness;
use soroban_testkit_generators::strategies::token_amount;

let harness = DifferentialHarness::new();
harness.compare(
    token_amount(),
    |amount| client.calculate_fee(&amount),
    |amount| reference_fee_calculator(amount),
);
```

The first closure is the contract, the second the model. Both take the generated input by value, so the contract call borrows it (`&amount`) while the model consumes it. `compare` panics on the shrunk input, naming what each side did with it:

```text
contract diverged from its reference model
	smallest failing input: 1
	return value: contract 1, model 0 (contract is 1 from the model)
	tolerance: 0
```

That line is what `diff` reports without panicking, which is the form you want when the comparison is one of several checks in a test:

```rust
let report = DifferentialHarness::new()
    .cases(64)
    .tolerance(1)
    .diff(token_amount(), |amount| client.calculate_fee(&amount), reference_fee_calculator);

assert!(report.is_match(), "{}", report.failure().unwrap().render(1));
```

`tolerance` is how far apart two numbers may be and still count as equal — absolute units, not a percentage. A fee the contract rounds half-up and the model truncates is one unit out on every odd amount, and `.tolerance(1)` says so without also forgiving a fee computed off the wrong basis. Non-numeric results ignore the tolerance: `false` is never one unit from `true`, and a `u128` above `i128::MAX` has no signed form to diff.

<div class="tk-callout">
  <span class="tk-callout__title">Smaller, not necessarily smallest</span>
  <p>Shrinking is <code>proptest</code>'s, and it walks down from a failing case to a smaller one that still fails, stopping at the first dead end. For a bug that only shows on odd amounts, that dead end can be a large odd number rather than <code>1</code>. Take the printed payload as the reproducer — paste it into an example-based test — or pin generation with <code>.seed(7)</code>, which replays the same inputs in the same order.</p>
</div>

### Comparing state as well as the return value

A call on a ledger can return the right number and write the wrong state, which is why a side may return an `ExecutionTrace` instead of a bare value. `with_state` names a storage key the call leaves behind, and the harness compares keys the same way it compares return values:

```rust
use soroban_testkit_generators::differential::{DifferentialHarness, ExecutionTrace};

let report = DifferentialHarness::new().diff(
    token_amount(),
    |amount| {
        let fee = client.calculate_fee(&amount);
        ExecutionTrace::new(fee).with_state("fee_total", client.fee_total())
    },
    |amount| {
        let fee = reference_fee_calculator(amount);
        ExecutionTrace::new(fee).with_state("fee_total", fee)
    },
);
```

Keys are strings you choose rather than symbols read off the ledger, which is what lets the model side record state without building an `Env`. Two consequences follow from that. A key only one side records is itself a divergence, reported against `<unset>`, so both sides have to name the same keys for the comparison to mean anything. And a contract that writes four keys needs four `with_state` calls: a trace that names one of them proves one of them.

<div class="tk-callout">
  <span class="tk-callout__title">Where to put one</span>
  <p>Differential tests belong next to the arithmetic rather than the access control: bonding curves, fee splits, AMM invariants, interest accrual. Anywhere a second, simpler implementation is cheaper to write than the bug is to find. Every case builds a real <code>Env</code> for the contract side, so narrow the strategy with <a href="../crates/generators.html">a generator</a> before you raise the case count.</p>
</div>

<hr class="tk-divider" />

## Stateful property-based testing with state machines

Stateful property testing generates *sequences* of contract calls and verifies invariants after each step. This catches bugs that only appear across multiple interacting transactions (e.g., deposit → lock → vote → withdraw → claim). Testkit provides a state machine harness built on `proptest::state_machine`.

### Defining a state machine

```rust
use soroban_testkit_generators::state_machine::{SorobanStateMachine, SorobanStateMachineRunner};
use soroban_sdk::{Env, Address, testutils::Address as _};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TokenModel {
    balances: std::collections::HashMap<Address, i128>,
    total_supply: i128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TokenAction {
    Mint { to: Address, amount: i128 },
    Transfer { from: Address, to: Address, amount: i128 },
    Burn { from: Address, amount: i128 },
}

struct TokenStateMachine {
    env: Env,
}

impl SorobanStateMachine for TokenStateMachine {
    type Action = TokenAction;
    type Model = TokenModel;
    type Context = TokenTestContext;

    fn generate_action(&self, model: &Self::Model) -> BoxedStrategy<Self::Action> {
        // Generate actions based on current model state
    }

    fn check_precondition(&self, model: &Self::Model, action: &Self::Action) -> bool {
        // Validate action against model state
    }

    fn execute(&self, context: &mut Self::Context, action: &Self::Action) -> Result<(), String> {
        // Execute action against real contract
    }

    fn apply_to_model(&self, model: &mut Self::Model, action: &Self::Action) {
        // Update expected model state
    }

    fn check_invariant(&self, model: &Self::Model, context: &Self::Context) -> TestCaseResult {
        // Verify invariant (e.g., total supply conservation)
    }

    fn initial_model(&self) -> Self::Model {
        TokenModel::default()
    }

    fn create_context(&self) -> Self::Context {
        TokenTestContext { ... }
    }
}
```

### Running the state machine

```rust
let machine = TokenStateMachine::new();
let runner = SorobanStateMachineRunner::new(machine).cases(100);
runner.run().unwrap();
```

Or use the simpler `prop_state_machine_test` function:

```rust
use soroban_testkit_generators::state_machine::prop_state_machine_test;

prop_state_machine_test(machine, 20, 100).unwrap();
```

### Key features

- **Automated invariant verification** after each transition
- **Precondition checking** to filter invalid actions
- **Automated shrinking** of failing action sequences to the minimal failing sequence
- **Sequences of length ≥ 50** supported
- **Integration example**: Token balance conservation invariant verification

## When to use property testing

<div class="tk-grid tk-grid--2">
  <div class="tk-card">
    <span class="tk-card__kicker">Invariants</span>
    <h3 class="tk-card__title">Arithmetic</h3>
    <p class="tk-card__body">Supply conservation, no overflow, and rounding that stays consistent across amounts.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Invariants</span>
    <h3 class="tk-card__title">State machines</h3>
    <p class="tk-card__body">A valid state remains reachable after any sequence of operations, not just the one you scripted.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Security</span>
    <h3 class="tk-card__title">Access control</h3>
    <p class="tk-card__body">Unauthorized callers fail regardless of the parameters they present.</p>
  </div>
  <div class="tk-card tk-card--accent">
    <span class="tk-card__kicker">Edges</span>
    <h3 class="tk-card__title">Boundaries</h3>
    <p class="tk-card__body">Zero amounts, maximum values and empty collections are generated rather than remembered.</p>
  </div>
</div>

<hr class="tk-divider" />

<p class="tk-muted">Strategy reference: <a href="../crates/generators.html">soroban-testkit-generators</a>.</p>
