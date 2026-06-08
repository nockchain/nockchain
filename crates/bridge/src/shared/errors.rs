use nockapp::CrownError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum BridgeError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Base bridge initialization failed: {0}")]
    BaseBridgeInit(String),

    #[error("Base bridge submission failed: {0}")]
    BaseBridgeSubmission(String),

    #[error("Base bridge monitoring failed: {0}")]
    BaseBridgeMonitoring(String),

    #[error("Base bridge query failed: {0}")]
    BaseBridgeQuery(String),

    #[error("Nockapp task failed: {0}")]
    NockappTask(String),

    #[error("Ack task failed: {0}")]
    AckTask(String),

    #[error("Invalid contract address: {0}")]
    InvalidContractAddress(String),

    #[error("Invalid private key: {0}")]
    InvalidPrivateKey(String),

    #[error("WebSocket connection failed: {0}")]
    WebSocketConnection(String),

    #[error("Contract interaction failed: {0}")]
    ContractInteraction(String),

    #[error("Event monitoring failed: {0}")]
    EventMonitoring(String),

    #[error("Value conversion failed: {0}")]
    ValueConversion(String),

    #[error("Bridge runtime error: {0}")]
    Runtime(String),

    #[error("Signature generation failed: {0}")]
    SignatureGeneration(String),

    #[error("Invalid signature format: {0}")]
    InvalidSignatureFormat(String),

    #[error("Invalid deposit log base: {0}")]
    InvalidDepositLogBase(String),

    #[error("Invalid deposit log entry: {0}")]
    InvalidDepositLogEntry(String),
}

impl From<anyhow::Error> for BridgeError {
    fn from(err: anyhow::Error) -> Self {
        BridgeError::Config(err.to_string())
    }
}

impl From<std::env::VarError> for BridgeError {
    fn from(err: std::env::VarError) -> Self {
        BridgeError::Config(format!("Environment variable error: {}", err))
    }
}

impl From<std::num::ParseIntError> for BridgeError {
    fn from(err: std::num::ParseIntError) -> Self {
        BridgeError::Config(format!("Parse error: {}", err))
    }
}

impl From<BridgeError> for CrownError {
    fn from(err: BridgeError) -> Self {
        CrownError::Unknown(err.to_string())
    }
}

impl From<CrownError> for BridgeError {
    fn from(err: CrownError) -> Self {
        BridgeError::NockappTask(err.to_string())
    }
}

impl From<serde_json::Error> for BridgeError {
    fn from(err: serde_json::Error) -> Self {
        BridgeError::Config(format!("JSON parsing error: {}", err))
    }
}

impl From<nockapp_grpc::NockAppGrpcError> for BridgeError {
    fn from(err: nockapp_grpc::NockAppGrpcError) -> Self {
        BridgeError::NockappTask(err.to_string())
    }
}

impl From<nockapp::NockAppError> for BridgeError {
    fn from(err: nockapp::NockAppError) -> Self {
        BridgeError::NockappTask(err.to_string())
    }
}

impl From<alloy::signers::local::LocalSignerError> for BridgeError {
    fn from(err: alloy::signers::local::LocalSignerError) -> Self {
        BridgeError::InvalidPrivateKey(err.to_string())
    }
}
