//! Shared substrate for the external miner binaries.
//!
//! Both the ZK-PoW (dumb-puzzle) and AI-PoW miners run in their own
//! process and talk to a `nockchain` node over the node's private
//! `NockAppService` gRPC. This crate carries everything those
//! binaries share — except the wire vocabulary, which is per-crate
//! (each miner uses its own `SOURCE` on the wire, e.g.
//! `zk_pow_miner::ZkPowMinerWire` with `SOURCE = "zk-pow-miner"`).
//!
//! - [`MiningKeyConfig`] / [`MiningPkhConfig`] — CLI-parseable
//!   mining reward configuration.
//! - [`MiningCandidate`] — decoded form of a `%mine` effect emitted by
//!   the node's kernel.
//! - [`NodeClient`] — high-level wrapper over `PrivateNockAppGrpcClient`.
//!   Typed helpers (`set_mining_key`, `enable_mining`,
//!   `submit_mined_block`) take a `WireRepr` parameter so callers
//!   supply their crate's source.

pub mod candidate;
pub mod key_config;
pub mod node_client;

pub use candidate::{CandidateDecodeError, MiningCandidate, MiningCandidateKind};
pub use key_config::{MiningKeyConfig, MiningPkhConfig};
pub use node_client::{NodeClient, NodeClientError};
