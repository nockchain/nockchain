use std::{fmt, io};

use nockapp::NockAppError;

#[derive(Debug)]
pub enum WalletError {
    Io(io::Error),
    NockApp(NockAppError),
    Parse(String),
    Command(String),
}

impl fmt::Display for WalletError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WalletError::Io(e) => write!(f, "IO error: {}", e),
            WalletError::NockApp(e) => write!(f, "NockApp error: {}", e),
            WalletError::Parse(s) => write!(f, "Parse error: {}", s),
            WalletError::Command(s) => write!(f, "Command error: {}", s),
        }
    }
}

impl std::error::Error for WalletError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WalletError::Io(e) => Some(e),
            WalletError::NockApp(e) => Some(e),
            WalletError::Parse(_) => None,
            WalletError::Command(_) => None,
        }
    }
}

impl From<io::Error> for WalletError {
    fn from(err: io::Error) -> Self {
        WalletError::Io(err)
    }
}

impl From<NockAppError> for WalletError {
    fn from(err: NockAppError) -> Self {
        WalletError::NockApp(err)
    }
}

impl From<WalletError> for NockAppError {
    fn from(err: WalletError) -> Self {
        match err {
            WalletError::NockApp(e) => e,
            WalletError::Io(e) => NockAppError::IoError(e),
            WalletError::Parse(_) => NockAppError::OtherError(String::from("Wallet Parse error")),
            WalletError::Command(_) => {
                NockAppError::OtherError(String::from("Wallet Command error"))
            }
        }
    }
}

impl From<nockapp::CrownError> for WalletError {
    fn from(err: nockapp::CrownError) -> Self {
        WalletError::Parse(err.to_string())
    }
}
