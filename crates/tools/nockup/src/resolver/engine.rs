use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use colored::Colorize;

use crate::cache::PackageCache;
use crate::git_fetcher::{GitFetcher, GitSpec};
use crate::manifest::{DependencySpec, HoonPackage};
use crate::resolver::types::{ResolvedGraph, ResolvedPackage};
use crate::resolver::{registry, VersionSpec};

/// Main dependency resolver
pub struct Resolver {
    cache: PackageCache,
    git_fetcher: GitFetcher,
}

impl Resolver {
    /// Create a new resolver
    pub fn new() -> Result<Self> {
        let cache = PackageCache::new()?;
        let git_fetcher = GitFetcher::new(cache.git_dir());

        Ok(Self { cache, git_fetcher })
    }

    /// Resolve all dependencies in a manifest
    pub async fn resolve(&self, manifest: &HoonPackage) -> Result<ResolvedGraph> {
        println!("{} Resolving dependencies...", "📦".cyan());

        let mut graph = ResolvedGraph::new();
        let mut visited = std::collections::HashSet::new();
        let mut to_resolve = Vec::new();

        // Get dependencies from manifest
        let dependencies = match manifest.dependencies.as_ref() {
            Some(deps) if !deps.is_empty() => deps,
            _ => {
                println!("  No dependencies to resolve");
                return Ok(graph);
            }
        };

        // Queue initial dependencies
        for (name, spec) in dependencies {
            to_resolve.push((name.clone(), spec.clone()));
        }

        // Resolve dependencies recursively
        while let Some((name, spec)) = to_resolve.pop() {
            // Skip if already resolved
            if visited.contains(&name) {
                continue;
            }
            visited.insert(name.clone());

            println!("  {} Resolving {}...", "→".cyan(), name.yellow());

            // Check cache first
            if let Some(cached) = self.check_cache(&name, &spec).await? {
                println!("    {} Found in cache", "✓".green());
                graph.add_package(cached);

                // Queue transitive dependencies
                let deps = registry::get_dependencies(&name).await;
                for dep in deps {
                    if !visited.contains(&dep) {
                        // Use "latest" for transitive dependencies
                        to_resolve
                            .push((dep.clone(), DependencySpec::Simple("latest".to_string())));
                    }
                }
                continue;
            }

            // Resolve from source
            let resolved = self
                .resolve_dependency(&name, &spec)
                .await
                .with_context(|| format!("Failed to resolve dependency '{}'", name))?;

            graph.add_package(resolved);

            // Queue transitive dependencies
            let deps = registry::get_dependencies(&name).await;
            for dep in deps {
                if !visited.contains(&dep) {
                    // Use "latest" for transitive dependencies
                    to_resolve.push((dep.clone(), DependencySpec::Simple("latest".to_string())));
                }
            }
        }

        // Compute installation order (topological sort)
        graph.compute_install_order()?;

        println!("{} Resolved {} packages", "✓".green(), graph.packages.len());

        Ok(graph)
    }

    /// Resolve a single dependency
    async fn resolve_dependency(
        &self,
        name: &str,
        spec: &DependencySpec,
    ) -> Result<ResolvedPackage> {
        // Convert DependencySpec to GitSpec
        let git_spec = self.dep_spec_to_git_spec(spec, name).await?;

        // Fetch the repository
        println!(
            "    {} Fetching from {}...",
            "⬇".cyan(),
            git_spec.url.cyan()
        );
        let repo_path = self
            .git_fetcher
            .fetch(&git_spec)
            .await
            .context("Failed to fetch git repository")?;

        // Determine exact commit
        let commit = self.get_exact_commit(&git_spec).await?;

        println!(
            "    {} Commit: {}",
            "→".cyan(),
            commit.chars().take(12).collect::<String>().yellow()
        );

        // Determine the source directory to cache
        let source_dir = if let Some(ref subpath) = git_spec.path {
            repo_path.join(subpath)
        } else {
            repo_path.clone()
        };

        // Verify source directory exists
        if !source_dir.exists() {
            anyhow::bail!(
                "Source path {} does not exist in repository",
                source_dir.display()
            );
        }

        // Validate all requested source files exist
        let source_files = self.validate_source_files(&source_dir, spec)?;

        // Check for transitive dependencies (look for hoon.toml in fetched repo)
        let transitive_deps = self
            .load_transitive_deps(repo_path.as_path(), &git_spec)
            .await?;

        if !transitive_deps.is_empty() {
            println!(
                "    {} Found {} transitive dependencies",
                "→".cyan(),
                transitive_deps.len()
            );
        }

        // Cache the package (always cache the full source directory)
        let version_spec = self.spec_to_version_spec(spec)?;
        let version_str = version_spec.to_canonical_string();

        // For wildcard versions ("*" or "latest"), cache using the commit hash instead
        // so that the cache lookup will work correctly
        let cache_version_str = if version_str == "*" {
            format!("commit:{}", commit)
        } else {
            version_str.clone()
        };

        println!("    {} Caching to packages cache...", "💾".cyan());

        self.cache
            .cache_package(
                name, &cache_version_str, &commit, &git_spec.url, &source_dir,
            )
            .await?;

        Ok(ResolvedPackage {
            name: name.to_string(),
            version_spec,
            commit,
            source_url: git_spec.url.clone(),
            source_path: git_spec.path.clone(),
            install_path: git_spec.install_path.clone(),
            source_files: if source_files.is_empty() {
                None
            } else {
                Some(source_files)
            },
            dependencies: transitive_deps,
        })
    }

