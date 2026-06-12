use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::shared::errors::BridgeError;
use crate::withdrawal::sequencer::store::WithdrawalSequencerStore;
use crate::withdrawal::state::{LiveWithdrawalView, WithdrawalState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualSubmitApprovalConfig {
    pub enabled: bool,
    pub approval_dir: PathBuf,
}

impl Default for ManualSubmitApprovalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            approval_dir: PathBuf::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawalApprovalFacts {
    pub withdrawal_id_as_of: String,
    pub withdrawal_id_base_event_id: String,
    pub epoch: u64,
    pub proposal_hash: String,
    pub authorized_transaction_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawalApprovalRecord {
    pub facts: WithdrawalApprovalFacts,
    pub approve: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualSubmitApprovalDecision {
    Approved,
    Deferred(String),
}

pub fn default_manual_submit_approval_dir(sequencer_data_dir: &Path) -> PathBuf {
    sequencer_data_dir.join("withdrawal-approvals")
}

pub fn approval_facts_for_authorized_row(
    row: &LiveWithdrawalView,
) -> Result<Option<WithdrawalApprovalFacts>, BridgeError> {
    if row.state != WithdrawalState::Authorized {
        return Ok(None);
    }

    let proposal_hash = row.proposal_hash.clone().ok_or_else(|| {
        BridgeError::Runtime(format!(
            "authorized withdrawal {:?} epoch {} is missing proposal_hash",
            row.id, row.current_epoch
        ))
    })?;
    let authorized_transaction_name = row.authorized_transaction_name.clone().ok_or_else(|| {
        BridgeError::Runtime(format!(
            "authorized withdrawal {:?} epoch {} is missing authorized_transaction_name",
            row.id, row.current_epoch
        ))
    })?;

    Ok(Some(WithdrawalApprovalFacts {
        withdrawal_id_as_of: row.id.as_of.to_base58(),
        withdrawal_id_base_event_id: hex::encode(&row.id.base_event_id.0),
        epoch: row.current_epoch,
        proposal_hash,
        authorized_transaction_name,
    }))
}

impl WithdrawalSequencerStore {
    pub async fn load_authorized_approval_facts_by_tx_id(
        &self,
        tx_id: &str,
    ) -> Result<Option<WithdrawalApprovalFacts>, BridgeError> {
        for row in self.list_sequenced_withdrawals().await? {
            if row.state == WithdrawalState::Authorized
                && row.authorized_transaction_name.as_deref() == Some(tx_id)
            {
                return approval_facts_for_authorized_row(&row);
            }
        }
        Ok(None)
    }

    pub async fn list_pending_approval_facts(
        &self,
    ) -> Result<Vec<WithdrawalApprovalFacts>, BridgeError> {
        let mut facts = Vec::new();
        for row in self.list_sequenced_withdrawals().await? {
            if let Some(row_facts) = approval_facts_for_authorized_row(&row)? {
                facts.push(row_facts);
            }
        }
        Ok(facts)
    }
}

pub fn render_approval_record(facts: &WithdrawalApprovalFacts) -> String {
    format!(
        "withdrawal_id_as_of={}\nwithdrawal_id_base_event_id={}\nepoch={}\nproposal_hash={}\nauthorized_transaction_name={}\napprove=true\n",
        facts.withdrawal_id_as_of,
        facts.withdrawal_id_base_event_id,
        facts.epoch,
        facts.proposal_hash,
        facts.authorized_transaction_name,
    )
}

pub fn parse_approval_record(contents: &str) -> Result<WithdrawalApprovalRecord, BridgeError> {
    let mut values = BTreeMap::<String, String>::new();
    for (line_number, raw_line) in contents.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(BridgeError::Runtime(format!(
                "malformed approval record line {}: expected key=value",
                line_number + 1
            )));
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "withdrawal_id_as_of"
            | "withdrawal_id_base_event_id"
            | "epoch"
            | "proposal_hash"
            | "authorized_transaction_name"
            | "approve" => {}
            other => {
                return Err(BridgeError::Runtime(format!(
                    "malformed approval record line {}: unknown key {other}",
                    line_number + 1
                )));
            }
        }
        if values.insert(key.to_string(), value.to_string()).is_some() {
            return Err(BridgeError::Runtime(format!(
                "malformed approval record line {}: duplicate key {key}",
                line_number + 1
            )));
        }
    }

    let withdrawal_id_as_of = required_record_value(&mut values, "withdrawal_id_as_of")?;
    let withdrawal_id_base_event_id =
        required_record_value(&mut values, "withdrawal_id_base_event_id")?;
    let epoch = required_record_value(&mut values, "epoch")?
        .parse::<u64>()
        .map_err(|err| BridgeError::Runtime(format!("malformed approval epoch: {err}")))?;
    let proposal_hash = required_record_value(&mut values, "proposal_hash")?;
    let authorized_transaction_name =
        required_record_value(&mut values, "authorized_transaction_name")?;
    let approve = match required_record_value(&mut values, "approve")?.as_str() {
        "true" => true,
        "false" => false,
        other => {
            return Err(BridgeError::Runtime(format!(
                "malformed approval approve value: expected true or false, got {other}"
            )));
        }
    };

    Ok(WithdrawalApprovalRecord {
        facts: WithdrawalApprovalFacts {
            withdrawal_id_as_of,
            withdrawal_id_base_event_id,
            epoch,
            proposal_hash,
            authorized_transaction_name,
        },
        approve,
    })
}

fn required_record_value(
    values: &mut BTreeMap<String, String>,
    key: &str,
) -> Result<String, BridgeError> {
    values
        .remove(key)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| BridgeError::Runtime(format!("malformed approval record: missing {key}")))
}

