//! gRPC server implementation for NockApp
//!
//! This crate provides a gRPC interface to NockApp, replacing the old socket-based
//! interface with modern RPC patterns for easier cross-language compatibility.

// Include the generated protobuf code
pub mod pb {
    tonic::include_proto!("nockapp.v1");

    pub const FILE_DESCRIPTOR_SET: &[u8] =
        tonic::include_file_descriptor_set!("nockapp_descriptor");
}

pub mod client;
pub mod driver;
pub mod error;
pub mod server;
#[cfg(test)]
mod tests;
pub mod wire_conversion;

#[cfg(feature = "client")]
pub use client::NockAppGrpcClient;
pub use driver::grpc_server_driver;
pub use error::{NockAppGrpcError, Result};
pub use server::NockAppGrpcServer;
