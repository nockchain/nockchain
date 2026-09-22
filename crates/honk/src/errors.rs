use std::fmt;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, CompilerError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompilerErrorKind {
    Parse,
    UnsupportedExpr,
    Backend,
    Decode,
    Noun,
}

impl fmt::Display for CompilerErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse => write!(f, "parse error"),
            Self::UnsupportedExpr => write!(f, "unsupported Hoon expression"),
            Self::Backend => write!(f, "backend error"),
            Self::Decode => write!(f, "decode error"),
            Self::Noun => write!(f, "noun error"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompilerErrorLocation {
    pub file: Option<String>,
    pub start_byte: Option<usize>,
    pub end_byte: Option<usize>,
    pub start_line: Option<u64>,
    pub start_col: Option<u64>,
    pub end_line: Option<u64>,
    pub end_col: Option<u64>,
}

/// Owned editor-only fact captured while type-checking a debug-spotted Hoon
/// node. The summary contains no noun or compiler-arena references and may
/// safely cross the compiler actor boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompilerSemanticFact {
    pub location: CompilerErrorLocation,
    pub type_summary: String,
}

/// Owned editor-only provenance for a compiler-resolved arm reference.
/// Locations and the arm name are detached from compiler nouns and arenas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompilerResolutionFact {
    pub use_location: CompilerErrorLocation,
    pub definition_location: CompilerErrorLocation,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompilerErrorMetadata {
    pub location: Option<CompilerErrorLocation>,
}

impl fmt::Display for CompilerErrorLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (
            self.file.as_deref(),
            self.start_line,
            self.start_col,
            self.end_line,
            self.end_col,
        ) {
            (Some(file), Some(start_line), Some(start_col), Some(end_line), Some(end_col))
                if (start_line, start_col) != (end_line, end_col) =>
            {
                write!(f, "{file}:{start_line}:{start_col}-{end_line}:{end_col}")
            }
            (Some(file), Some(line), Some(col), _, _) => write!(f, "{file}:{line}:{col}"),
            (None, Some(start_line), Some(start_col), Some(end_line), Some(end_col))
                if (start_line, start_col) != (end_line, end_col) =>
            {
                write!(f, "line {start_line}:{start_col}-{end_line}:{end_col}")
            }
            (None, Some(line), Some(col), _, _) => write!(f, "line {line}:{col}"),
            (Some(file), _, _, _, _) => write!(f, "{file}"),
            (None, _, _, _, _) => Ok(()),
        }
    }
}

impl fmt::Display for CompilerErrorMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(location) = &self.location {
            write!(f, " at {location}")?;
        }
        Ok(())
    }
}

impl CompilerErrorMetadata {
    pub fn with_location(mut self, location: CompilerErrorLocation) -> Self {
        self.location = Some(location);
        self
    }
}

#[derive(Debug, Error)]
pub enum CompilerError {
    #[error("parse error: {0}")]
    Parse(String),
    #[error("unsupported Hoon expression: {0}")]
    UnsupportedExpr(String),
    #[error("backend error: {0}")]
    Backend(String),
    #[error("decode error: {0}")]
    Decode(String),
    #[error("noun error: {0}")]
    Noun(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{kind}: {message}{metadata}")]
    Detailed {
        kind: CompilerErrorKind,
        message: String,
        metadata: CompilerErrorMetadata,
    },
}

impl CompilerError {
    pub fn kind(&self) -> Option<CompilerErrorKind> {
        match self {
            Self::Parse(_) => Some(CompilerErrorKind::Parse),
            Self::UnsupportedExpr(_) => Some(CompilerErrorKind::UnsupportedExpr),
            Self::Backend(_) => Some(CompilerErrorKind::Backend),
            Self::Decode(_) => Some(CompilerErrorKind::Decode),
            Self::Noun(_) => Some(CompilerErrorKind::Noun),
            Self::Detailed { kind, .. } => Some(*kind),
            Self::Io(_) => None,
        }
    }

    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Parse(message)
            | Self::UnsupportedExpr(message)
            | Self::Backend(message)
            | Self::Decode(message)
            | Self::Noun(message) => Some(message.as_str()),
            Self::Detailed { message, .. } => Some(message.as_str()),
            Self::Io(_) => None,
        }
    }

    pub fn metadata(&self) -> Option<&CompilerErrorMetadata> {
        match self {
            Self::Detailed { metadata, .. } => Some(metadata),
            _ => None,
        }
    }

    pub fn is_unsupported_expr(&self) -> bool {
        self.kind() == Some(CompilerErrorKind::UnsupportedExpr)
    }

    pub fn with_metadata(self, metadata: CompilerErrorMetadata) -> Self {
        match self {
            Self::Parse(message) => Self::Detailed {
                kind: CompilerErrorKind::Parse,
                message,
                metadata,
            },
            Self::UnsupportedExpr(message) => Self::Detailed {
                kind: CompilerErrorKind::UnsupportedExpr,
                message,
                metadata,
            },
            Self::Backend(message) => Self::Detailed {
                kind: CompilerErrorKind::Backend,
                message,
                metadata,
            },
            Self::Decode(message) => Self::Detailed {
                kind: CompilerErrorKind::Decode,
                message,
                metadata,
            },
            Self::Noun(message) => Self::Detailed {
                kind: CompilerErrorKind::Noun,
                message,
                metadata,
            },
            Self::Detailed { kind, message, .. } => Self::Detailed {
                kind,
                message,
                metadata,
            },
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CompilerError, CompilerErrorKind, CompilerErrorLocation, CompilerErrorMetadata};

    #[test]
    fn with_metadata_wraps_plain_error() {
        let metadata = CompilerErrorMetadata::default().with_location(CompilerErrorLocation {
            file: Some("foo.hoon".to_string()),
            start_byte: Some(12),
            end_byte: Some(20),
            start_line: Some(2),
            start_col: Some(3),
            end_line: Some(2),
            end_col: Some(11),
        });
        let error = CompilerError::Parse("bad parse".to_string()).with_metadata(metadata.clone());

        assert_eq!(
            error.to_string(),
            "parse error: bad parse at foo.hoon:2:3-2:11"
        );

        match error {
            CompilerError::Detailed {
                kind,
                message,
                metadata: got,
            } => {
                assert_eq!(kind, CompilerErrorKind::Parse);
                assert_eq!(message, "bad parse");
                assert_eq!(got, metadata);
            }
            other => panic!("expected detailed error, got {other:?}"),
        }
    }

    #[test]
    fn unsupported_detection_handles_plain_and_structured() {
        let plain = CompilerError::UnsupportedExpr("x".to_string());
        assert!(plain.is_unsupported_expr());

        let detailed = plain.with_metadata(CompilerErrorMetadata::default());
        assert!(detailed.is_unsupported_expr());
    }
}
