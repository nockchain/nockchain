use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Metadata about a cached package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedPackage {
    pub name: String,
    pub version_spec: String, // e.g., "k414", "commit:abc123", "^1.2.0"
    pub commit: String,       // Exact commit hash
    pub cached_at: u64,       // Unix timestamp
    pub source_url: String,
}

/// Cache index tracking all cached packages
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CacheIndex {
    pub packages: HashMap<String, Vec<CachedPackage>>,
}

/// Manages the Nockup package cache
pub struct PackageCache {
    root: PathBuf, // ~/.nockup/cache/
}

impl PackageCache {
    /// Create a new PackageCache, creating directories if needed
    pub fn new() -> Result<Self> {
        let home =
            dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;
        let root = home.join(".nockup").join("cache");

        // Create cache directories
        std::fs::create_dir_all(root.join("git"))?;
        std::fs::create_dir_all(root.join("packages"))?;
        std::fs::create_dir_all(root.join("registry"))?;

        Ok(Self { root })
    }

    /// Create a PackageCache with custom root (for testing)
    pub fn with_root(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(root.join("git"))?;
        std::fs::create_dir_all(root.join("packages"))?;
        std::fs::create_dir_all(root.join("registry"))?;

        Ok(Self { root })
    }

    /// Get the root cache directory
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Get the git cache directory (for GitFetcher)
    pub fn git_dir(&self) -> PathBuf {
        self.root.join("git")
    }

    /// Get the packages cache directory
    pub fn packages_dir(&self) -> PathBuf {
        self.root.join("packages")
    }

    /// Get the registry cache directory
    pub fn registry_dir(&self) -> PathBuf {
        self.root.join("registry")
    }

    /// Get the path for a specific package version
    /// Format: ~/.nockup/cache/packages/<name>/<version-spec>/
    pub fn package_path(&self, name: &str, version_spec: &str) -> PathBuf {
        let safe_spec = self.sanitize_version_spec(version_spec);
        self.packages_dir().join(name).join(safe_spec)
    }

    /// Check if a package is cached
    pub fn is_cached(&self, name: &str, version_spec: &str) -> bool {
        self.package_path(name, version_spec).exists()
    }

    /// Cache a package from a git repo path
    pub async fn cache_package(
        &self,
        name: &str,
        version_spec: &str,
        commit: &str,
        source_url: &str,
        source_path: &Path,
    ) -> Result<PathBuf> {
        let target_path = self.package_path(name, version_spec);

        // Create parent directory
        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Copy source to cache
        self.copy_directory(source_path, &target_path).await?;

        let cached_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .context("System clock is before UNIX_EPOCH")?
            .as_secs();

        // Update cache index
        self.add_to_index(CachedPackage {
            name: name.to_string(),
            version_spec: version_spec.to_string(),
            commit: commit.to_string(),
            cached_at,
            source_url: source_url.to_string(),
        })
        .await?;

        Ok(target_path)
    }

    /// Load the cache index
    pub async fn load_index(&self) -> Result<CacheIndex> {
        let index_path = self.root.join("cache-index.json");

        if !index_path.exists() {
            return Ok(CacheIndex::default());
        }

        let contents = tokio::fs::read_to_string(&index_path).await?;
        let index: CacheIndex =
            serde_json::from_str(&contents).context("Failed to parse cache index")?;

        Ok(index)
    }

    /// Save the cache index
    pub async fn save_index(&self, index: &CacheIndex) -> Result<()> {
        let index_path = self.root.join("cache-index.json");
        let contents = serde_json::to_string_pretty(index)?;
        tokio::fs::write(&index_path, contents).await?;
        Ok(())
    }

    /// Add a package to the cache index
    async fn add_to_index(&self, package: CachedPackage) -> Result<()> {
        let mut index = self.load_index().await?;

        index
            .packages
            .entry(package.name.clone())
            .or_insert_with(Vec::new)
            .push(package);

        self.save_index(&index).await?;
        Ok(())
    }

    /// List all cached packages
    pub async fn list_cached(&self) -> Result<Vec<CachedPackage>> {
        let index = self.load_index().await?;
        let mut all_packages: Vec<CachedPackage> = index.packages.into_values().flatten().collect();

        all_packages.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(all_packages)
    }

    /// Find cached package by name and version spec
    pub async fn find_cached(
        &self,
        name: &str,
        version_spec: &str,
    ) -> Result<Option<CachedPackage>> {
        let index = self.load_index().await?;

        if let Some(packages) = index.packages.get(name) {
            for pkg in packages {
                if pkg.version_spec == version_spec {
                    return Ok(Some(pkg.clone()));
                }
            }
        }

        Ok(None)
    }

