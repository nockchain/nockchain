use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use toml;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct HoonPackage {
    pub package: PackageMeta,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<BTreeMap<String, DependencySpec>>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct PackageMeta {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_commit: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct NockAppManifest {
    pub package: PackageMeta,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_commit: Option<String>,

    #[serde(default)]
    pub dependencies: BTreeMap<String, DependencySpec>,

    // Optional local section (rare)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
}

impl NockAppManifest {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            anyhow::bail!("Manifest file not found: {}", path.display());
        }
        let content = std::fs::read_to_string(path)?;
        let manifest = toml::from_str(&content)?;
        Ok(manifest)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum DependencySpec {
    // "1.0"
    Simple(String),
    // version = "1.0"
    Version {
        version: String,
    },
    // "k409" etc.
    Full {
        version: Option<String>,
        git: Option<String>,
        commit: Option<String>,
        tag: Option<String>,
        branch: Option<String>,
        path: Option<String>,
        files: Option<Vec<String>>, // Specific files to extract (e.g., ["seq", "test"])
        kelvin: Option<String>,
    },
}

// nockapp.lock format – always exact commit hashes
#[derive(Debug, Serialize, Deserialize)]
pub struct NockAppLock {
    pub package: Vec<LockedPackage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LockedPackage {
    pub name: String,
    // k414", "commit:abc123", "^1.0", etc.
    pub version: String,
    pub source: LockSource,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LockSource {
    #[serde(rename = "git")]
    Git {
        url: String,
        commit: String,
        path: Option<String>,
    },
    #[serde(rename = "path")]
    Path { path: String },
}

impl HoonPackage {
    pub fn load(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(path)?;
        let pkg = toml::from_str(&content)?;
        Ok(Some(pkg))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

impl NockAppLock {
    pub fn load(path: &Path) -> Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            Ok(toml::from_str(&content)?)
        } else {
            Ok(NockAppLock { package: vec![] })
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}
