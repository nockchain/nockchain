use std::fs;
use std::path::{Path, PathBuf};

use bridge::shared::e2e_environment::BaseSepoliaE2eManifest;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct BaseE2eEnvironment {
    manifest: BaseSepoliaE2eManifest,
}

impl BaseE2eEnvironment {
    pub fn from_json(input: &str) -> Result<Self, BaseEnvironmentError> {
        let manifest = BaseSepoliaE2eManifest::from_json(input)
            .map_err(|error| BaseEnvironmentError::InvalidManifest(error.to_string()))?;
        Ok(Self { manifest })
    }

    pub fn from_manifest(manifest: BaseSepoliaE2eManifest) -> Result<Self, BaseEnvironmentError> {
        manifest
            .validate()
            .map_err(|error| BaseEnvironmentError::InvalidManifest(error.to_string()))?;
        Ok(Self { manifest })
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, BaseEnvironmentError> {
        let path = path.as_ref();
        let input = fs::read_to_string(path).map_err(|source| BaseEnvironmentError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_json(&input)
    }

    pub fn manifest(&self) -> &BaseSepoliaE2eManifest {
        &self.manifest
    }
}

#[derive(Debug, Error)]
pub enum BaseEnvironmentError {
    #[error("failed to read Base E2E environment manifest at {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid Base E2E environment manifest: {0}")]
    InvalidManifest(String),
}