pub fn approval_file_path(approval_dir: &Path, tx_id: &str) -> Result<PathBuf, BridgeError> {
    validate_approval_tx_id(tx_id)?;
    Ok(approval_dir.join(format!("{tx_id}.approve")))
}

fn approval_tmp_file_path(approval_dir: &Path, tx_id: &str) -> Result<PathBuf, BridgeError> {
    validate_approval_tx_id(tx_id)?;
    Ok(approval_dir.join(format!("{tx_id}.approve.tmp")))
}

fn validate_approval_tx_id(tx_id: &str) -> Result<(), BridgeError> {
    if tx_id.is_empty()
        || tx_id == "."
        || tx_id == ".."
        || tx_id.contains('/')
        || tx_id.contains('\\')
    {
        return Err(BridgeError::Runtime(format!(
            "invalid approval transaction id for file name: {tx_id:?}"
        )));
    }
    Ok(())
}

pub fn write_approval_record_atomic(
    approval_dir: &Path,
    facts: &WithdrawalApprovalFacts,
) -> Result<PathBuf, BridgeError> {
    fs::create_dir_all(approval_dir).map_err(|err| {
        BridgeError::Runtime(format!(
            "failed to create approval directory {}: {err}",
            approval_dir.display()
        ))
    })?;
    let final_path = approval_file_path(approval_dir, &facts.authorized_transaction_name)?;
    let tmp_path = approval_tmp_file_path(approval_dir, &facts.authorized_transaction_name)?;
    let rendered = render_approval_record(facts);

    {
        let mut file = fs::File::create(&tmp_path).map_err(|err| {
            BridgeError::Runtime(format!(
                "failed to create approval temp file {}: {err}",
                tmp_path.display()
            ))
        })?;
        file.write_all(rendered.as_bytes()).map_err(|err| {
            BridgeError::Runtime(format!(
                "failed to write approval temp file {}: {err}",
                tmp_path.display()
            ))
        })?;
        file.sync_all().map_err(|err| {
            BridgeError::Runtime(format!(
                "failed to sync approval temp file {}: {err}",
                tmp_path.display()
            ))
        })?;
    }

    fs::rename(&tmp_path, &final_path).map_err(|err| {
        BridgeError::Runtime(format!(
            "failed to move approval temp file {} to {}: {err}",
            tmp_path.display(),
            final_path.display()
        ))
    })?;

    Ok(final_path)
}

