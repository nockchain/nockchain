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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patches: Vec<PackagePatch>,
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
        files: Option<Vec<String>>,
        kelvin: Option<String>,
    },
}

/// A post-install patch a package asks nockup to apply to the consumer
/// project after symlinking. See `crate::patches` for the engine.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PackagePatch {
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(flatten)]
    pub op: PatchOp,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PatchOp {
    Overwrite {
        content: String,
    },
    ReplacePattern {
        pattern: String,
        replacement: String,
        #[serde(default = "default_true")]
        once: bool,
    },
    EnsureDependency {
        table: String,
        name: String,
        value: toml::Value,
    },
    EnsurePatch {
        registry: String,
        entries: BTreeMap<String, toml::Value>,
    },
}

fn default_true() -> bool {
    true
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applied_patches: Vec<AppliedPatch>,
}

/// One entry per patch the install engine actually wrote (or re-confirmed
/// as already in shape). The hash lets a later install detect that the
/// user has since hand-edited the file.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppliedPatch {
    pub index: usize,
    pub file: String,
    pub content_hash: String,
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
