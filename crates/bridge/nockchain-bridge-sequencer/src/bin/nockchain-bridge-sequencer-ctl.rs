use std::error::Error;
use std::path::{Path, PathBuf};
use std::{fs, io};

use bridge::shared::config::SequencerConfigToml;
use bridge::withdrawal::sequencer::approval::{
    approval_file_path, default_manual_submit_approval_dir, write_approval_record_atomic,
    ManualSubmitApprovalConfig, WithdrawalApprovalFacts,
};
use bridge::withdrawal::sequencer::store::WithdrawalSequencerStore;
use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "nockchain-bridge-sequencer-ctl")]
struct NockchainBridgeSequencerCtlCli {
    #[command(subcommand)]
    command: CtlCommand,
}

#[derive(Subcommand, Debug)]
enum CtlCommand {
    PendingApprovals(CommonArgs),
    ShowApproval {
        #[arg(long)]
        tx_id: String,
        #[command(flatten)]
        common: CommonArgs,
    },
    ExportTx {
        #[arg(long)]
        tx_id: String,
        #[arg(short, long, value_name = "PATH")]
        output: Option<PathBuf>,
        #[command(flatten)]
        common: CommonArgs,
    },
    ApproveWithdrawal {
        #[arg(long)]
        tx_id: String,
        #[command(flatten)]
        common: CommonArgs,
        #[arg(long, help = "Required to write the approval record.")]
        yes: bool,
    },
}

#[derive(Args, Debug)]
struct CommonArgs {
    #[arg(long = "sequencer-config-path")]
    sequencer_config_path: PathBuf,
    #[arg(long)]
    data_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = NockchainBridgeSequencerCtlCli::parse();
    match cli.command {
        CtlCommand::PendingApprovals(common) => pending_approvals(&common).await,
        CtlCommand::ShowApproval { tx_id, common } => show_approval(&common, &tx_id).await,
        CtlCommand::ExportTx {
            tx_id,
            output,
            common,
        } => export_tx(&common, &tx_id, output).await,
        CtlCommand::ApproveWithdrawal { tx_id, common, yes } => {
            approve_withdrawal(&common, &tx_id, yes).await
        }
    }
}

async fn pending_approvals(common: &CommonArgs) -> Result<(), Box<dyn Error>> {
    let (store, approval_config) = open_ctl_context(common).await?;
    let facts = store.list_pending_approval_facts().await?;
    if facts.is_empty() {
        println!("no pending withdrawal approvals");
        return Ok(());
    }

    for (idx, facts) in facts.iter().enumerate() {
        if idx > 0 {
            println!();
        }
        print_approval_facts(facts, &approval_config)?;
    }
    Ok(())
}

async fn show_approval(common: &CommonArgs, tx_id: &str) -> Result<(), Box<dyn Error>> {
    let (store, approval_config) = open_ctl_context(common).await?;
    let facts = load_facts_by_tx_id(&store, tx_id).await?;
    print_approval_facts(&facts, &approval_config)?;
    Ok(())
}

async fn export_tx(
    common: &CommonArgs,
    tx_id: &str,
    output: Option<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    let (store, _) = open_ctl_context(common).await?;
    let export = store
        .load_authorized_transaction_export_by_tx_id(tx_id)
        .await?
        .ok_or_else(|| input_error(format!("no authorized transaction found for tx-id {tx_id}")))?;
    let output_path = resolve_export_output_path(output, &export.submitted_raw_tx_id);
    write_export_file(&output_path, &export.transaction_jam)?;
    println!("authorized_transaction_name={}", export.submitted_raw_tx_id);
    println!("transaction_file={}", output_path.display());
    println!("transaction_bytes={}", export.transaction_jam.len());
    Ok(())
}

async fn approve_withdrawal(
    common: &CommonArgs,
    tx_id: &str,
    yes: bool,
) -> Result<(), Box<dyn Error>> {
    if !yes {
        return Err(input_error(
            "approve-withdrawal requires --yes; refusing to write approval record",
        ));
    }

    let (store, approval_config) = open_ctl_context(common).await?;
    let facts = load_facts_by_tx_id(&store, tx_id).await?;
    print_approval_facts(&facts, &approval_config)?;
    let path = write_approval_record_atomic(&approval_config.approval_dir, &facts)?;
    println!("approval_written={}", path.display());
    Ok(())
}

