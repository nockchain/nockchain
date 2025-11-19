//! gRPC server implementation for NockApp
//!
//! This crate provides a gRPC interface to NockApp, replacing the old socket-based
//! interface with modern RPC patterns for easier cross-language compatibility.

// Include the generated protobuf code
pub mod pb {
    pub mod common {
        pub mod v1 {
            tonic::include_proto!("nockchain.common.v1");
        }
        pub mod v2 {
            tonic::include_proto!("nockchain.common.v2");
        }
    }
    pub mod monitoring {
        pub mod v1 {
            tonic::include_proto!("nockchain.monitoring.v1");
        }
    }
    pub mod private {
        pub mod v1 {
            tonic::include_proto!("nockchain.private.v1");
        }
    }
    pub mod public {
        pub mod v1 {
            tonic::include_proto!("nockchain.public.v1");
        }
        pub mod v2 {
            tonic::include_proto!("nockchain.public.v2");
        }
    }

    pub const FILE_DESCRIPTOR_SET: &[u8] =
        tonic::include_file_descriptor_set!("nockapp_descriptor");
}

pub mod common;
pub mod v1;
pub mod v2;
