use thiserror::Error;

pub type Result<T> = std::result::Result<T, NockAppGrpcError>;

#[derive(Error, Debug)]
pub enum NockAppGrpcError {
    #[error("NockApp error: {0}")]
    NockApp(#[from] nockapp::NockAppError),

    #[error("gRPC transport error: {0}")]
    Transport(#[from] tonic::transport::Error),

    #[error("gRPC status error: {0}")]
    Status(#[from] tonic::Status),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Peek operation failed")]
    PeekFailed,

    #[error("Poke operation failed")]
    PokeFailed,

    #[error("Timeout error")]
    Timeout,

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Serialization error: {0}")]
    Serialization(String),
}

impl From<NockAppGrpcError> for tonic::Status {
    fn from(err: NockAppGrpcError) -> Self {
        use NockAppGrpcError::*;

        use crate::pb::ErrorCode;

        let (code, message, error_code) = match &err {
            NockApp(nockapp::NockAppError::PeekFailed) => (
                tonic::Code::NotFound,
                "Peek operation failed".to_string(),
                ErrorCode::PeekFailed,
            ),
            NockApp(nockapp::NockAppError::PokeFailed) => (
                tonic::Code::InvalidArgument,
                "Poke operation failed".to_string(),
                ErrorCode::PokeFailed,
            ),
            NockApp(nockapp::NockAppError::Timeout) => (
                tonic::Code::DeadlineExceeded,
                "Operation timed out".to_string(),
                ErrorCode::Timeout,
            ),
            NockApp(e) => (
                tonic::Code::Internal,
                format!("NockApp error: {}", e),
                ErrorCode::NackappError,
            ),
            Transport(e) => (
                tonic::Code::Unavailable,
                format!("Transport error: {}", e),
                ErrorCode::InternalError,
            ),
            Status(status) => return status.clone(),
            InvalidRequest(msg) => (
                tonic::Code::InvalidArgument,
                msg.clone(),
                ErrorCode::InvalidRequest,
            ),
            PeekFailed => (
                tonic::Code::NotFound,
                "Peek operation failed".to_string(),
                ErrorCode::PeekFailed,
            ),
            PokeFailed => (
                tonic::Code::InvalidArgument,
                "Poke operation failed".to_string(),
                ErrorCode::PokeFailed,
            ),
            Timeout => (
                tonic::Code::DeadlineExceeded,
                "Operation timed out".to_string(),
                ErrorCode::Timeout,
            ),
            Internal(msg) => (tonic::Code::Internal, msg.clone(), ErrorCode::InternalError),
            Serialization(msg) => (
                tonic::Code::Internal,
                format!("Serialization error: {}", msg),
                ErrorCode::InternalError,
            ),
        };

        let status = tonic::Status::new(code, message);

        // Add structured error details
        let error_details = crate::pb::ErrorStatus {
            code: error_code as i32,
            message: status.message().to_string(),
            details: None,
        };

        let _details_bytes = prost::Message::encode_to_vec(&error_details);
        // Note: with_details is not available in tonic 0.14, so we'll just return the basic status
        status
    }
}
