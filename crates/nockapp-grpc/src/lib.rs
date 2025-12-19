//! gRPC server implementation for NockApp
//!
//! This crate provides a gRPC interface to NockApp, replacing the old socket-based
//! interface with modern RPC patterns for easier cross-language compatibility.

// Allow clippy lints for architectural patterns in gRPC services
#![allow(clippy::needless_borrow)]
#![allow(clippy::needless_borrows_for_generic_args)]
#![allow(clippy::clone_on_copy)]
#![allow(clippy::unnecessary_cast)]
#![allow(clippy::io_other_error)]
#![allow(clippy::redundant_guards)]
#![allow(clippy::single_match)]
#![allow(clippy::useless_conversion)]
#![allow(clippy::type_complexity)]
#![allow(clippy::while_let_loop)]
#![allow(clippy::option_map_or_none)]
#![allow(clippy::module_inception)]
#![allow(clippy::result_large_err)]
#![allow(clippy::bind_instead_of_map)]
// Allow unwrap in test code
#![cfg_attr(test, allow(clippy::unwrap_used))]

// Include the generated protobuf code

pub mod error;
pub mod services;
#[cfg(test)]
mod tests;
pub mod v1;
pub mod v2;
pub mod wire_conversion;

pub use error::{NockAppGrpcError, Result};
pub use nockapp_grpc_proto::pb;
pub use nockapp_grpc_proto::v1::convert;
pub use services::{private_nockapp, public_nockchain};

// Backcompat re-export: allow imports like `nockapp_grpc::driver::...`
pub mod driver {
    pub use crate::services::public_nockchain::v1::driver::{
        grpc_listener_driver, grpc_server_driver,
    };
}
