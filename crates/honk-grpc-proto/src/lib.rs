#![allow(clippy::large_enum_variant)]
#![allow(clippy::unnecessary_fallible_conversions)]

pub mod v1 {
    tonic::include_proto!("honk.compiler.v1");
}

pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("honk_descriptor");
