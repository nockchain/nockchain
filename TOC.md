# Open crate guide

This guide lists the Rust crates in the open workspace and briefly explains what each one does.

| Crate | Path | What it does |
| --- | --- | --- |
| `equix-latency` | `open/crates/equix-latency` | Equi-X latency and benchmarking utilities. |
| `hoon` | `open/crates/hoon` | Rust support crate for Hoon-facing data and helpers. |
| `honk` | `open/crates/honk` | Native Rust Hoon compiler: parses Hoon ASTs, performs native `++ut` typechecking/minting, and emits JAM artifacts. |
| `hoonc` | `open/crates/hoonc` | Canonical Hoon compiler wrapper used for bootstrap builds and parity comparisons. |
| `hatch` | `open/crates/hatch` | Native Rust Hoon parser that turns source text into the typed Hoon AST consumed by `honk` and parser parity tests. |
| `kernels-open-bridge` | `open/crates/kernels/bridge` | Builds and packages the open bridge kernel artifact. |
| `kernels-open-dumb` | `open/crates/kernels/dumb` | Builds and packages the open dumb kernel artifact. |
| `kernels-open-miner` | `open/crates/kernels/miner` | Builds and packages the open miner kernel artifact. |
| `kernels-open-nockchain-peek` | `open/crates/kernels/nockchain-peek` | Builds and packages the open peek kernel artifact. |
| `kernels-open-wallet` | `open/crates/kernels/wallet` | Builds and packages the open wallet kernel artifact. |
| `nockapp` | `open/crates/nockapp` | Runtime framework for Nock applications: booting kernels, wiring drivers, managing nouns, and app execution support. |
| `nockapp-grpc` | `open/crates/nockapp-grpc` | gRPC integration layer for NockApp services. |
| `nockapp-grpc-proto` | `open/crates/nockapp-grpc-proto` | Generated protobuf bindings and shared gRPC message types. |
| `nockchain` | `open/crates/nockchain` | Main Nockchain node binary and application wiring. |
| `nockchain-api` | `open/crates/nockchain-api` | Public API types and helpers for interacting with Nockchain. |
| `nockchain-explorer-tui` | `open/crates/nockchain-explorer-tui` | Terminal UI for exploring Nockchain state. |
| `nockchain-libp2p-io` | `open/crates/nockchain-libp2p-io` | libp2p networking IO support for Nockchain. |
| `nockchain-math` | `open/crates/nockchain-math` | Shared math, numeric, and proof-related helpers. |
| `nockchain-peek` | `open/crates/nockchain-peek` | CLI/tooling for peeking Nockchain state. |
| `nockchain-types` | `open/crates/nockchain-types` | Shared Nockchain domain types, JAM fixtures, and serialization helpers. |
| `nockchain-wallet` | `open/crates/nockchain-wallet` | Wallet CLI and library support for keys, addresses, notes, and transactions. |
| `nockup` | `open/crates/nockup` | Project scaffolding and templates for building NockApp-based applications. |
| `ibig` | `open/crates/nockvm/rust/ibig` | Big-integer support used by the Nock VM. |
| `murmur3` | `open/crates/nockvm/rust/murmur3` | Murmur3 hashing implementation used by VM/runtime code. |
| `nockvm` | `open/crates/nockvm/rust/nockvm` | Rust Nock VM implementation and noun runtime. |
| `nockvm_macros` | `open/crates/nockvm/rust/nockvm_macros` | Procedural macros for Nock VM and noun code. |
| `noun-serde` | `open/crates/noun-serde` | Serde support for noun-backed data structures. |
| `noun-serde-derive` | `open/crates/noun-serde-derive` | Derive macros for `noun-serde`. |
| `raw-tx-checker` | `open/crates/raw-tx-checker` | Utility crate for checking raw transaction data. |
| `zkvm-jetpack` | `open/crates/zkvm-jetpack` | Open zkVM jetpack support code. |

Additional documentation for native compiler work lives in `../docs/native-compiler/`. Parser-only notes live in `docs/native-parser/`.
