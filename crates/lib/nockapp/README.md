# NockApp

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
- `NounSlab` keeps each active region's allocation frontier within its backing
  block, including the one-past-end position. Capacity checks compare the
  requested word count with the remaining words before advancing the frontier;
  an empty region has no capacity, and requests exceeding capacity grow the
  region using the existing backing-block allocation policy.
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

## Checks

- `cargo check -p nockapp`

The slab capacity boundary test uses a borrowed word buffer and exercises the
same private predicate used by region growth, without allocating backing blocks:

```bash
cargo +nightly-2026-04-03 miri test -p nockapp --lib \
  noun::slab::tests::region_capacity_boundaries -- --exact
```

Run the slab module tests normally to exercise backing-block growth, checkpoint
rewind, and frame reclamation:

```bash
cargo test -p nockapp --lib noun::slab::tests
```

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

## HTTP driver ports

When the HTTP driver runs in local mode (`HTTPS_DOMAIN` is unset or a local
domain such as `localhost`, `127.*`, `192.168.*`, or `*.local`), it binds to
`127.0.0.1:8080` by default. For an embedding binary, override it with:

```sh
HTTP_PORT=3000 nockchain
```

`HTTP_PORT` must be a valid port number (0–65535); invalid values fail startup.
Production mode (a non-local `HTTPS_DOMAIN`) uses port 80 for ACME challenges and
443 for HTTPS, unaffected by `HTTP_PORT`.

## Tracy profiling

The default `tracing-tracy` feature enables Tracy integration. Collection begins
when a compatible [Tracy GUI](https://github.com/wolfpld/tracy) connects; no special
application CLI flags are needed. The GUI and Rust client versions must match the
[client compatibility table](https://github.com/nagisa/rust_tracy_client#version-support-table).
Disable the Cargo feature if profiling support is not wanted.

Instrumentation combines Rust `tracing` spans with NockVM/Hoon spans. Rust spans
have source locations; Hoon spans use arm paths without Rust source locations.
Native stack samples complement those spans. Leave CPU capacity for the Serf
thread, kernel, and profiler; connecting to a fully saturated miner can materially
affect performance.

To build the GUI from a Tracy source checkout, install its platform dependencies
(FreeType, debuginfod, Wayland, D-Bus, libxkbcommon, OpenGL, Meson, CMake, Git, and
Wayland protocols on Linux), then run:

```sh
cmake -B profiler/build -S profiler -DCMAKE_BUILD_TYPE=Release
cmake --build profiler/build --parallel
```

Local applications appear in the GUI's connect menu. **Never expose the Tracy
listener to networks you do not control end-to-end.** Keep remote listeners
private and use SSH forwarding, then connect the GUI to `127.0.0.1:8087`:

```sh
ssh -L 127.0.0.1:8087:127.0.0.1:8086 <node-host>
```

Port 8087 avoids the GUI's own use of 8086. Native stack sampling is not supported
on macOS; a Linux container with suitable permissions is an alternative. On Linux,
stack sampling may require relaxing ptrace/perf restrictions:

```sh
echo 0 | sudo tee /proc/sys/kernel/yama/ptrace_scope
echo 1 | sudo tee /proc/sys/kernel/perf_event_paranoid
```

These changes weaken host security and last until reboot; change only what is
needed on a controlled profiling host, restore the original values afterward,
and restart the application if required. Instrumentation does not require native
stack sampling.
