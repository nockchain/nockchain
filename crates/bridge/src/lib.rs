// Allow unwrap in unit-test-only code paths; production code is still linted with `-D clippy::unwrap_used`.
#![cfg_attr(test, allow(clippy::unwrap_used))]

#[cfg(all(feature = "snmalloc", feature = "malloc"))]
compile_error!("features `snmalloc` and `malloc` are mutually exclusive");

pub mod bridge_status;
pub mod config;
pub mod deposit_log;
pub mod errors;
pub mod ethereum;
pub mod grpc;
pub mod health;
pub mod ingress;
pub mod metrics;
pub mod nockchain;
pub mod proposal_cache;
pub mod proposer;
pub mod runtime;
pub mod schema;
pub mod signing;
pub mod status;
pub mod stop;
pub mod tui;
pub mod tui_api;
pub mod tui_client;
pub mod types;
