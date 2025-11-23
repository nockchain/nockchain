# Nockchain AI Coding Guidelines

## Architecture Overview

Nockchain is a ZK-Proof of Work L1 blockchain implementing Nock (Urbit's VM) for programmable money. Core components:

- **nockvm**: Nock runtime with automatic persistence (see `crates/nockvm/README.md`)
- **hoonc**: Hoon compiler for compiling Hoon code to Nock (self-bootstrapped, see `crates/hoonc/README.md`)
- **nockchain**: Main node binary with libp2p networking and mining
- **nockapp**: Framework for Hoon-based apps/contracts
- **Hoon code**: Located in `hoon/` directory; apps in `hoon/apps/`, common libs in `hoon/common/`

Data flows: Hoon apps compile to Nock via hoonc, run on nockvm, integrated with Rust components via gRPC (nockapp-grpc).

## Developer Workflows

- **Build all**: `make build` (builds Hoon and Rust)
- **Install tools**: `make install-hoonc` and `make install-nockchain-wallet`
- **Run miner/node**: Use scripts in `scripts/` like `run_nockchain_miner.sh`
- **Test**: `cargo test --release` (Rust); Hoon tests via compiled apps
- **Debug Hoon**: Use `hoonc --arbitrary` for single files; check syntax errors in `hoon/apps/wallet/lib/utils.hoon` style
- **Bootstrap hoonc**: Run `cargo run --release hoon/apps/hoonc/hoonc.hoon hoon` to regenerate `bootstrap/hoonc.jam`

## Conventions and Patterns

- **Hoon imports**: Use `/+` for libs (e.g., `/+  wallet-types`), `/-` for sur (discouraged), `/=` for paths, `/*` for marked files
- **Rust-Hoon integration**: Hoon apps expose gates; Rust calls via nockapp-grpc
- **File structure**: Hoon files end `.hoon`; Rust crates in `crates/`; jams (precomputed Nock) in `jams/`
- **Error handling**: In Hoon, use `?.` for conditionals; Rust uses `anyhow` for errors
- **Version pinning**: `/?` in Hoon (ignored in hoonc)
- **Build cache**: hoonc reuses parse cache; clear with `--new` flag

## Examples

- Compile Hoon app: `hoonc hoon/apps/wallet/wallet.hoon hoon`
- Load lib in Hoon: `/+  tx-builder-v1` (from `hoon/apps/wallet/lib/`)
- Run test: `cargo test -p nockchain-types`
- Key generation: `nockchain-wallet keygen` (sets `MINING_PKH` in `.env`)

Reference: `README.md` for setup, `Makefile` for commands, `hoon/common/` for shared libs.
