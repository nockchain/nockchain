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
    /// Initialize nockup cache and download templates
    Install,
    /// Initialize a new NockApp project from a .toml config file
    Init {
        /// Name of the project config file (looks for <name>.toml)
        name: String,
    },
    /// Check for updates to nockup, hoon, and hoonc
    Update,
    /// Build a NockApp project
    Build {
        /// Path to the project directory
        project: String,
    },
    /// Run a NockApp project
    Run {
        /// Path to the project directory  
        project: String,
        /// Additional arguments to pass to the running application
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Manage channels (e.g., set default)
    Channel {
        #[command(subcommand)]
        action: ChannelAction,
    },
}

#[derive(Subcommand)]
pub enum ChannelAction {
    /// Set the default channel (e.g., stable, nightly)
    Set {
        channel: String, // e.g., "stable" or "nightly"
    },
    /// Show the current channel
    Show,
}
