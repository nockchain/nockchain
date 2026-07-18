# NockApp

Status: Active

Canonical/Legacy: Canonical (Tier 1 scoped authority for NockApp runtime behavior; application and protocol authority remain with the embedded kernel)

`nockapp` is the Rust runtime framework for persistent Nock state machines. It
boots a kernel noun, serializes `peek` and `poke` interactions, dispatches kernel
effects to drivers, and coordinates event persistence, shutdown, and telemetry.
Nockchain, Roswell, wallets, compilers, and other applications embed this same
runtime contract.

## Place in the system

The kernel owns application semantics. `nockapp` owns the host lifecycle around
it:

```text
drivers and RPCs <-> NockApp handle <-> ordered kernel peeks/pokes
                                         |
                                  effects and durable state
                                         |
                                      NockVM
```

`nockvm` evaluates nouns and manages persistent memory. Drivers translate
external IO into kernel wires and translate effects back into IO. `nockapp`
does not define Nockchain consensus; the embedded Hoon kernel does.

## Canonical Scope

This document is Tier 1 canonical for:
- `nockapp` runtime interface expectations (`Kernel`, `poke`, `peek`, effect handling).
- Developer/operator usage guidance for this crate's runtime behavior.
- Logging/runtime configuration knobs exposed by this crate.

This document is NOT canonical for:
- protocol/consensus rules (use [`PROTOCOL.md`](../../PROTOCOL.md)).
- cross-crate architecture boundaries (use [`ARCHITECTURE.md`](../../ARCHITECTURE.md)).

## Maintained invariants and trust boundaries

- Kernel pokes are serialized through the runtime; drivers do not mutate kernel
  state directly.
- Effects are produced by completed kernel events. The broadcast effect bus is
  bounded and live-only: lagging subscribers may miss notifications and must
  recover from current state.
- Persistence and event-log ordering must not acknowledge a durable transition
  that cannot be recovered after restart.
- A driver owns its wire vocabulary. Sharing transport infrastructure does not
  make two wire sources interchangeable.
- Nouns crossing thread, persistence, or RPC boundaries retain canonical JAM
  representation and explicit ownership.
- Cancellation and shutdown stop new work without exposing partially applied
  kernel transitions as committed state.
- Native jets are accelerators or explicitly mandatory protocol components.
  Their registration and semantics must match the Hoon hint used by the kernel.

NockApp correctness depends on deterministic NockVM execution, the embedded
kernel's own invariants, canonical noun serialization, and the durability
ordering of the configured persistence backend. Network authentication,
consensus validity, and application authorization belong to the embedding
application and its drivers.

## Failure Modes And Limits

- This crate is alpha-grade and interface details may evolve quickly.
- Examples may lag implementation unless updated in the same PR as interface changes.
- This doc cannot resolve protocol disputes; if runtime behavior appears to conflict with protocol semantics, protocol sources win.

## Verification Contract

When runtime-interface behavior changes in `nockapp`, update this doc in the same change.

Minimum validation:
- `make -C open docs-check`
- `cargo check -p nockapp`

<br>

## Get Started

To test compiling a Nock kernel using the `hoonc` command-line Hoon compiler, run the following commands from the repository root:

```
make install-hoonc
hoonc hoon/apps/dumbnet/outer.hoon hoon
```

For large builds, the rust stack might overflow. To get around this, increase the stack size by setting: `RUST_MIN_STACK=838860`.

## Building NockApps

The `nockapp` library is the primary framework for building NockApps. It provides a simple interface to a `Kernel`: a Nock core which can make state transitions with effects (via the `poke()` method) and allow inspection of its state via the `peek()` method.

For compiling Hoon to Nock, we're also including a pre-release of `hoonc`: a NockApp for the Hoon compiler. `hoonc` can compile Hoon to Nock as a batch-mode command-line process, without the need to spin up an interactive Urbit ship. It is intended both for developer workflows and for CI. `hoonc` is also our first example NockApp. More are coming!

## Logging Configuration

### Basic Usage

```bash
# nockapp is a library crate, configure logging on the binary that embeds it
RUST_LOG=info <nockapp-based-binary> <args>

# Use minimal log format
MINIMAL_LOG_FORMAT=true <nockapp-based-binary> <args>
```

### TLDR

Use `MINIMAL_LOG_FORMAT=true` for compact logging format

### Minimal Log Format Features

The minimal log format (`MINIMAL_LOG_FORMAT=true`) provides:
- Single-letter colored log levels (T, D, I, W, E)
- Simplified timestamps in HH:MM:SS format
- Abbreviated module paths (e.g., 'nockapp::kernel::boot' becomes '[cr] kernel::boot')
- Special handling for slogger messages (colored by log level)

### Environment Variables

The following environment variables can be used to configure logging:

```bash
# Set log level
RUST_LOG="nockapp::kernel=trace" <nockapp-based-binary> <args>

# Enable minimal log format
MINIMAL_LOG_FORMAT=true <nockapp-based-binary> <args>

# Combine environment variables
RUST_LOG="nockapp::kernel=trace" MINIMAL_LOG_FORMAT=true <nockapp-based-binary> <args>
```