pub fn check_manual_submit_approval(
    config: &ManualSubmitApprovalConfig,
    row: &LiveWithdrawalView,
) -> Result<ManualSubmitApprovalDecision, BridgeError> {
    if !config.enabled {
        return Ok(ManualSubmitApprovalDecision::Approved);
    }

    let Some(expected_facts) = approval_facts_for_authorized_row(row)? else {
        return Ok(ManualSubmitApprovalDecision::Deferred(format!(
            "manual operator approval required for withdrawal {:?} epoch {}, but the sequencer row is not authorized",
            row.id, row.current_epoch
        )));
    };
    let approval_path = approval_file_path(
        &config.approval_dir, &expected_facts.authorized_transaction_name,
    )?;
    let contents = match fs::read_to_string(&approval_path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ManualSubmitApprovalDecision::Deferred(format!(
                "manual operator approval required for withdrawal {:?} epoch {}; no approval file at {}",
                row.id,
                row.current_epoch,
                approval_path.display()
            )));
        }
        Err(err) => {
            return Ok(ManualSubmitApprovalDecision::Deferred(format!(
                "manual operator approval required for withdrawal {:?} epoch {}; failed to read {}: {err}",
                row.id,
                row.current_epoch,
                approval_path.display()
            )));
        }
    };

    let record = match parse_approval_record(&contents) {
        Ok(record) => record,
        Err(err) => {
            return Ok(ManualSubmitApprovalDecision::Deferred(format!(
                "manual operator approval required for withdrawal {:?} epoch {}; approval file {} is malformed: {err}",
                row.id,
                row.current_epoch,
                approval_path.display()
            )));
        }
    };
    if !record.approve {
        return Ok(ManualSubmitApprovalDecision::Deferred(format!(
            "manual operator approval required for withdrawal {:?} epoch {}; approval file {} does not set approve=true",
            row.id,
            row.current_epoch,
            approval_path.display()
        )));
    }
    if record.facts != expected_facts {
        return Ok(ManualSubmitApprovalDecision::Deferred(format!(
            "manual operator approval required for withdrawal {:?} epoch {}; approval file {} does not match the authorized transaction facts",
            row.id,
            row.current_epoch,
            approval_path.display()
        )));
    }

    Ok(ManualSubmitApprovalDecision::Approved)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::shared::types::AtomBytes;
    use crate::withdrawal::types::WithdrawalId;

    fn sample_facts() -> WithdrawalApprovalFacts {
        WithdrawalApprovalFacts {
            withdrawal_id_as_of: crate::shared::types::zero_tip5_hash().to_base58(),
            withdrawal_id_base_event_id: "001122".to_string(),
            epoch: 7,
            proposal_hash: "proposal-hash".to_string(),
            authorized_transaction_name: "authorized-tx".to_string(),
        }
    }

    fn sample_authorized_row(facts: &WithdrawalApprovalFacts) -> LiveWithdrawalView {
        LiveWithdrawalView {
            id: WithdrawalId {
                as_of: nockchain_types::tx_engine::common::Hash::from_base58(
                    &facts.withdrawal_id_as_of,
                )
                .unwrap_or_else(|_| crate::shared::types::zero_tip5_hash()),
                base_event_id: AtomBytes(hex::decode(&facts.withdrawal_id_base_event_id).unwrap()),
            },
            recipient: None,
            gross_burned_amount: None,
            base_batch_end: None,
            withdrawal_nonce: None,
            current_epoch: facts.epoch,
            proposal_hash: Some(facts.proposal_hash.clone()),
            peer_commit_certificate: None,
            authorized_transaction_name: Some(facts.authorized_transaction_name.clone()),
            handoff_index: 0,
            turn_started_base_height: None,
            submit_attempt_count: 0,
            last_submit_attempt_base_height: None,
            last_submit_error: None,
            state: WithdrawalState::Authorized,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn approval_record_render_roundtrips_through_parser() {
        let facts = sample_facts();
        let record = parse_approval_record(&render_approval_record(&facts))
            .expect("rendered approval record should parse");

        assert_eq!(
            record,
            WithdrawalApprovalRecord {
                facts,
                approve: true
            }
        );
    }

    #[test]
    fn approval_record_parser_rejects_malformed_records() {
        assert!(parse_approval_record("withdrawal_id_as_of").is_err());
        assert!(parse_approval_record("unknown=value\n").is_err());
        assert!(parse_approval_record("approve=true\n").is_err());
        assert!(parse_approval_record(
            "withdrawal_id_as_of=a\nwithdrawal_id_as_of=b\napprove=true\n"
        )
        .is_err());
    }

    #[test]
    fn ctl_atomic_write_record_is_accepted_by_daemon_parser() {
        let dir = tempdir().expect("approval tempdir");
        let facts = sample_facts();

        let final_path = write_approval_record_atomic(dir.path(), &facts)
            .expect("atomic approval write should succeed");
        let parsed = parse_approval_record(
            &fs::read_to_string(final_path).expect("approval record should be readable"),
        )
        .expect("daemon parser should accept ctl-rendered record");

        assert_eq!(parsed.facts, facts);
        assert!(parsed.approve);
    }

    #[test]
    fn manual_approval_ignores_tmp_file_until_final_record_exists() {
        let dir = tempdir().expect("approval tempdir");
        let facts = sample_facts();
        let row = sample_authorized_row(&facts);
        let config = ManualSubmitApprovalConfig {
            enabled: true,
            approval_dir: dir.path().to_path_buf(),
        };
        let tmp_path =
            approval_tmp_file_path(dir.path(), &facts.authorized_transaction_name).unwrap();
        fs::write(tmp_path, render_approval_record(&facts)).expect("write tmp approval");

        let decision =
            check_manual_submit_approval(&config, &row).expect("approval check should not fail");
        assert!(matches!(
            decision,
            ManualSubmitApprovalDecision::Deferred(_)
        ));

        write_approval_record_atomic(dir.path(), &facts).expect("write final approval");
        let decision =
            check_manual_submit_approval(&config, &row).expect("approval check should not fail");
        assert_eq!(decision, ManualSubmitApprovalDecision::Approved);
    }
}