    /// Clean the cache (remove all cached packages)
    pub async fn clean(&self) -> Result<()> {
        // Remove packages directory
        if self.packages_dir().exists() {
            tokio::fs::remove_dir_all(self.packages_dir()).await?;
            tokio::fs::create_dir_all(self.packages_dir()).await?;
        }

        // Clear index
        self.save_index(&CacheIndex::default()).await?;

        Ok(())
    }

    /// Prune old cached packages (keep only latest N versions per package)
    pub async fn prune(&self, keep_versions: usize) -> Result<()> {
        let mut index = self.load_index().await?;

        for (name, packages) in &mut index.packages {
            if packages.len() <= keep_versions {
                continue;
            }

            // Sort by cached_at timestamp (newest first)
            packages.sort_by(|a, b| b.cached_at.cmp(&a.cached_at));

            // Remove old versions
            let to_remove: Vec<CachedPackage> = packages.drain(keep_versions..).collect();

            for pkg in to_remove {
                let path = self.package_path(&pkg.name, &pkg.version_spec);
                if path.exists() {
                    tokio::fs::remove_dir_all(&path).await?;
                }
                println!("  Pruned {}@{}", name, pkg.version_spec);
            }
        }

        self.save_index(&index).await?;
        Ok(())
    }

    /// Get cache statistics
    pub async fn stats(&self) -> Result<CacheStats> {
        let index = self.load_index().await?;

        let total_packages = index.packages.values().map(|v| v.len()).sum();
        let unique_packages = index.packages.len();

        // Calculate total size (approximate)
        let total_size = self.calculate_directory_size(&self.packages_dir()).await?;

        Ok(CacheStats {
            total_packages,
            unique_packages,
            total_size_bytes: total_size,
        })
    }

    // Private helper methods

    /// Sanitize version spec for use in filesystem path
    fn sanitize_version_spec(&self, spec: &str) -> String {
        spec.replace(['/', ':', '@'], "_")
            .replace('^', "caret_")
            .replace('~', "tilde_")
            .replace('>', "gt_")
            .replace('<', "lt_")
    }

    /// Recursively copy a directory
    fn copy_directory<'a>(
        &'a self,
        src: &'a Path,
        dst: &'a Path,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + 'a>> {
        Box::pin(async move {
            if !src.exists() {
                anyhow::bail!("Source directory does not exist: {}", src.display());
            }

            tokio::fs::create_dir_all(dst).await?;

            let mut entries = tokio::fs::read_dir(src).await?;

            while let Some(entry) = entries.next_entry().await? {
                let src_path = entry.path();
                let file_name = entry.file_name();
                let dst_path = dst.join(&file_name);

                // Skip .git directories
                if file_name == ".git" {
                    continue;
                }

                if src_path.is_dir() {
                    self.copy_directory(&src_path, &dst_path).await?;
                } else {
                    tokio::fs::copy(&src_path, &dst_path).await?;
                }
            }

            Ok(())
        })
    }

    /// Calculate total size of a directory (recursive)
    fn calculate_directory_size<'a>(
        &'a self,
        path: &'a Path,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u64>> + 'a>> {
        Box::pin(async move {
            if !path.exists() {
                return Ok(0);
            }

            let mut total_size = 0u64;
            let mut entries = tokio::fs::read_dir(path).await?;

            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                let metadata = tokio::fs::metadata(&path).await?;

                if path.is_dir() {
                    total_size += self.calculate_directory_size(&path).await?;
                } else {
                    total_size += metadata.len();
                }
            }

            Ok(total_size)
        })
    }
}

/// Cache statistics
#[derive(Debug)]
pub struct CacheStats {
    pub total_packages: usize,
    pub unique_packages: usize,
    pub total_size_bytes: u64,
}

impl CacheStats {
    pub fn total_size_mb(&self) -> f64 {
        self.total_size_bytes as f64 / (1024.0 * 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_version_spec() {
        let cache =
            PackageCache::with_root(PathBuf::from("/tmp/test")).expect("Failed to init cache");

        assert_eq!(cache.sanitize_version_spec("k414"), "k414");
        assert_eq!(
            cache.sanitize_version_spec("commit:abc123"),
            "commit_abc123"
        );
        assert_eq!(cache.sanitize_version_spec("^1.2.0"), "caret_1.2.0");
        assert_eq!(cache.sanitize_version_spec("~1.2.3"), "tilde_1.2.3");
        assert_eq!(cache.sanitize_version_spec("@tag:v1.0"), "_tag_v1.0");
    }

    #[test]
    fn test_package_path() {
        let cache =
            PackageCache::with_root(PathBuf::from("/tmp/test")).expect("Failed to init cache");
        let path = cache.package_path("arvo", "k414");

        assert_eq!(path, PathBuf::from("/tmp/test/packages/arvo/k414"));
    }
}