fn resolve_export_output_path(output: Option<PathBuf>, tx_id: &str) -> PathBuf {
    output.unwrap_or_else(|| PathBuf::from(format!("{tx_id}.tx")))
}

fn write_export_file(path: &Path, contents: &[u8]) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

async fn open_ctl_context(
    common: &CommonArgs,
) -> Result<(WithdrawalSequencerStore, ManualSubmitApprovalConfig), Box<dyn Error>> {
    let sequencer_config = SequencerConfigToml::from_file(&common.sequencer_config_path)?;
    let sequencer_data_dir = sequencer_data_dir_from_cli(&common.data_dir);
    let store_path = sequencer_data_dir.join("withdrawal-state-store.sqlite");
    if !store_path.is_file() {
        return Err(input_error(format!(
            "withdrawal sequencer store not found at {}; check --data-dir",
            store_path.display()
        )));
    }
    let store = WithdrawalSequencerStore::open(store_path).await?;
    let approval_config = ManualSubmitApprovalConfig {
        enabled: sequencer_config.manual_submit_approval,
        approval_dir: sequencer_config
            .manual_submit_approval_dir
            .clone()
            .unwrap_or_else(|| default_manual_submit_approval_dir(&sequencer_data_dir)),
    };
    Ok((store, approval_config))
}

async fn load_facts_by_tx_id(
    store: &WithdrawalSequencerStore,
    tx_id: &str,
) -> Result<WithdrawalApprovalFacts, Box<dyn Error>> {
    store
        .load_authorized_approval_facts_by_tx_id(tx_id)
        .await?
        .ok_or_else(|| input_error(format!("no authorized withdrawal found for tx-id {tx_id}")))
}

fn print_approval_facts(
    facts: &WithdrawalApprovalFacts,
    approval_config: &ManualSubmitApprovalConfig,
) -> Result<(), Box<dyn Error>> {
    let approval_file = approval_file_path(
        &approval_config.approval_dir, &facts.authorized_transaction_name,
    )?;
    println!(
        "manual_submit_approval={}",
        if approval_config.enabled {
            "true"
        } else {
            "false"
        }
    );
    println!("withdrawal_id_as_of={}", facts.withdrawal_id_as_of);
    println!(
        "withdrawal_id_base_event_id={}",
        facts.withdrawal_id_base_event_id
    );
    println!("epoch={}", facts.epoch);
    println!("proposal_hash={}", facts.proposal_hash);
    println!(
        "authorized_transaction_name={}",
        facts.authorized_transaction_name
    );
    println!("approval_file={}", approval_file.display());
    Ok(())
}

fn sequencer_data_dir_from_cli(data_dir: &Path) -> PathBuf {
    data_dir.join("nockchain")
}

fn input_error(message: impl Into<String>) -> Box<dyn Error> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn unique_test_dir(name: &str) -> Result<PathBuf, Box<dyn Error>> {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        Ok(std::env::temp_dir().join(format!(
            "nockchain-bridge-sequencer-ctl-{name}-{}-{timestamp}",
            std::process::id()
        )))
    }

    #[test]
    fn default_export_output_path_uses_tx_id_with_tx_suffix() {
        assert_eq!(
            resolve_export_output_path(None, "submitted-tx"),
            PathBuf::from("submitted-tx.tx")
        );
        assert_eq!(
            resolve_export_output_path(Some(PathBuf::from("custom.tx")), "submitted-tx"),
            PathBuf::from("custom.tx")
        );
    }

    #[test]
    fn write_export_file_creates_parent_and_writes_contents() -> Result<(), Box<dyn Error>> {
        let dir = unique_test_dir("write-export")?;
        let path = dir.join("nested").join("withdrawal.tx");

        write_export_file(&path, b"tx-bytes")?;

        assert_eq!(fs::read(&path)?, b"tx-bytes");
        fs::remove_dir_all(&dir)?;
        Ok(())
    }
}
