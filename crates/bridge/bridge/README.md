# bridge runtime quickstart

Build and run the Nockchain/Base bridge and its monitoring client.

## prerequisites
- Rust toolchain via `rustup` (uses the workspace `rust-toolchain.toml`).
- Foundry 1.8.6 via official foundryup (also pinned by Bazel and the Nix shell):
  ```bash
  curl -L https://foundry.paradigm.xyz | bash
  ~/.foundry/bin/foundryup --install 1.8.6
  ```
- Config: copy `bridge-conf.example.toml` to
  `~/.nockapp/bridge/bridge-conf.toml` (or pass `--config-path`).

## quick run setup (from repo root)
From the repository root, install/update contract dependencies and build the
bridge runtime binary:

```bash
make assets/bridge.jam
make -C crates/bridge/bridge/contracts install  # installs or updates foundry and downloads contract dependencies
cargo build --profile release --bin bridge
```

## cross-compile for x86_64 linux

From the repository root:

```bash
make install-cargo-zigbuild
make zig-build-bridge
```

Default target is `x86_64-unknown-linux-gnu.2.39`.
Set `ZIGBUILD_TARGET` to a value compatible with your deployment host's glibc
version, for example:

```bash
make zig-build-bridge ZIGBUILD_TARGET=x86_64-unknown-linux-gnu.2.39
```

Output binary:

```bash
target/x86_64-unknown-linux-gnu/release/bridge
```

## nockchain bridge tui (operators only)
The TUI client can be launched from your local machine (for example, your
laptop or operator workstation); it does not need to run on the bridge node.
Your local machine must be connected to Tailscale so it can reach the bridge
node ingress gRPC endpoint.

From the repository root, build the TUI:

```bash
make build-nockchain-bridge-tui
```

Run the TUI:

```bash
# Uses default server 127.0.0.1:8001
./target/release/nockchain-bridge-tui

# Connect to a bridge node over Tailscale
./target/release/nockchain-bridge-tui --server <tailscale-ip>:8001
```

## running the node
```bash
./bridge  -c bridge_config_path
```
- `--start` will unfreeze stopped nodes
- `--new` requires a fresh data directory; omit it for restarts.
