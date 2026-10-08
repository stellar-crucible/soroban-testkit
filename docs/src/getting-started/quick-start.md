# Quick Start

This guide walks through writing a complete test using Testkit.

## Setup

```rust
use soroban_sdk::{Env, Address};
use testkit_fixtures::builder::TestContextBuilder;
use testkit_assert::events::EventMatcher;
use testkit_core::budget::BudgetSnapshot;
```

## Write a Test

```rust
#[test]
fn test_token_transfer() {
    // 1. Create a test context with pre-generated users
    let ctx = TestContextBuilder::new()
        .with_users(2)
        .build();

    let sender = &ctx.users[0];
    let receiver = &ctx.users[1];

    // 2. Register your contract
    let contract_id = ctx.env.register(MyContract, ());
    let client = MyContractClient::new(&ctx.env, &contract_id);

    // 3. Capture budget before invocation
    let before = BudgetSnapshot::capture(&ctx.env.budget());

    // 4. Invoke the function under test
    client.transfer(sender, receiver, &1000);

    // 5. Assert events were emitted
    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("Transfer")
        .assert_emitted();

    // 6. Check resource consumption
    let after = BudgetSnapshot::capture(&ctx.env.budget());
    let diff = before.diff(&after);
    assert!(diff.cpu_insns < 500_000);
}
```

## What's Happening

- **TestContextBuilder** eliminates manual `Env` creation, address generation, and auth mocking
- **EventMatcher** replaces raw tuple iteration over `env.events().all()`
- **BudgetSnapshot** surfaces resource costs that are invisible in standard tests

Each of these can be used independently — adopt what helps your workflow.
