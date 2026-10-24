/// Package registry system using typhoon registry format
/// Fetches registry from https://github.com/sigilante/typhoon
use std::collections::HashMap;
use std::sync::RwLock;

use anyhow::{anyhow, Context, Result};
use once_cell::sync::Lazy;
use serde::Deserialize;

use crate::git_fetcher::GitSpec;

#[derive(Debug, Clone)]
pub struct RegistryEntry {
    pub git_url: String,
    pub path: Option<String>, // Path in repo to fetch from (e.g., "pkg/arvo/sys")
    pub install_path: Option<String>, // Path to install to (e.g., "sys")
    pub file: Option<String>, // Specific file to extract (e.g., "zuse.hoon")
}

/// Typhoon registry TOML format structures
#[derive(Debug, Clone, Deserialize)]
pub struct RegistryToml {
    #[serde(default)]
    pub workspace: HashMap<String, Workspace>,
    #[serde(default)]
    pub package: Vec<Package>,
    #[serde(default)]
    pub alias: Vec<Alias>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Workspace {
    pub git_url: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub description: Option<String>,
    pub root_path: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Package {
    pub name: String,
    pub workspace: String,
    pub path: String,
    pub file: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Alias {
    pub name: String,
    pub target: String,
}

/// Well-known packages registry
static REGISTRY: Lazy<HashMap<&'static str, RegistryEntry>> = Lazy::new(|| {
    let mut m = HashMap::new();

    // Standard Urbit libraries from urbit/urbit - single files
    // path: where to fetch from in repo (e.g., "pkg/arvo/sys")
    // install_path: where to install to (e.g., "sys")
    m.insert(
        "urbit/zuse",
        RegistryEntry {
            git_url: "https://github.com/urbit/urbit".to_string(),
            path: Some("pkg/arvo/sys".to_string()),
            install_path: Some("sys".to_string()),
            file: Some("zuse.hoon".to_string()),
        },
    );

    m.insert(
        "urbit/lull",
        RegistryEntry {
            git_url: "https://github.com/urbit/urbit".to_string(),
            path: Some("pkg/arvo/sys".to_string()),
            install_path: Some("sys".to_string()),
            file: Some("lull.hoon".to_string()),
        },
    );

    m.insert(
        "urbit/hoon",
        RegistryEntry {
            git_url: "https://github.com/urbit/urbit".to_string(),
            path: Some("pkg/arvo/sys".to_string()),
            install_path: Some("sys".to_string()),
            file: Some("hoon.hoon".to_string()),
        },
    );

    m.insert(
        "urbit/arvo",
        RegistryEntry {
            git_url: "https://github.com/urbit/urbit".to_string(),
            path: Some("pkg/arvo/sys".to_string()),
            install_path: Some("sys".to_string()),
            file: Some("arvo.hoon".to_string()),
        },
    );

    // Urbit lib files - also single files
    // These install to "lib/" directory
    m.insert(
        "map",
        RegistryEntry {
            git_url: "https://github.com/urbit/urbit".to_string(),
            path: Some("pkg/arvo/lib".to_string()),
            install_path: Some("lib".to_string()),
            file: Some("map.hoon".to_string()),
        },
    );

    m.insert(
        "bits",
        RegistryEntry {
            git_url: "https://github.com/urbit/urbit".to_string(),
            path: Some("pkg/arvo/lib".to_string()),
            install_path: Some("lib".to_string()),
            file: Some("bits.hoon".to_string()),
        },
    );

    m.insert(
        "list",
        RegistryEntry {
            git_url: "https://github.com/urbit/urbit".to_string(),
            path: Some("pkg/arvo/lib".to_string()),
            install_path: Some("lib".to_string()),
            file: Some("list.hoon".to_string()),
        },
    );

    m.insert(
        "maplist",
        RegistryEntry {
            git_url: "https://github.com/urbit/urbit".to_string(),
            path: Some("pkg/arvo/lib".to_string()),
            install_path: Some("lib".to_string()),
            file: Some("maplist.hoon".to_string()),
        },
    );

    m.insert(
        "math",
        RegistryEntry {
            git_url: "https://github.com/urbit/urbit".to_string(),
            path: Some("pkg/arvo/lib".to_string()),
            install_path: Some("lib".to_string()),
            file: Some("math.hoon".to_string()),
        },
    );

    m.insert(
        "mapset",
        RegistryEntry {
            git_url: "https://github.com/urbit/urbit".to_string(),
            path: Some("pkg/arvo/lib".to_string()),
            install_path: Some("lib".to_string()),
            file: Some("mapset.hoon".to_string()),
        },
    );

    m.insert(
        "set",
        RegistryEntry {
            git_url: "https://github.com/urbit/urbit".to_string(),
            path: Some("pkg/arvo/lib".to_string()),
            install_path: Some("lib".to_string()),
            file: Some("set.hoon".to_string()),
        },
    );

    m.insert(
        "tiny",
        RegistryEntry {
            git_url: "https://github.com/urbit/urbit".to_string(),
            path: Some("pkg/arvo/lib".to_string()),
            install_path: Some("lib".to_string()),
            file: Some("tiny.hoon".to_string()),
        },
    );

    // Nockchain packages - no file restriction, will use all .hoon files
    m.insert(
        "nockchain",
        RegistryEntry {
            git_url: "https://github.com/nockchain/nockchain".to_string(),
            path: None,
            install_path: None,
            file: None,
        },
    );

    m
});

/// Cached online registry
static ONLINE_REGISTRY: Lazy<RwLock<Option<RegistryToml>>> = Lazy::new(|| RwLock::new(None));

const REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/sigilante/typhoon/master/registry.toml";

/// Fetch and parse the online registry (blocking - use spawn_blocking in async context)
fn fetch_registry_sync() -> Result<RegistryToml> {
    let response =
        reqwest::blocking::get(REGISTRY_URL).context("Failed to fetch registry from GitHub")?;

    let content = response
        .text()
        .context("Failed to read registry response")?;

    let registry: RegistryToml =
        toml::from_str(&content).context("Failed to parse registry TOML")?;

    Ok(registry)
}

/// Get the online registry (with caching) - async wrapper around blocking fetch
async fn get_online_registry() -> Result<RegistryToml> {
    // Try to read from cache first
    {
        let cache = ONLINE_REGISTRY
            .read()
            .map_err(|err| anyhow!("Failed to read registry cache: {err}"))?;
        if let Some(ref registry) = *cache {
            return Ok(registry.clone());
        }
    }

    // Fetch and cache (spawn blocking task to avoid blocking async runtime)
    let registry = tokio::task::spawn_blocking(fetch_registry_sync)
        .await
        .context("Failed to spawn blocking task")?
        .context("Failed to fetch registry")?;

    {
        let mut cache = ONLINE_REGISTRY
            .write()
            .map_err(|err| anyhow!("Failed to write registry cache: {err}"))?;
        *cache = Some(registry.clone());
    }

    Ok(registry)
}

/// Resolve an alias to its target package name
fn resolve_alias(name: &str, registry: &RegistryToml) -> String {
    for alias in &registry.alias {
        if alias.name == name {
            return alias.target.clone();
        }
    }
    name.to_string()
}

/// Look up a package in the registry (tries online registry first, falls back to hardcoded)
pub async fn lookup(name: &str) -> Option<RegistryEntry> {
    // Try online registry first
    if let Ok(registry) = get_online_registry().await {
        // Resolve aliases
        let resolved_name = resolve_alias(name, &registry);

        // Find the package
        if let Some(package) = registry.package.iter().find(|p| p.name == resolved_name) {
            // Look up workspace info
            if let Some(workspace) = registry.workspace.get(&package.workspace) {
                // Concatenate root_path + path to get full repository path for fetching
                // e.g., root_path="pkg/arvo", path="sys" -> fetch from "pkg/arvo/sys"
                // But install_path is just "sys" (the package path)
                let entry = RegistryEntry {
                    git_url: workspace.git_url.clone(),
                    path: Some(format!("{}/{}", workspace.root_path, package.path)),
                    install_path: Some(package.path.clone()),
                    file: Some(package.file.clone()),
                };
                return Some(entry);
            }
        }
    }

    // Fall back to hardcoded registry
    REGISTRY.get(name).cloned()
}

/// Get the dependencies of a package from the registry
pub async fn get_dependencies(name: &str) -> Vec<String> {
    // Try online registry first
    if let Ok(registry) = get_online_registry().await {
        // Resolve aliases
        let resolved_name = resolve_alias(name, &registry);

        // Find the package
        if let Some(package) = registry.package.iter().find(|p| p.name == resolved_name) {
            return package.dependencies.clone();
        }
    }

    // No dependencies found
    Vec::new()
}

/// Convert a registry entry to a GitSpec with version info
pub fn to_git_spec(entry: &RegistryEntry, tag: Option<String>, branch: Option<String>) -> GitSpec {
    GitSpec {
        url: entry.git_url.clone(),
        commit: None,
        tag,
        branch,
        path: entry.path.clone(),
        install_path: entry.install_path.clone(),
        file: entry.file.clone(),
    }
}
