// Allow architectural design choices that would be disruptive to change
#![allow(clippy::too_many_arguments)]
#![allow(clippy::result_large_err)]
#![allow(clippy::enum_variant_names)]
#![allow(clippy::items_after_test_module)]

mod backend;
mod behaviour; // Nockchain libp2p behavior type
pub mod config; // Configurable values for the Nockchain network driver
pub mod driver; // Nockchain network driver for NockApp
mod ip_block; // IP-level connection gating (deny banned IPs)
mod key_fair_queue; // Fair queue for key-value pairs, allowing replacement
mod messages; // Messages exchanged between Nockchain nodes
pub mod metrics; // Nockchain network metrics (gnort)
mod p2p_state; // State maintained by the Nockchain network driver
pub mod p2p_util; // Utilities for the Nockchain network driver
mod peer_policy;
pub mod peer_stats; // Read-only per-peer req-res stats snapshots
#[doc(hidden)]
pub mod test_support; // Reusable req-res integration harness support
pub mod tip5_util; // tip5 <> string conversion
mod tracked_join_set; // Custom task set which allows tracking named tasks
mod traffic_cop; // Network traffic prioritization
pub mod transport; // Bounded backend command/event contract
pub mod types; // Transport-neutral network value types

#[cfg(test)]
mod cbor_tests;
