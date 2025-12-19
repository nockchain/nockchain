use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::process::Command;

/// Specification for a Git repository to fetch
#[derive(Debug, Clone)]
pub struct GitSpec {
    pub url: String,
    pub commit: Option<String>,
    pub tag: Option<String>,
    pub branch: Option<String>,
    pub path: Option<String>, // Subdir within repo to fetch from (e.g., "pkg/arvo/sys")
    pub install_path: Option<String>, // Subdir to install to (e.g., "sys")
    pub file: Option<String>, // Specific file to extract (e.g., "zuse.hoon")
}

/// Handles Git repository fetching and management
pub struct GitFetcher {
    cache_dir: PathBuf, // ~/.nockup/cache/git/
}

impl GitFetcher {
    /// Create a new GitFetcher with the given cache directory
    pub fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    /// Fetch a repository according to the spec, returning the local path
    pub async fn fetch(&self, spec: &GitSpec) -> Result<PathBuf> {
        // Determine target ref (commit > tag > branch > default)
        let target_ref = self.determine_target_ref(spec).await?;

        // Create cache path based on URL and commit hash
        let repo_path = self.get_repo_cache_path(&spec.url, &target_ref);

        // Check if already cached
        if repo_path.exists() {
            return Ok(repo_path);
        }

        // Clone the repository
        self.clone_repo(spec, &repo_path, &target_ref).await?;

        Ok(repo_path)
    }

