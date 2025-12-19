use std::process;

use clap::Parser;
use colored::Colorize;
use nockup::cli::*;
use nockup::{commands, version};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        // Hierarchical commands
        Some(Commands::Project(cmd)) => commands::build::run(cmd).await,
        Some(Commands::Package(cmd)) => commands::package::run(cmd).await,
        Some(Commands::Channel(cmd)) => commands::channel::run(cmd).await,

        // Legacy flat commands (backward compatible)
        Some(Commands::Build { project }) => {
            commands::build::run(ProjectCommand::Build {
                project: Some(project),
            })
            .await
        }
        Some(Commands::Init { project }) => commands::init::run(project).await,
        Some(Commands::Update) => commands::update::run().await,
        // Some(Commands::Init { name: _ }) => {
        //     eprintln!("{}", "warning: `nockup init` is now `nockup package init`".yellow());
        //     commands::package::run(PackageCommand::Init{ name: name }).await
        // }
        Some(Commands::Install) => {
            eprintln!(
                "{}",
                "warning: `nockup install` is now `nockup update`".yellow()
            );
            commands::package::run(PackageCommand::Install).await
        }
        Some(Commands::Run { project, args }) => {
            commands::build::run(ProjectCommand::Run { project, args }).await
        }
        Some(Commands::TestPhase1) => commands::test_phase1::run().await,

        None => version::show_version_info().await,
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}
