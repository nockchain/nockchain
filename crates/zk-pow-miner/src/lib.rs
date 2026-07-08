//! `zk-pow-miner` — standalone block-mining binary for Nockchain's
//! ZK PoW (the `puzzle-nock` STARK puzzle).
//!
//! The miner is a separate OS process that:
//! 1. Connects to a running `nockchain` node over the node's private
//!    [`nockapp_grpc`] `NockAppService` (Peek/Poke + the new
//!    `WatchEffects` streaming subscription).
//! 2. Pokes `set-mining-key-advanced` + `enable-mining` to configure
//!    the kernel's coinbase payout and turn candidate-block generation on.
//! 3. Subscribes via `WatchEffects(head_filter=[b"mine"])` to receive
//!    `[%mine version commit target pow-len]` effects.
//! 4. For each candidate, dispatches mining attempts across a pool of
//!    [`Worker`]s — each one a `SerfThread` loaded with the miner kernel
//!    `assets/miner.jam` (vendored as `kernels-open-miner::KERNEL`).
//! 5. On a successful proof, pokes the node back with the `%pow`
//!    command, which the node treats as a `heard-block` from the
//!    `%miner` wire source (see `hoon/apps/dumbnet/inner.hoon`).
//!
//! Architecture overview:
//! ```text
//!     +------------+   gRPC   +------------------+
//!     |  nockchain |<---------|  zk-pow-miner    |
//!     |   (node)   |          |  +------------+  |
//!     |            |          |  | run loop   |  |
//!     | %mine eff  |--Watch-->|  | (NodeClient|  |
//!     |            |          |  |  ↔ Pool)   |  |
//!     |  %pow poke |<--Poke---|  |            |  |
//!     +------------+          |  +-----+------+  |
//!                             |        | dispatch|
//!                             |        v         |
//!                             |  +-----+------+  |
//!                             |  |  Pool      |  |
//!                             |  | (N workers)|  |
//!                             |  +-----+------+  |
//!                             |        |         |
//!                             |        v         |
//!                             |  +-----+------+  |
//!                             |  | SerfThread |  |
//!                             |  | + miner.jam|  |
//!                             |  +------------+  |
//!                             +------------------+
//! ```

pub mod pool;
pub mod run;
pub mod wire;
pub mod worker;

pub use pool::Pool;
pub use run::{run, MinerConfig, MinerError};
pub use wire::ZkPowMinerWire;
pub use worker::{MineResult, SerfWorker, Worker, WorkerError, WorkerId};
