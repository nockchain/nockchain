pub mod set;
pub mod show;

use anyhow::Result;

use crate::cli::ChannelCommand;

pub async fn run(command: ChannelCommand) -> Result<()> {
    match command {
        ChannelCommand::Set { channel } => set::run(&channel),
        ChannelCommand::Show => show::run(),
    }
}
