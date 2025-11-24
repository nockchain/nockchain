use std::process;

use clap::Parser;

mod cli;
mod commands;
mod lib_manager;
mod version;

use cli::*;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        None => {
            // No subcommand provided - show version info
            version::show_version_info().await
        }
        Some(Commands::Install) => commands::install::run().await,
        Some(Commands::Init { name }) => commands::init::run(name).await,
        Some(Commands::Update) => commands::update::run().await,
        Some(Commands::Build { project }) => commands::build::run(project).await,
        Some(Commands::Run { project, args }) => commands::run::run(project, args).await,
        Some(Commands::Channel { action }) => commands::channel::run(action).await,
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}