    /// Resolve a tag or branch to a commit hash
    pub async fn resolve_ref(&self, url: &str, ref_name: &str) -> Result<String> {
        // Use git ls-remote to get commit hash without cloning
        let output = Command::new("git")
            .args(["ls-remote", url, ref_name])
            .output()
            .await
            .context("Failed to run git ls-remote")?;

        if !output.status.success() {
            anyhow::bail!(
                "Failed to resolve ref '{}' in {}: {}",
                ref_name,
                url,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let commit = stdout
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().next())
            .ok_or_else(|| anyhow::anyhow!("No commit found for ref '{}'", ref_name))?;

        Ok(commit.to_string())
    }

    /// Get commit hash for HEAD of a branch
    pub async fn resolve_branch(&self, url: &str, branch: &str) -> Result<String> {
        let ref_name = format!("refs/heads/{}", branch);
        self.resolve_ref(url, &ref_name).await
    }

    /// Get commit hash for a tag
    pub async fn resolve_tag(&self, url: &str, tag: &str) -> Result<String> {
        let ref_name = format!("refs/tags/{}", tag);
        self.resolve_ref(url, &ref_name).await
    }

    /// Checkout a specific commit in an already-cloned repo
    pub async fn checkout_commit(&self, repo_path: &Path, commit: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["checkout", commit])
            .current_dir(repo_path)
            .output()
            .await
            .context("Failed to checkout commit")?;

        if !output.status.success() {
            anyhow::bail!(
                "Failed to checkout commit {}: {}",
                commit,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(())
    }

    /// Fetch a subdirectory from a repo using sparse checkout
    pub async fn fetch_subdir(&self, spec: &GitSpec, subdir: &str) -> Result<PathBuf> {
        let target_ref = self.determine_target_ref(spec).await?;
        let repo_path = self.get_repo_cache_path(&spec.url, &target_ref);

        if repo_path.exists() {
            return Ok(repo_path.join(subdir));
        }

        // Clone with sparse checkout
        self.clone_sparse(spec, &repo_path, &target_ref, subdir)
            .await?;

        Ok(repo_path.join(subdir))
    }

    // Private helper methods

    /// Determine which ref to use (commit > tag > branch > default)
    async fn determine_target_ref(&self, spec: &GitSpec) -> Result<String> {
        if let Some(ref commit) = spec.commit {
            // If commit is specified, use it directly
            Ok(commit.clone())
        } else if let Some(ref tag) = spec.tag {
            // Resolve tag to commit
            self.resolve_tag(&spec.url, tag).await
        } else if let Some(ref branch) = spec.branch {
            // Resolve branch to commit
            self.resolve_branch(&spec.url, branch).await
        } else {
            // Default to HEAD of main/master
            match self.resolve_branch(&spec.url, "main").await {
                Ok(commit) => Ok(commit),
                Err(_) => self.resolve_branch(&spec.url, "master").await,
            }
        }
    }

    /// Generate cache path from URL and commit hash
    fn get_repo_cache_path(&self, url: &str, commit: &str) -> PathBuf {
        // Hash the URL to create a safe directory name
        let url_hash = self.hash_url(url);

        // Short commit hash (first 12 chars)
        let short_commit = &commit[..commit.len().min(12)];

        self.cache_dir.join(url_hash).join(short_commit)
    }

    /// Hash a URL to create a safe directory name
    fn hash_url(&self, url: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        url.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    /// Clone a repository (full clone with depth=1 for efficiency)
    async fn clone_repo(&self, spec: &GitSpec, target_path: &Path, commit: &str) -> Result<()> {
        // Create parent directory
        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Clone with depth=1 for the specific commit (if possible)
        // Note: Some git servers don't support fetching arbitrary commits with depth=1,
        // so we do a full clone and then checkout
        let output = Command::new("git")
            .arg("clone")
            .arg(&spec.url)
            .arg(target_path.as_os_str())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("Failed to clone repository")?;

        if !output.status.success() {
            anyhow::bail!(
                "Failed to clone {}: {}",
                spec.url,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Checkout the specific commit
        self.checkout_commit(target_path, commit).await?;

        Ok(())
    }

    /// Clone with sparse checkout for a specific subdirectory
    async fn clone_sparse(
        &self,
        spec: &GitSpec,
        target_path: &Path,
        commit: &str,
        subdir: &str,
    ) -> Result<()> {
        // Create parent directory
        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Initialize repo
        Command::new("git")
            .args(["init"])
            .current_dir(target_path)
            .output()
            .await?;

        // Configure sparse checkout
        Command::new("git")
            .args(["config", "core.sparseCheckout", "true"])
            .current_dir(target_path)
            .output()
            .await?;

        // Set sparse checkout paths
        let sparse_file = target_path.join(".git/info/sparse-checkout");
        tokio::fs::write(&sparse_file, format!("{}\n", subdir)).await?;

        // Add remote
        Command::new("git")
            .args(["remote", "add", "origin", &spec.url])
            .current_dir(target_path)
            .output()
            .await?;

        // Fetch and checkout
        Command::new("git")
            .args(["fetch", "--depth=1", "origin", commit])
            .current_dir(target_path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await?;

        self.checkout_commit(target_path, commit).await?;

        Ok(())
    }

    /// List all tags in a remote repository
    pub async fn list_tags(&self, url: &str) -> Result<Vec<String>> {
        let output = Command::new("git")
            .args(["ls-remote", "--tags", url])
            .output()
            .await
            .context("Failed to list tags")?;

        if !output.status.success() {
            anyhow::bail!(
                "Failed to list tags for {}: {}",
                url,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let tags: Vec<String> = stdout
            .lines()
            .filter_map(|line| {
                line.split_whitespace()
                    .nth(1)
                    .and_then(|ref_name| ref_name.strip_prefix("refs/tags/"))
                    .map(|tag| tag.to_string())
            })
            .collect();

        Ok(tags)
    }

    /// Check if git is available on the system
    pub async fn check_git_available() -> Result<()> {
        let output = Command::new("git")
            .arg("--version")
            .output()
            .await
            .context("Git command not found. Please install git.")?;

        if !output.status.success() {
            anyhow::bail!("Git is installed but not working correctly");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_url() {
        let fetcher = GitFetcher::new(PathBuf::from("/tmp/cache"));
        let hash1 = fetcher.hash_url("https://github.com/urbit/urbit");
        let hash2 = fetcher.hash_url("https://github.com/urbit/urbit");
        let hash3 = fetcher.hash_url("https://github.com/different/repo");

        assert_eq!(hash1, hash2, "Same URL should produce same hash");
        assert_ne!(
            hash1, hash3,
            "Different URLs should produce different hashes"
        );
    }

    #[test]
    fn test_get_repo_cache_path() {
        let fetcher = GitFetcher::new(PathBuf::from("/tmp/cache"));
        let path = fetcher.get_repo_cache_path("https://github.com/urbit/urbit", "abc123def456789");

        assert!(path.to_string_lossy().contains("/tmp/cache"));
        assert!(path.to_string_lossy().contains("abc123def456"));
    }
}
