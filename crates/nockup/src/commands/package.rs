pub mod add;
pub mod init;
pub mod install;
pub mod list;
pub mod purge;
pub mod remove;
pub mod update;

use anyhow::Result;

use crate::cli::PackageCommand;

pub async fn run(cmd: PackageCommand) -> Result<()> {
    match cmd {
        PackageCommand::Init { name } => init::run(name).await,
        PackageCommand::Add { name, version } => add::run(name, version).await,
        PackageCommand::Remove { name } => remove::run(name).await,
        PackageCommand::List => list::run().await,
        PackageCommand::Install => install::run().await,
        PackageCommand::Update => update::run().await,
        PackageCommand::Purge { dry_run } => purge::purge(dry_run).await,
        PackageCommand::Grab { .. } => {
            anyhow::bail!("`nockup package grab` is deprecated – use `add`")
        }
        PackageCommand::GenerateProxy { .. } => {
            anyhow::bail!("`generate-proxy` coming soon")
        }
    }
}
