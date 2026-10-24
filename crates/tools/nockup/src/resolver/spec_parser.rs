use anyhow::Result;
use semver::VersionReq;

use crate::manifest::DependencySpec;

/// Parsed version specification
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSpec {
    /// Kelvin version (e.g., @k414)
    Kelvin(u32),

    /// Exact commit hash (e.g., @commit:abc123def)
    Commit(String),

    /// Git tag (e.g., @tag:v1.2.3)
    Tag(String),

    /// Git branch (e.g., @branch:main)
    Branch(String),

    /// Semver requirement (e.g., ^1.2.0, ~1.2.3, >=2.0.0)
    Semver(VersionReq),
}

impl VersionSpec {
    /// Parse a version spec string
    ///
    /// Supported formats:
    /// - `@k414` or `k414` → Kelvin(414)
    /// - `@commit:abc123` or `commit:abc123` → Commit("abc123")
    /// - `@tag:v1.2.3` or `tag:v1.2.3` → Tag("v1.2.3")
    /// - `@branch:main` or `branch:main` → Branch("main")
    /// - `latest` or `*` → Semver(STAR) (always latest)
    /// - `^1.2.0`, `~1.2.3`, `>=2.0.0`, `1.2.3` → Semver(...)
    pub fn parse(input: &str) -> Result<Self> {
        // Trim whitespace and remove leading @ if present
        let input = input.trim().trim_start_matches('@');

        // Handle "latest" as an alias for "*"
        if input == "latest" {
            return Ok(VersionSpec::Semver(VersionReq::STAR));
        }

        // Try kelvin format (with optional ^ prefix for minimum version)
        // ^k409 or k409
        let kelvin_input = input.strip_prefix('^').unwrap_or(input);
        if let Some(kelvin_str) = kelvin_input.strip_prefix('k') {
            if let Ok(kelvin) = kelvin_str.parse::<u32>() {
                return Ok(VersionSpec::Kelvin(kelvin));
            }
        }

        // Try explicit type prefixes
        if let Some(commit) = input.strip_prefix("commit:") {
            return Ok(VersionSpec::Commit(commit.to_string()));
        }

        if let Some(tag) = input.strip_prefix("tag:") {
            return Ok(VersionSpec::Tag(tag.to_string()));
        }

        if let Some(branch) = input.strip_prefix("branch:") {
            return Ok(VersionSpec::Branch(branch.to_string()));
        }

        // Try semver parsing
        match VersionReq::parse(input) {
            Ok(req) => Ok(VersionSpec::Semver(req)),
            Err(e) => {
                anyhow::bail!(
                    "Invalid version spec '{}': not a kelvin, commit, tag, branch, or semver. Error: {}",
                    input,
                    e
                )
            }
        }
    }

    /// Check if this spec matches a given version string
    pub fn matches(&self, version: &str) -> bool {
        match self {
            VersionSpec::Kelvin(k) => {
                // Check if version is k<number> matching our kelvin
                version
                    .trim_start_matches('@')
                    .strip_prefix('k')
                    .and_then(|s| s.parse::<u32>().ok())
                    .map(|v| v == *k)
                    .unwrap_or(false)
            }
            VersionSpec::Commit(c) => {
                // Match exact commit or prefix
                version.starts_with(c) || c.starts_with(version)
            }
            VersionSpec::Tag(t) => {
                // Match exact tag
                version == t || version == format!("@{}", t)
            }
            VersionSpec::Branch(b) => {
                // Match exact branch name
                version == b || version == format!("@{}", b)
            }
            VersionSpec::Semver(req) => {
                // Parse version and check semver match
                if let Ok(ver) = semver::Version::parse(version.trim_start_matches('v')) {
                    req.matches(&ver)
                } else {
                    false
                }
            }
        }
    }

    /// Convert to a DependencySpec for use in manifests
    pub fn to_dependency_spec(&self, git_url: Option<String>) -> DependencySpec {
        match self {
            VersionSpec::Kelvin(k) => DependencySpec::Full {
                version: None,
                git: git_url,
                commit: None,
                tag: None,
                branch: None,
                path: None,
                files: None,
                kelvin: Some(format!("k{}", k)),
            },
            VersionSpec::Commit(c) => DependencySpec::Full {
                version: None,
                git: git_url,
                commit: Some(c.clone()),
                tag: None,
                branch: None,
                path: None,
                files: None,
                kelvin: None,
            },
            VersionSpec::Tag(t) => DependencySpec::Full {
                version: None,
                git: git_url,
                commit: None,
                tag: Some(t.clone()),
                branch: None,
                path: None,
                files: None,
                kelvin: None,
            },
            VersionSpec::Branch(b) => DependencySpec::Full {
                version: None,
                git: git_url,
                commit: None,
                tag: None,
                branch: Some(b.clone()),
                path: None,
                files: None,
                kelvin: None,
            },
            VersionSpec::Semver(req) => DependencySpec::Full {
                version: Some(req.to_string()),
                git: git_url,
                commit: None,
                tag: None,
                branch: None,
                path: None,
                files: None,
                kelvin: None,
            },
        }
    }

    /// Get a canonical string representation
    pub fn to_canonical_string(&self) -> String {
        match self {
            VersionSpec::Kelvin(k) => format!("k{}", k),
            VersionSpec::Commit(c) => format!("commit:{}", c),
            VersionSpec::Tag(t) => format!("tag:{}", t),
            VersionSpec::Branch(b) => format!("branch:{}", b),
            VersionSpec::Semver(req) => req.to_string(),
        }
    }

