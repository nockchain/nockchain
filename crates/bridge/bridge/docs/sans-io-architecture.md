# Bridge Sans-IO Architecture

## What this doc is for
A readable description of the bridge's sans-io architecture.

- We separate **thinking** from **doing**.
- “Thinking” means deciding what should happen next.
- “Doing” means talking to the network, contracts, kernel, or disk.

That split makes behavior easier to reason about, easier to test, and less
fragile when we change integrations.

## The main idea

Most bridge loops follow the same pattern:

1. Read current state (tip heights, next nonce, queued work, etc.)
2. Decide what action makes sense
3. Execute that action
4. Repeat

In this architecture:

- step 2 is a small, predictable decision function
- step 3 is handled by shell/integration code

So we can test decisions without needing real RPC endpoints, timers, or sleeps.

## A quick example

This is roughly what a loop does:

```rust
// Pseudocode
let input = gather_input();
let action = planner(input);
execute(action).await;
```

And for signing/posting, we also expose a single “tick” call:

```rust
// Pseudocode
let outcome = signing_tick_once(&context, &mut state, input).await;
```

That lets tests drive one step at a time and assert exactly what happened.

## How the bridge is split up

### Decision code (the “thinking” side)

- `src/core/base_observer.rs`
- `src/core/nock_observer.rs`
- `src/core/signing.rs`
- `src/core/posting.rs`

These files decide things like:

- “wait for more confirmations”
- “fetch this block range”
- “skip, not my turn”
- “submit now”

### Integration code (the “doing” side)

- `src/ethereum.rs`
- `src/nockchain.rs`
- `src/runtime.rs`
- `src/main.rs`

These files do the real work:

- call RPC/gRPC
- send/receive runtime events
- submit contract transactions
- run timers/retries/stop handling

## Why this helps

### Easier to test

We can test decisions in tight loops without flaky timing behavior.

### Safer changes

We can replace transport details (RPC client, watcher wiring, etc.) without
rewriting core behavior.

### Clearer failures

When something goes wrong, it is easier to tell whether:

- the decision was wrong, or
- the network call failed.

## What “single-tick” gives us

For signing and posting, we have bounded tick APIs that do one unit of work.
This is useful in tests and debugging.

Examples in code:

- `signing_tick_once(...)`
- `posting_tick_once(...)`

Related tests:

- `tests/loop_tick_tests.rs`
- `tests/loop_policy_shell_tests.rs`
- `tests/core_runner_integration.rs`

## What stays the same

This architecture does **not** change bridge protocol semantics by itself.
The kernel protocol and end-to-end flow are still the same:

- observers feed runtime
- runtime talks to kernel
- kernel emits effects
- drivers execute effects

The change is mostly about making Rust-side behavior easier to understand and
verify.

## Current state

Already in place:

- planner/executor split across observer/signing/posting paths
- policy-based loop wrappers for cadence/retry/stop behavior
- deterministic tick tests for core branches

Still planned:

- deeper runtime/kernel harness replay tooling for even stronger transcript-style
  validation
- final parity soak closure in staging

## Where to read next

- `open/crates/bridge/bridge/docs/architecture.md` for full system flow
- `open/crates/bridge/bridge/src/core/` for decision logic
- `open/crates/bridge/bridge/src/runtime.rs` for loop orchestration
- `open/crates/bridge/bridge/tests/loop_tick_tests.rs` for deterministic examples