    /// Check if package is already in cache
    async fn check_cache(
        &self,
        name: &str,
        spec: &DependencySpec,
    ) -> Result<Option<ResolvedPackage>> {
        let version_spec = self.spec_to_version_spec(spec)?;
        let version_str = version_spec.to_canonical_string();

        if let Some(cached) = self.cache.find_cached(name, &version_str).await? {
            // Reconstruct the GitSpec to get source_path and source_files
            let git_spec = self.dep_spec_to_git_spec(spec, name).await?;

            // Extract files list from spec
            let source_files = match spec {
                DependencySpec::Full { files, .. } => files
                    .as_ref()
                    .map(|f| f.iter().map(|s| format!("{}.hoon", s)).collect()),
                _ => None,
            };

            return Ok(Some(ResolvedPackage {
                name: name.to_string(),
                version_spec,
                commit: cached.commit,
                source_url: cached.source_url,
                source_path: git_spec.path,
                install_path: git_spec.install_path,
                source_files,
                dependencies: HashMap::new(), // TODO: Store in cache metadata
            }));
        }

        Ok(None)
    }

    /// Convert DependencySpec to GitSpec
    async fn dep_spec_to_git_spec(&self, spec: &DependencySpec, name: &str) -> Result<GitSpec> {
        match spec {
            DependencySpec::Simple(version) => {
                // Try to look up in registry
                if let Some(entry) = registry::lookup(name).await {
                    // Parse the version spec to extract tag/branch/commit
                    let version_spec = VersionSpec::parse(version)?;
                    let (tag, branch) = match version_spec {
                        VersionSpec::Kelvin(k) => (Some(format!("{}k", k)), None),
                        VersionSpec::Tag(t) => (Some(t), None),
                        VersionSpec::Branch(b) => (None, Some(b)),
                        VersionSpec::Semver(ref req) if req == &semver::VersionReq::STAR => {
                            // "latest" or "*" means use the default branch
                            (None, None)
                        }
                        VersionSpec::Semver(_) => (Some(version.clone()), None),
                        VersionSpec::Commit(_) => {
                            // For commits, we'll let get_exact_commit handle it
                            (None, None)
                        }
                    };
                    Ok(registry::to_git_spec(&entry, tag, branch))
                } else {
                    anyhow::bail!(
                        "Package '{}' not found in registry. \
                        Use full git spec with 'git' field.",
                        name
                    )
                }
            }
            DependencySpec::Version { version } => {
                // Try to look up in registry
                if let Some(entry) = registry::lookup(name).await {
                    let version_spec = VersionSpec::parse(version)?;
                    let (tag, branch) = match version_spec {
                        VersionSpec::Kelvin(k) => (Some(format!("{}k", k)), None),
                        VersionSpec::Tag(t) => (Some(t), None),
                        VersionSpec::Branch(b) => (None, Some(b)),
                        VersionSpec::Semver(ref req) if req == &semver::VersionReq::STAR => {
                            // "latest" or "*" means use the default branch
                            (None, None)
                        }
                        VersionSpec::Semver(_) => (Some(version.clone()), None),
                        VersionSpec::Commit(_) => (None, None),
                    };
                    Ok(registry::to_git_spec(&entry, tag, branch))
                } else {
                    anyhow::bail!(
                        "Package '{}' not found in registry. \
                        Use full git spec with 'git' field.",
                        name
                    )
                }
            }
            DependencySpec::Full {
                git,
                commit,
                tag,
                branch,
                path,
                ..
            } => {
                let url = git.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("Git URL is required (registry not yet implemented)")
                })?;

                Ok(GitSpec {
                    url: url.clone(),
                    commit: commit.clone(),
                    tag: tag.clone(),
                    branch: branch.clone(),
                    path: path.clone(),
                    install_path: None, // Don't auto-set for manifest packages; let install.rs handle it
                    file: None,         // Multiple files handled separately in source_files
                })
            }
        }
    }

    /// Get exact commit hash for a GitSpec
    async fn get_exact_commit(&self, spec: &GitSpec) -> Result<String> {
        if let Some(ref commit) = spec.commit {
            // Already have exact commit
            return Ok(commit.clone());
        }

        if let Some(ref tag) = spec.tag {
            // Resolve tag to commit
            return self.git_fetcher.resolve_tag(&spec.url, tag).await;
        }

        if let Some(ref branch) = spec.branch {
            // Resolve branch to commit
            return self.git_fetcher.resolve_branch(&spec.url, branch).await;
        }

        // Default: resolve main/master
        match self.git_fetcher.resolve_branch(&spec.url, "main").await {
            Ok(commit) => Ok(commit),
            Err(_) => self.git_fetcher.resolve_branch(&spec.url, "master").await,
        }
    }

    /// Load transitive dependencies from a fetched package
    async fn load_transitive_deps(
        &self,
        repo_path: &Path,
        git_spec: &GitSpec,
    ) -> Result<HashMap<String, DependencySpec>> {
        // Check for hoon.toml in the fetched repo
        let manifest_path = if let Some(ref subdir) = git_spec.path {
            repo_path.join(subdir).join("hoon.toml")
        } else {
            repo_path.join("hoon.toml")
        };

        if !manifest_path.exists() {
            // No transitive dependencies
            return Ok(HashMap::new());
        }

        // Load and parse manifest
        match HoonPackage::load(&manifest_path)? {
            Some(pkg) => Ok(pkg.dependencies.unwrap_or_default().into_iter().collect()),
            None => Ok(HashMap::new()),
        }
    }

    /// Validate that all requested source files exist and return the list
    fn validate_source_files(
        &self,
        source_dir: &Path,
        spec: &DependencySpec,
    ) -> Result<Vec<String>> {
        let files = match spec {
            DependencySpec::Full { files: Some(f), .. } => f.clone(),
            _ => return Ok(Vec::new()),
        };

        let mut validated = Vec::new();
        for file_path in &files {
            let full_path = format!("{}.hoon", file_path);
            let abs_path = source_dir.join(&full_path);

            if !abs_path.exists() {
                anyhow::bail!(
                    "Requested file '{}' not found in package at {}",
                    full_path,
                    source_dir.display()
                );
            }

            validated.push(full_path);
        }

        Ok(validated)
    }

    /// Convert DependencySpec to VersionSpec for caching
    fn spec_to_version_spec(&self, spec: &DependencySpec) -> Result<VersionSpec> {
        match spec {
            DependencySpec::Simple(s) => VersionSpec::parse(s),
            DependencySpec::Version { version } => VersionSpec::parse(version),
            DependencySpec::Full {
                version,
                commit,
                tag,
                branch,
                kelvin,
                ..
            } => {
                // Priority: commit > tag > kelvin > branch > version
                if let Some(c) = commit {
                    return Ok(VersionSpec::Commit(c.clone()));
                }
                if let Some(t) = tag {
                    return Ok(VersionSpec::Tag(t.clone()));
                }
                if let Some(k) = kelvin {
                    return VersionSpec::parse(k);
                }
                if let Some(b) = branch {
                    return Ok(VersionSpec::Branch(b.clone()));
                }
                if let Some(v) = version {
                    return VersionSpec::parse(v);
                }

                anyhow::bail!("DependencySpec has no version information")
            }
        }
    }
}
