use thiserror::Error;

#[derive(Debug, Error)]
pub enum TestkitError {
    #[error("failed to read scenario: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse scenario yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("invalid scenario: {0}")]
    Invalid(String),
}
