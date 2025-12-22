use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "nockup")]
#[command(about = "A developer support framework for NockApp development")]
#[command(version = env!("FULL_VERSION"))]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    // Hierarchical commands
    /// Project management (build, run, init)
    #[command(subcommand)]
    Project(ProjectCommand),

    /// Package and dependency management
    #[command(subcommand)]
    Package(PackageCommand),

    /// Cache management
    #[command(subcommand)]
    Cache(CacheCommand),

    /// Toolchain / channel management
    #[command(subcommand)]
    Channel(ChannelCommand),

    // Legacy flat commands (backward compatible)
    /// Build a NockApp project
    #[command(hide = true)]
    Build {
        #[arg(value_name = "PROJECT")]
        project: String,
    },

    /// Initialize a new NockApp project
    #[command(hide = true)]
    Init {
        #[arg(value_name = "NAME")]
        project: String,
    },

    /// Check for updates to nockup, hoon, and hoonc
    Update,

    /// Initialize nockup cache and download templates
    Install,

    /// Run a NockApp project
    #[command(hide = true)]
    Run {
        project: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Test Phase 1 infrastructure (temporary demo command)
    #[command(hide = true)]
    TestPhase1,
}

#[derive(clap::Subcommand, Debug)]
pub enum ProjectCommand {
    /// Build a NockApp project
    Build { project: Option<String> },
    /// Run a NockApp project
    Run {
        project: Option<String>,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Initialize a new NockApp project
    Init,
}

#[derive(clap::Subcommand, Debug)]
pub enum PackageCommand {
    /// Initialize a new NockApp project
    Init { name: Option<String> },

    /// Add a dependency to nockapp.toml
    Add {
        /// Package name
        name: String,
        /// Version specification (e.g., @k409, ^1.2.3, @tag:v1.0.0)
        #[arg(short, long)]
        version: Option<String>,
    },

    /// Remove a dependency from nockapp.toml
    Remove {
        /// Package name to remove
        name: String,
    },

    /// List all dependencies and their installation status
    List,

    /// Install dependencies from nockapp.toml
    Install,

    /// Update dependencies to latest versions
    Update,

    /// Clear the package cache
    Purge {
        /// Only show what would be deleted without actually deleting
        #[arg(short = 'n', long)]
        dry_run: bool,
    },

    /// Grab a package (deprecated - use add)
    #[command(hide = true)]
    Grab { spec: String },

    /// Generate proxy files for a package
    GenerateProxy { url: String, path: Option<String> },
}

#[derive(clap::Subcommand, Debug)]
pub enum CacheCommand {
    /// Clear cache directories
    Clear {
        /// Clear git repository cache
        #[arg(long)]
        git: bool,
        /// Clear processed packages cache
        #[arg(long)]
        packages: bool,
        /// Clear registry cache
        #[arg(long)]
        registry: bool,
        /// Clear all caches
        #[arg(long)]
        all: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum ChannelCommand {
    Show,
    Set { channel: String },
}
