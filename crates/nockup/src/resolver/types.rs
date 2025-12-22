use std::collections::HashMap;

use crate::manifest::DependencySpec;
use crate::resolver::VersionSpec;

/// A resolved package with exact commit and dependencies
#[derive(Debug, Clone)]
pub struct ResolvedPackage {
    pub name: String,
    pub version_spec: VersionSpec, // Original spec from manifest
    pub commit: String,            // Exact commit hash
    pub source_url: String,
    pub source_path: Option<String>, // Subdir within repo to fetch from (e.g., "pkg/arvo/sys")
    pub install_path: Option<String>, // Subdir to install to (e.g., "sys")
    pub source_files: Option<Vec<String>>, // Specific files to extract (if any)
    pub dependencies: HashMap<String, DependencySpec>, // Transitive deps
}

/// A resolved dependency graph
#[derive(Debug)]
pub struct ResolvedGraph {
    pub packages: HashMap<String, ResolvedPackage>,
    pub install_order: Vec<String>, // Topological sort for installation
}

impl ResolvedGraph {
    /// Create a new empty graph
    pub fn new() -> Self {
        Self {
            packages: HashMap::new(),
            install_order: Vec::new(),
        }
    }

    /// Add a package to the graph
    pub fn add_package(&mut self, package: ResolvedPackage) {
        self.packages.insert(package.name.clone(), package);
    }

    /// Compute topological installation order
    /// Simple approach: no cycles allowed, packages with no deps come first
    pub fn compute_install_order(&mut self) -> anyhow::Result<()> {
        let mut visited = HashMap::new();
        let mut order = Vec::new();

        for name in self.packages.keys() {
            self.visit_package(name, &mut visited, &mut order)?;
        }

        self.install_order = order;
        Ok(())
    }

    fn visit_package(
        &self,
        name: &str,
        visited: &mut HashMap<String, bool>,
        order: &mut Vec<String>,
    ) -> anyhow::Result<()> {
        // Check if already processed
        if let Some(&done) = visited.get(name) {
            if !done {
                anyhow::bail!("Circular dependency detected involving package '{}'", name);
            }
            return Ok(());
        }

        // Mark as being visited (for cycle detection)
        visited.insert(name.to_string(), false);

        // Visit dependencies first
        if let Some(pkg) = self.packages.get(name) {
            for dep_name in pkg.dependencies.keys() {
                // Only visit if we have this dependency in our graph
                if self.packages.contains_key(dep_name) {
                    self.visit_package(dep_name, visited, order)?;
                }
            }
        }

        // Mark as done
        visited.insert(name.to_string(), true);
        order.push(name.to_string());

        Ok(())
    }
}

impl Default for ResolvedGraph {
    fn default() -> Self {
        Self::new()
    }
}
