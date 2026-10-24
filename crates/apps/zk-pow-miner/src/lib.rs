//! `zk-pow-miner` — standalone block-mining binary for Nockchain's
//! ZK PoW (the `puzzle-nock` STARK puzzle).
//!
//! The miner is a separate OS process that:
//! 1. Connects to a running `nockchain` node over the node's private
//!    [`nockapp_grpc`] `NockAppService` (Peek/Poke + `WatchEffects`
//!    streaming subscription).
//! 2. Pokes `set-mining-key-advanced` + `enable-mining` to configure
//!    the kernel's coinbase payout and turn candidate-block generation on.
//! 3. Subscribes via `WatchEffects(head_filter=[b"mine-zk"])` to receive
//!    `[%mine-zk version commit target pow-len]` effects.
//! 4. For each candidate, dispatches mining attempts across a pool of
//!    [`Worker`]s. Serf workers search and prove with `assets/miner.jam`;
//!    CUDA workers search V5 nonces on GPUs and share a Serf prover pool
//!    for winners, unsupported proof versions, and long puzzles.
//! 5. On a successful proof, pokes the node back with the `%pow`
//!    command, which the node treats as a `heard-block` from the
//!    `%zk-pow-miner` wire source.
//!
//! Architecture overview:
//! ```text
//!     +------------+   gRPC   +------------------+
//!     |  nockchain |<---------|  zk-pow-miner    |
//!     |   (node)   |          |  +------------+  |
//!     |            |          |  | run loop   |  |
//!     |%mine-zk eff|--Watch-->|  | (NodeClient|  |
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
//!                             |  +------------+  |
//!                             |  | CUDA GPUs  |  |
//!                             |  | + shared   |  |
//!                             |  | Serf pool  |  |
//!                             |  +------------+  |
//!                             +------------------+
//! ```

#![cfg_attr(test, allow(clippy::unwrap_used))]
#[cfg(feature = "cuda")]
pub mod cuda;
pub mod pool;
pub mod run;
pub mod v5;
pub mod wire;
pub mod worker;
pub use pool::Pool;
pub use run::{run, MinerConfig, MinerError};
#[cfg(feature = "cuda")]
pub use run::{run_cuda, CudaConfig};
pub use wire::ZkPowMinerWire;
pub use worker::{MineResult, SerfWorker, Worker, WorkerError, WorkerId};
