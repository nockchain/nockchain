// Allow architectural design choices that would be disruptive to change
#![allow(clippy::too_many_arguments)]
#![allow(clippy::result_large_err)]
#![allow(clippy::enum_variant_names)]
#![allow(clippy::items_after_test_module)]

mod behaviour; // Nockchain libp2p behavior type
pub mod config; // Configurable values for the Nockchain libp2p driver
pub mod driver; // Nockchain libp2p driver for NockApp
mod key_fair_queue; // Fair queue for key-value pairs, allowing replacement
mod messages; // Messages exchanged between Nockchain nodes
pub mod metrics; // Nockchain libp2p metrics (gnort)
mod p2p_state; // State maintained by the Nockchain libp2p driver
pub mod p2p_util; // Utilities for the Nockchain libp2p driver
pub mod tip5_util; // tip5 <> string conversion
mod tracked_join_set; // Custom task set which allows tracking named tasks
mod traffic_cop; // Network traffic prioritization

#[cfg(test)]
mod cbor_tests;
