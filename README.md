# Nockchain

Nockchain is a Proof-of-Work blockchain whose Hoon consensus kernel runs on NockVM.
This repository contains the node, wallet, miners, proof systems, and regression
harnesses.

Crates are grouped by role: `crates/apps` contains node, wallet, API, and miner
applications; `crates/bridge` contains bridge services and development support;
`crates/tools` contains compilers, developer tools, and test harnesses; and
`crates/lib` contains shared runtime, protocol, math, and proof libraries.
Package and binary names are independent of these directory groups.

## Setup

Install [rustup](https://rustup.rs/). On Debian/Ubuntu, install the native build
dependencies, then run these commands from this repository's root:

```sh
sudo apt update
sudo apt install clang llvm-dev libclang-dev make protobuf-compiler
cp .env_example .env
make install-hoonc
export PATH="$HOME/.cargo/bin:$PATH"
make install-nockchain-wallet
make install-nockchain
```

If building `hoonc` directly with Cargo, use `--release`. Linux requires memory
overcommit for the runtime's virtual-memory allocation:

```sh
echo 'vm.overcommit_memory=1' | sudo tee /etc/sysctl.d/99-overcommit.conf
sudo sysctl -p /etc/sysctl.d/99-overcommit.conf
```

Before connecting to a network, check the [protocol activation and deployment
requirements](docs/protocol/README.md). For a non-mining node:

```sh
bash scripts/run_nockchain_node.sh
```

Use a fresh working directory for launch, not state left by fakenet testing.
See the [node guide](crates/apps/nockchain/README.md#operations) for mining, local
fakenet, networking, logs, and troubleshooting; the
[wallet guide](crates/apps/nockchain-wallet/README.md) covers key generation, backups,
and transaction commands. Never commit `.env`, keys, or node state.

## Documentation

- [Protocol rules and upgrade history](docs/protocol/README.md), including current activation heights and [coordinated deployment](docs/AI_POW_CUTOVER.md).
- [Architecture](docs/architecture/README.md) and [transaction engine](docs/architecture/tx-engine/00-overview.md).
- [NockApp runtime](crates/lib/nockapp/README.md), [NockVM](crates/lib/nockvm/README.md), and [public API deployment](crates/apps/nockchain-api/README.md).
- [PMA operator FAQ](docs/pma/faq.md) and [durability/recovery](docs/pma/DURABILITY-OPERATIONS.md).
- [Bridge operations](crates/bridge/bridge/docs/node-runbook.md), [governance](crates/bridge/bridge/docs/governance.md), and [batch multisig](crates/apps/nockchain-wallet/batch-multisig.md).
- [ZK miner](crates/apps/zk-pow-miner/README.md), [AI miner](crates/apps/ai-pow-miner/README.md), and [Roswell proof/test harness](crates/tools/roswell/README.md).
- [Native Hoon compiler](docs/native-compiler/README.md), [parser](docs/native-parser/README.md), and [NockApp scaffolding](crates/tools/nockup/README.md).
- [Dependency maintenance and build constraints](docs/dependencies.md).
- [Project vision](docs/vision/nockchain-eli5.md), [ZK Compute Network](docs/vision/zk-compute-network-eli5.md), and [AI Compute Network](docs/vision/ai-compute-network-eli5.md). Proposed designs do not override active protocol rules.

## License

Dual-licensed under [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your
option. Unless explicitly stated otherwise, contributions intentionally submitted
for inclusion are licensed under those same terms.
