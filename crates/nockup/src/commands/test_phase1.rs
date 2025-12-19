/// Temporary test command to demonstrate Phase 1 infrastructure
/// This will be removed once full resolver is implemented
use anyhow::Result;
use colored::Colorize;

use crate::cache::PackageCache;
use crate::git_fetcher::{GitFetcher, GitSpec};
use crate::resolver::VersionSpec;

pub async fn run() -> Result<()> {
    println!("{}", "=== Testing Phase 1 Infrastructure ===".cyan().bold());
    println!();

    // Initialize cache
    println!("{} Initializing package cache...", "📦".green());
    let cache = PackageCache::new()?;
    println!(
        "  Cache directory: {}",
        cache.root().display().to_string().cyan()
    );
    println!();

    // Initialize git fetcher
    println!("{} Initializing Git fetcher...", "🔧".green());
    let git_fetcher = GitFetcher::new(cache.git_dir());
    println!();

    // Test 1: Parse version specs
    println!("{}", "--- Test 1: Version Spec Parsing ---".yellow().bold());
    let specs = vec!["k409", "commit:abc123def", "tag:v1.2.3", "branch:main", "^1.2.0"];

    for spec_str in specs {
        match VersionSpec::parse(spec_str) {
            Ok(spec) => {
                println!(
                    "  ✓ Parsed '{}' → {}",
                    spec_str.cyan(),
                    spec.to_canonical_string().green()
                );
            }
            Err(e) => {
                println!("  ✗ Failed to parse '{}': {}", spec_str.red(), e);
            }
        }
    }
    println!();

    // Test 2: Fetch a real repository
    println!(
        "{}",
        "--- Test 2: Git Repository Fetching ---".yellow().bold()
    );
    println!("  Fetching github.com/urbit/urbit (may take a moment)...");

    let spec = GitSpec {
        url: "https://github.com/urbit/urbit".to_string(),
        commit: None,
        tag: Some("409k".to_string()), // Using kelvin tag format
        branch: None,
        path: None,
        install_path: None,
        file: None,
    };

    match git_fetcher.fetch(&spec).await {
        Ok(path) => {
            println!(
                "  {} Fetched to: {}",
                "✓".green(),
                path.display().to_string().cyan()
            );

            // Check if path exists
            if path.exists() {
                println!("  {} Repository is cached!", "✓".green());
            }
        }
        Err(e) => {
            println!("  {} Fetch failed: {}", "✗".red(), e);
            println!("  (This is expected if git or network is unavailable)");
        }
    }
    println!();

    // Test 3: List tags from a repo
    println!("{}", "--- Test 3: List Repository Tags ---".yellow().bold());
    println!("  Fetching tags from github.com/urbit/urbit...");

    match git_fetcher
        .list_tags("https://github.com/urbit/urbit")
        .await
    {
        Ok(tags) => {
            let display_count = 10.min(tags.len());
            println!(
                "  {} Found {} tags (showing first {}):",
                "✓".green(),
                tags.len(),
                display_count
            );
            for tag in tags.iter().take(display_count) {
                println!("    - {}", tag.cyan());
            }
        }
        Err(e) => {
            println!("  {} Failed to list tags: {}", "✗".red(), e);
            println!("  (This is expected if git or network is unavailable)");
        }
    }
    println!();

    // Test 4: Cache operations
    println!("{}", "--- Test 4: Cache Operations ---".yellow().bold());

    match cache.stats().await {
        Ok(stats) => {
            println!("  {} Cache Statistics:", "📊".cyan());
            println!(
                "    Total packages: {}",
                stats.total_packages.to_string().green()
            );
            println!(
                "    Unique packages: {}",
                stats.unique_packages.to_string().green()
            );
            println!("    Total size: {:.2} MB", stats.total_size_mb());
        }
        Err(e) => {
            println!("  {} Failed to get cache stats: {}", "✗".red(), e);
        }
    }
    println!();

    // Test 5: Package spec parsing
    println!("{}", "--- Test 5: Package Spec Parsing ---".yellow().bold());
    let package_specs = vec!["arvo@k414", "lagoon@^0.2.0", "sequent@commit:abc123"];

    for spec_str in package_specs {
        match crate::resolver::parse_package_spec(spec_str) {
            Ok((name, version)) => {
                println!(
                    "  ✓ Parsed '{}' → name={}, version={}",
                    spec_str.cyan(),
                    name.green(),
                    version.to_canonical_string().yellow()
                );
            }
            Err(e) => {
                println!("  ✗ Failed to parse '{}': {}", spec_str.red(), e);
            }
        }
    }
    println!();

    println!("{}", "=== Phase 1 Tests Complete ===".cyan().bold());
    println!();
    println!("The following modules are ready:");
    println!(
        "  {} GitFetcher - Fetch repos, resolve tags/branches",
        "✓".green()
    );
    println!("  {} PackageCache - Store and manage packages", "✓".green());
    println!(
        "  {} VersionSpec Parser - Parse all version formats",
        "✓".green()
    );
    println!();
    println!("Next: Implement full dependency resolver in Phase 2");

    Ok(())
}
