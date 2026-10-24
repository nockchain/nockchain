use nockchain_math::crypto::cheetah::CheetahError;
use thiserror::Error;

use crate::pb::common::v1::ErrorStatus;

/// Helper trait for extracting required fields from protobuf-generated types.
pub trait Required<T> {
    fn required(self, kind: &'static str, field: &'static str) -> Result<T, ConversionError>;
}

impl<T> Required<T> for Option<T> {
    fn required(self, kind: &'static str, field: &'static str) -> Result<T, ConversionError> {
        self.ok_or(ConversionError::MissingField(kind, field))
    }
}

#[derive(Debug, Error)]
#[error("grpc error code={code}: {message} ({details:?})")]
pub struct RPCErrorStatus {
    pub code: i32,
    pub message: String,
    pub details: Option<String>,
}

impl From<ErrorStatus> for RPCErrorStatus {
    fn from(status: ErrorStatus) -> Self {
        RPCErrorStatus {
            code: status.code,
            message: status.message,
            details: status.details,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConversionError {
    #[error("cheetah error: {0}")]
    Cheetah(#[from] CheetahError),
    #[error("{0} is missing field: {1}")]
    MissingField(&'static str, &'static str),
    #[error("Invalid value: {0}")]
    Invalid(&'static str),
}
