use noun_serde::NounDecodeError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExternalError {
    #[error("unknown error: {0}")]
    UnknownError(anyhow::Error),
    #[error("conversion error: {0}")]
    ConversionError(String),
    // Add other common error variants as needed
}

#[derive(Debug, Error)]
pub enum CrownError<T = ExternalError> {
    #[error("external")]
    External(T),
    #[error("mutex error")]
    MutexError,
    #[error("invalid kernel input")]
    InvalidKernelInput,
    #[error("unknown effect")]
    UnknownEffect,
    #[error("io error: {0}")]
    IOError(#[from] std::io::Error),
    #[error("Crown NounError: {0}")]
    Noun(#[from] NounError),
    #[error("{0}")]
    InterpreterError(#[from] SwordError),
    #[error("kernel error")]
    KernelError(Option<nockvm::noun::Noun>),
    #[error("{0}")]
    Utf8FromError(#[from] std::string::FromUtf8Error),
    #[error("{0}")]
    Utf8Error(#[from] std::str::Utf8Error),
    #[error("newt error")]
    NewtError,
    #[error("newt")]
    Newt(#[from] anyhow::Error),
    #[error("boot error")]
    BootError,
    #[error("Serf load error")]
    SerfLoadError,
    #[error("work bail")]
    WorkBail,
    #[error("peek bail")]
    PeekBail,
    #[error("work swap")]
    WorkSwap,
    #[error("tank error")]
    TankError,
    #[error("play bail")]
    PlayBail,
    #[error("queue error")]
    QueueRecv(yaque::TryRecvError),
    #[error("save error: {0}")]
    SaveError(String),
    #[error("try from int error: {0}")]
    IntError(#[from] std::num::TryFromIntError),
    #[error("join error: {0}")]
    JoinError(#[from] tokio::task::JoinError),
    #[error("decode error")]
    DecodeError(#[from] bincode::error::DecodeError),
    #[error("encode error")]
    EncodeError(#[from] bincode::error::EncodeError),
    #[error("state jam format error: the state jam file format is not recognized")]
    StateJamFormatError,
    #[error("unknown error: {0}")]
    Unknown(String),
    #[error("conversion error: {0}")]
    ConversionError(#[from] ConversionError),
    #[error("unknown error")]
    UnknownError(anyhow::Error),
    #[error("queue")]
    QueueError(#[from] QueueErrorWrapper),
    #[error("Serf MPSC error")]
    SerfMPSCError(),
    #[error("oneshot channel error")]
    OneshotChannelError(#[from] tokio::sync::oneshot::error::RecvError),
}

impl<C> From<tokio::sync::mpsc::error::SendError<crate::kernel::form::SerfAction<C>>>
    for CrownError
{
    fn from(_: tokio::sync::mpsc::error::SendError<crate::kernel::form::SerfAction<C>>) -> Self {
        CrownError::SerfMPSCError()
    }
}

impl From<CrownError> for NounDecodeError {
    fn from(e: CrownError) -> Self {
        NounDecodeError::Custom(e.to_string())
    }
}

#[derive(Debug)]
pub struct QueueErrorWrapper(pub yaque::TrySendError<Vec<u8>>);

#[derive(Debug, Error)]
pub struct SwordError(pub nockvm::interpreter::Error);

impl std::fmt::Display for SwordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sword Error: {:?}", self.0)
    }
}

impl<C> From<nockvm::interpreter::Error> for CrownError<C> {
    fn from(e: nockvm::interpreter::Error) -> Self {
        CrownError::InterpreterError(SwordError(e))
    }
}

impl<C> From<nockvm::jets::JetErr> for CrownError<C> {
    fn from(e: nockvm::jets::JetErr) -> Self {
        CrownError::InterpreterError(SwordError(nockvm::interpreter::Error::from(e)))
    }
}

impl std::fmt::Display for QueueErrorWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "queue error: {}", self.0)
    }
}

impl std::error::Error for QueueErrorWrapper {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

impl<C> From<yaque::TrySendError<Vec<u8>>> for CrownError<C> {
    fn from(e: yaque::TrySendError<Vec<u8>>) -> Self {
        CrownError::QueueError(QueueErrorWrapper(e))
    }
}

#[derive(Debug, Error)]
#[error("conversion error: {0}")]
pub enum ConversionError {
    TooBig(String),
}

#[derive(Debug, Error)]
pub struct NounError(pub nockvm::noun::Error);

impl std::fmt::Display for NounError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Noun Error: {}", self.0)
    }
}

impl<C> From<nockvm::noun::Error> for CrownError<C> {
    fn from(e: nockvm::noun::Error) -> Self {
        CrownError::Noun(NounError(e))
    }
}

impl<C, T, E: Into<CrownError<C>>> IntoCrownError<C, T> for core::result::Result<T, E> {
    fn nockapp(self) -> core::result::Result<T, CrownError<C>> {
        match self {
            Ok(val) => Ok(val),
            Err(e) => Err(e.into()),
        }
    }
}

pub trait IntoCrownError<C, T> {
    fn nockapp(self) -> core::result::Result<T, CrownError<C>>;
}

pub type Result<V, E = CrownError<ExternalError>> = std::result::Result<V, E>;