    /// Check if this is an exact version (commit or tag), not a range
    pub fn is_exact(&self) -> bool {
        matches!(self, VersionSpec::Commit(_) | VersionSpec::Tag(_))
    }
}

/// Parse a package spec in the form "name@version"
pub fn parse_package_spec(input: &str) -> Result<(String, VersionSpec)> {
    if let Some((name, version_str)) = input.split_once('@') {
        let name = name.trim().to_string();
        let version = VersionSpec::parse(version_str)?;
        Ok((name, version))
    } else {
        anyhow::bail!("Invalid package spec '{}': expected format 'name@version'", input)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn test_parse_kelvin() {
        let spec = VersionSpec::parse("k414").unwrap();
        assert_eq!(spec, VersionSpec::Kelvin(414));

        let spec = VersionSpec::parse("@k414").unwrap();
        assert_eq!(spec, VersionSpec::Kelvin(414));

        let spec = VersionSpec::parse("k417").unwrap();
        assert_eq!(spec, VersionSpec::Kelvin(417));
    }

    #[test]
    fn test_parse_commit() {
        let spec = VersionSpec::parse("commit:abc123def").unwrap();
        assert_eq!(spec, VersionSpec::Commit("abc123def".to_string()));

        let spec = VersionSpec::parse("@commit:abc123").unwrap();
        assert_eq!(spec, VersionSpec::Commit("abc123".to_string()));
    }

    #[test]
    fn test_parse_tag() {
        let spec = VersionSpec::parse("tag:v1.2.3").unwrap();
        assert_eq!(spec, VersionSpec::Tag("v1.2.3".to_string()));

        let spec = VersionSpec::parse("@tag:v2.0").unwrap();
        assert_eq!(spec, VersionSpec::Tag("v2.0".to_string()));
    }

    #[test]
    fn test_parse_branch() {
        let spec = VersionSpec::parse("branch:main").unwrap();
        assert_eq!(spec, VersionSpec::Branch("main".to_string()));

        let spec = VersionSpec::parse("@branch:develop").unwrap();
        assert_eq!(spec, VersionSpec::Branch("develop".to_string()));
    }

    #[test]
    fn test_parse_semver() {
        let spec = VersionSpec::parse("^1.2.0").unwrap();
        assert!(matches!(spec, VersionSpec::Semver(_)));

        let spec = VersionSpec::parse("~1.2.3").unwrap();
        assert!(matches!(spec, VersionSpec::Semver(_)));

        let spec = VersionSpec::parse(">=2.0.0").unwrap();
        assert!(matches!(spec, VersionSpec::Semver(_)));

        let spec = VersionSpec::parse("1.2.3").unwrap();
        assert!(matches!(spec, VersionSpec::Semver(_)));
    }

    #[test]
    fn test_matches_kelvin() {
        let spec = VersionSpec::Kelvin(414);

        assert!(spec.matches("k414"));
        assert!(spec.matches("@k414"));
        assert!(!spec.matches("k415"));
        assert!(!spec.matches("414"));
    }

    #[test]
    fn test_matches_commit() {
        let spec = VersionSpec::Commit("abc123def".to_string());

        assert!(spec.matches("abc123def456"));
        assert!(spec.matches("abc123def"));
        assert!(!spec.matches("def456abc"));
    }

    #[test]
    fn test_matches_tag() {
        let spec = VersionSpec::Tag("v1.2.3".to_string());

        assert!(spec.matches("v1.2.3"));
        assert!(spec.matches("@v1.2.3"));
        assert!(!spec.matches("v1.2.4"));
    }

    #[test]
    fn test_matches_semver() {
        let spec = VersionSpec::parse("^1.2.0").unwrap();

        assert!(spec.matches("1.2.0"));
        assert!(spec.matches("1.2.5"));
        assert!(spec.matches("1.9.0"));
        assert!(!spec.matches("2.0.0"));
        assert!(!spec.matches("1.1.9"));

        // Test with 'v' prefix
        assert!(spec.matches("v1.2.3"));
    }

    #[test]
    fn test_parse_package_spec() {
        let (name, version) = parse_package_spec("arvo@k414").unwrap();
        assert_eq!(name, "arvo");
        assert_eq!(version, VersionSpec::Kelvin(414));

        let (name, version) = parse_package_spec("lagoon@^0.2.0").unwrap();
        assert_eq!(name, "lagoon");
        assert!(matches!(version, VersionSpec::Semver(_)));

        let (name, version) = parse_package_spec("sequent@commit:abc123").unwrap();
        assert_eq!(name, "sequent");
        assert_eq!(version, VersionSpec::Commit("abc123".to_string()));
    }

    #[test]
    fn test_to_canonical_string() {
        assert_eq!(VersionSpec::Kelvin(414).to_canonical_string(), "k414");
        assert_eq!(
            VersionSpec::Commit("abc123".to_string()).to_canonical_string(),
            "commit:abc123"
        );
        assert_eq!(
            VersionSpec::Tag("v1.2.3".to_string()).to_canonical_string(),
            "tag:v1.2.3"
        );
        assert_eq!(
            VersionSpec::Branch("main".to_string()).to_canonical_string(),
            "branch:main"
        );
    }

    #[test]
    fn test_is_exact() {
        assert!(VersionSpec::Commit("abc123".to_string()).is_exact());
        assert!(VersionSpec::Tag("v1.0.0".to_string()).is_exact());
        assert!(!VersionSpec::Kelvin(414).is_exact());
        assert!(!VersionSpec::Branch("main".to_string()).is_exact());
        assert!(!VersionSpec::parse("^1.2.0").unwrap().is_exact());
    }
}
