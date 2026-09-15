#![allow(clippy::unwrap_used)]

use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{env, fs};

use anyhow::{anyhow, bail, Context, Result};
use aws_sdk_s3::config::{
    BehaviorVersion, Credentials, Region, RequestChecksumCalculation, ResponseChecksumValidation,
};
use aws_sdk_s3::Client as S3Client;
use bridge_dev::scenario::*;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

static E2E_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone)]
struct R2ScenarioJournal {
    endpoint: String,
    bucket: String,
    region: String,
    prefix: String,
    journal_id: String,
    access_key_id: String,
    secret_access_key: String,
}

impl R2ScenarioJournal {
    fn from_env(test_name: &str) -> Result<Option<Self>> {
        if !matches!(
            env::var(R2_E2E_ENABLE_ENV).ok().as_deref(),
            Some("1" | "true" | "yes")
        ) {
            eprintln!(
                "skipping R2-backed bridge-dev scenario; set {R2_E2E_ENABLE_ENV}=1 to run it"
            );
            return Ok(None);
        }
        let endpoint = r2_endpoint()?;
        let credentials = r2_credentials(&endpoint.account_id)?;
        let now = unix_now_for_test()?;
        let test_name = sanitize_key_segment(test_name);
        let run_id = format!("{now}-{}", std::process::id());
        let prefix_root = optional_env(R2_E2E_PREFIX_ENV)
            .unwrap_or_else(|| "withdrawal-sequencer-e2e/bridge-dev".to_string());
        Ok(Some(Self {
            endpoint: endpoint.endpoint,
            bucket: endpoint.bucket,
            region: optional_env(R2_E2E_REGION_ENV).unwrap_or_else(|| "auto".to_string()),
            prefix: format!("{prefix_root}/{test_name}/{run_id}"),
            journal_id: format!("bridge-dev-r2-e2e-{test_name}-{run_id}"),
            access_key_id: credentials.access_key_id,
            secret_access_key: credentials.secret_access_key,
        }))
    }

    fn env_overrides(&self) -> Vec<(String, String)> {
        vec![
            (
                BRIDGE_DEV_SEQUENCER_JOURNAL_ENABLED_ENV.to_string(),
                "1".to_string(),
            ),
            (
                "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_ENDPOINT".to_string(),
                self.endpoint.clone(),
            ),
            (
                "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_BUCKET".to_string(),
                self.bucket.clone(),
            ),
            (
                "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_REGION".to_string(),
                self.region.clone(),
            ),
            (
                "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_PREFIX".to_string(),
                self.prefix.clone(),
            ),
            (
                "WITHDRAWAL_SEQUENCER_JOURNAL_ID".to_string(),
                self.journal_id.clone(),
            ),
            (
                "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_ACCESS_KEY_ID".to_string(),
                self.access_key_id.clone(),
            ),
            (
                "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_SECRET_ACCESS_KEY".to_string(),
                self.secret_access_key.clone(),
            ),
        ]
    }

    fn event_prefix(&self) -> String {
        format!(
            "{}/v1/journals/{}/events/",
            self.prefix.trim_matches('/'),
            self.journal_id
        )
    }

    fn list_event_keys(&self) -> Result<Vec<String>> {
        let client = self.s3_client();
        let bucket = self.bucket.clone();
        let prefix = self.event_prefix();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("failed to build R2 cleanup runtime")?;
        runtime.block_on(async move {
            let mut page_cursor = None;
            let mut keys = Vec::new();
            loop {
                let mut request = client
                    .list_objects_v2()
                    .bucket(bucket.clone())
                    .prefix(prefix.clone());
                if let Some(token) = page_cursor {
                    request = request.continuation_token(token);
                }
                let output = request
                    .send()
                    .await
                    .context("failed to list bridge-dev R2 journal objects")?;
                keys.extend(
                    output
                        .contents()
                        .iter()
                        .filter_map(|object| object.key().map(ToString::to_string)),
                );
                page_cursor = output.next_continuation_token().map(ToString::to_string);
                if page_cursor.is_none() {
                    break;
                }
            }
            Ok(keys)
        })
    }

    fn assert_has_events(&self) -> Result<()> {
        let keys = self.list_event_keys()?;
        if keys.is_empty() {
            bail!(
                "R2 journal prefix {} did not contain any event objects",
                self.event_prefix()
            );
        }
        Ok(())
    }

    fn cleanup(&self) -> Result<()> {
        let keys = self.list_event_keys()?;
        if keys.is_empty() {
            return Ok(());
        }
        let client = self.s3_client();
        let bucket = self.bucket.clone();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("failed to build R2 cleanup runtime")?;
        runtime.block_on(async move {
            for key in keys {
                client
                    .delete_object()
                    .bucket(bucket.clone())
                    .key(key)
                    .send()
                    .await
                    .context("failed to delete bridge-dev R2 journal object")?;
            }
            Ok(())
        })
    }

    fn s3_client(&self) -> S3Client {
        let config = aws_sdk_s3::config::Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .endpoint_url(self.endpoint.clone())
            .region(Region::new(self.region.clone()))
            .credentials_provider(Credentials::new(
                self.access_key_id.clone(),
                self.secret_access_key.clone(),
                None,
                None,
                "bridge-dev-scenario",
            ))
            .force_path_style(true)
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired)
            .build();
        S3Client::from_conf(config)
    }
}

impl Drop for R2ScenarioJournal {
    fn drop(&mut self) {
        if matches!(
            env::var(R2_E2E_KEEP_OBJECTS_ENV).ok().as_deref(),
            Some("1" | "true" | "yes")
        ) {
            eprintln!(
                "leaving R2 bridge-dev journal objects for prefix {} because {}=1",
                self.event_prefix(),
                R2_E2E_KEEP_OBJECTS_ENV
            );
            return;
        }
        if let Err(err) = self.cleanup() {
            eprintln!(
                "R2 bridge-dev scenario cleanup failed: {}",
                redact(&err.to_string())
            );
        }
    }
}

#[derive(Debug, Clone)]
struct R2Endpoint {
    endpoint: String,
    account_id: String,
    bucket: String,
}

#[derive(Debug, Clone)]
struct R2Credentials {
    access_key_id: String,
    secret_access_key: String,
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn fresh_vnet_boot_reaches_status() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = ScenarioHarness::new("fresh-boot")?;

    scenario.spawn_fresh_cluster()?;
    let status = scenario.wait_for_status(Duration::from_secs(60))?;
    assert_cluster_available(&status)?;
    assert_sequencer_idle(&status)?;
    assert_queue_drained(&status)
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn deposit_happy_path_reaches_submitted_and_successful() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = ScenarioHarness::new("deposit-happy-path")?;

    scenario.spawn_fresh_cluster()?;
    let status = scenario.wait_for_status(Duration::from_secs(60))?;
    assert_cluster_available(&status)?;
    scenario.complete_deposit_on_all_nodes()?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    Ok(())
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn deposit_replays_after_all_bridges_were_down() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = ScenarioHarness::new("deposit-bridge-downtime")?;

    scenario.spawn_fresh_cluster()?;
    scenario.run_checked(&["stop", "bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_process_status(Duration::from_secs(120))?;
    assert_processes_not_running(&status, ALL_BRIDGE_COMPONENTS)?;
    scenario.run_checked_retry(
        &["deposit", "--amount-nicks", E2E_DEPOSIT_AMOUNT_NICKS],
        Duration::from_secs(E2E_DEPOSIT_SPEND_TIMEOUT_SECS),
    )?;
    scenario.run_checked(&["start", "bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_status(Duration::from_secs(240))?;
    assert_cluster_available(&status)?;
    let submitted = scenario.wait_for_deposit_on_node(ObservedDepositPhase::Submitted, 0, 360)?;
    let successful = scenario.wait_for_deposit_on_node(ObservedDepositPhase::Successful, 0, 480)?;
    assert_same_deposit_identity(
        &submitted, &successful, "post-downtime submitted", "post-downtime successful",
    )?;
    assert_successful_deposit_on_all_nodes(&mut scenario, &successful, 480)
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn deposit_state_survives_all_bridge_process_restart() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = ScenarioHarness::new("deposit-bridge-restart")?;

    scenario.spawn_fresh_cluster()?;
    let deposit = scenario.complete_deposit_on_all_nodes()?;
    scenario.restart_all_bridges()?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_successful_deposit_on_all_nodes(&mut scenario, &deposit, 360)
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn multiple_deposits_are_ordered_and_visible_on_all_nodes() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = ScenarioHarness::new("multiple-deposits")?;

    scenario.spawn_fresh_cluster()?;
    let first = scenario.complete_deposit_on_all_nodes()?;
    let second = scenario.complete_deposit_on_all_nodes_after(Some(first.nonce))?;
    assert_deposit_nonce_increased(&first, &second)?;
    assert_successful_deposit_on_all_nodes_after(&mut scenario, &second, 360, Some(first.nonce))
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn withdrawal_happy_path_reaches_executed() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = ScenarioHarness::new("withdrawal-happy-path")?;

    scenario.spawn_fresh_cluster()?;
    let status = scenario.wait_for_status(Duration::from_secs(60))?;
    assert_cluster_available(&status)?;
    scenario.complete_deposit_on_all_nodes()?;
    scenario.request_withdrawal_after_mint()?;
    scenario.wait_for_withdrawal_sequencer_confirmation()?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_sequencer_idle(&status)?;
    Ok(())
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn withdrawal_happy_path_spends_pre_bythos_bridge_deposit() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = ScenarioHarness::new("withdrawal-pre-bythos-deposit")?;
    let bythos_phase = E2E_PRE_BYTHOS_WITHDRAWAL_BYTHOS_PHASE;
    scenario.with_fakenet_bythos_phase(bythos_phase);

    scenario.spawn_fresh_cluster()?;
    let status = scenario.wait_for_status(Duration::from_secs(60))?;
    assert_cluster_available(&status)?;
    let initial_height = parse_status_nock_height(&status)?;
    if initial_height >= bythos_phase {
        bail!(
            "cluster started at nock height {initial_height}, already at or past bythos phase {bythos_phase}"
        );
    }

    scenario.complete_deposit_on_all_nodes()?;
    let post_deposit_height = scenario.current_nock_height()?;
    if post_deposit_height >= bythos_phase {
        bail!(
            "bridge multisig deposit was not pre-Bythos: post-deposit nock height {post_deposit_height}, bythos phase {bythos_phase}"
        );
    }

    scenario.wait_for_nock_height_at_least(bythos_phase, Duration::from_secs(600))?;
    scenario.request_withdrawal_after_mint()?;
    scenario.wait_for_withdrawal_sequencer_confirmation()?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_sequencer_idle(&status)?;
    Ok(())
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn withdrawal_happy_path_spends_mixed_pre_and_post_bythos_bridge_deposits() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = ScenarioHarness::new("withdrawal-mixed-bythos-deposits")?;
    let bythos_phase = E2E_PRE_BYTHOS_WITHDRAWAL_BYTHOS_PHASE;
    scenario.with_fakenet_bythos_phase(bythos_phase);

    scenario.spawn_fresh_cluster()?;
    let status = scenario.wait_for_status(Duration::from_secs(60))?;
    assert_cluster_available(&status)?;
    let initial_height = parse_status_nock_height(&status)?;
    if initial_height >= bythos_phase {
        bail!(
            "cluster started at nock height {initial_height}, already at or past bythos phase {bythos_phase}"
        );
    }

    let pre_bythos_deposit = scenario.complete_deposit_on_all_nodes()?;
    let post_pre_deposit_height = scenario.current_nock_height()?;
    if post_pre_deposit_height >= bythos_phase {
        bail!(
            "first bridge multisig deposit was not pre-Bythos: post-deposit nock height {post_pre_deposit_height}, bythos phase {bythos_phase}"
        );
    }

    scenario.wait_for_nock_height_at_least(bythos_phase, Duration::from_secs(600))?;
    let post_bythos_deposit =
        scenario.complete_deposit_on_all_nodes_after(Some(pre_bythos_deposit.nonce))?;
    assert_deposit_nonce_increased(&pre_bythos_deposit, &post_bythos_deposit)?;
    let post_second_deposit_height = scenario.current_nock_height()?;
    if post_second_deposit_height < bythos_phase {
        bail!(
            "second bridge multisig deposit was not post-Bythos: post-deposit nock height {post_second_deposit_height}, bythos phase {bythos_phase}"
        );
    }

    scenario.request_withdrawal_after_mint_amount(E2E_MIXED_INPUT_WITHDRAWAL_AMOUNT_NOCK)?;
    scenario.wait_for_withdrawal_sequencer_confirmation()?;
    scenario.assert_withdrawal_build_selected_input_count(2)?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_sequencer_idle(&status)?;
    scenario.assert_no_stop_conditions_in_logs()?;
    Ok(())
}

#[test]
#[ignore = "requires Tenderly VNET credentials, release bridge binaries, and sequencer ctl binary"]
fn withdrawal_manual_approval_defers_until_ctl_approval() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = ScenarioHarness::new("withdrawal-manual-approval")?;
    scenario.extend_env_overrides([(MANUAL_SUBMIT_APPROVAL_ENV.to_string(), "1".to_string())]);
    scenario.ensure_sequencer_ctl_binary()?;

    scenario.spawn_fresh_cluster()?;
    let status = scenario.wait_for_status(Duration::from_secs(60))?;
    assert_cluster_available(&status)?;
    scenario.complete_deposit_on_all_nodes()?;
    scenario.request_withdrawal_after_mint()?;

    let pending = scenario.wait_for_withdrawal_phase("Pending", "--pending", 240)?;
    let ready = scenario.wait_for_withdrawal_phase_for("Ready", "--ready", 480, Some(&pending))?;
    let authorized = scenario.wait_for_withdrawal_manual_approval_facts(&ready, 480)?;
    scenario.assert_withdrawal_not_submitted_before_manual_approval(&pending)?;

    let tx_id = authorized.authorized_transaction_name.as_str();
    let pending_approvals = scenario.run_sequencer_ctl_checked(&["pending-approvals"])?;
    assert_contains_all(&pending_approvals, &["manual_submit_approval=true"])?;
    assert_contains(
        &pending_approvals,
        &format!("proposal_hash={}", authorized.proposal_hash),
    )?;
    assert_contains(
        &pending_approvals,
        &format!("authorized_transaction_name={tx_id}"),
    )?;

    let approval_facts =
        scenario.run_sequencer_ctl_checked(&["show-approval", "--tx-id", tx_id])?;
    assert_contains_all(
        &approval_facts,
        &[
            "manual_submit_approval=true", "withdrawal_id_as_of=", "withdrawal_id_base_event_id=",
            "epoch=",
        ],
    )?;
    assert_contains(
        &approval_facts,
        &format!("proposal_hash={}", authorized.proposal_hash),
    )?;
    assert_contains(
        &approval_facts,
        &format!("authorized_transaction_name={tx_id}"),
    )?;

    let approval =
        scenario.run_sequencer_ctl_checked(&["approve-withdrawal", "--tx-id", tx_id, "--yes"])?;
    assert_contains_all(&approval, &["approval_written="])?;
    assert_contains(&approval, &format!("authorized_transaction_name={tx_id}"))?;

    let submitted =
        scenario.wait_for_withdrawal_phase_for("Submitted", "--submitted", 600, Some(&pending))?;
    let executed =
        scenario.wait_for_withdrawal_phase_for("Executed", "--executed", 720, Some(&pending))?;
    assert_withdrawal_progression(&pending, &authorized, &submitted, &executed)?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_sequencer_idle(&status)
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn withdrawal_executes_after_ready_bridge_restart() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = ScenarioHarness::new("withdrawal-ready-restart")?;

    scenario.spawn_fresh_cluster()?;
    scenario.complete_deposit_on_all_nodes()?;
    scenario.request_withdrawal_after_mint()?;
    let pending = scenario.wait_for_withdrawal_phase("Pending", "--pending", 240)?;
    let ready = scenario.wait_for_withdrawal_phase_for("Ready", "--ready", 480, Some(&pending))?;
    scenario.restart_all_bridges()?;
    let submitted =
        scenario.wait_for_withdrawal_phase_for("Submitted", "--submitted", 600, Some(&pending))?;
    let executed =
        scenario.wait_for_withdrawal_phase_for("Executed", "--executed", 720, Some(&pending))?;
    assert_withdrawal_progression(&pending, &ready, &submitted, &executed)?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_sequencer_idle(&status)
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn withdrawal_confirms_after_sequencer_restart_from_submitted() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = ScenarioHarness::new("withdrawal-submitted-sequencer-restart")?;

    scenario.spawn_fresh_cluster()?;
    scenario.complete_deposit_on_all_nodes()?;
    scenario.request_withdrawal_after_mint()?;
    let pending = scenario.wait_for_withdrawal_phase("Pending", "--pending", 240)?;
    let ready = scenario.wait_for_withdrawal_phase_for("Ready", "--ready", 480, Some(&pending))?;
    let submitted =
        scenario.wait_for_withdrawal_phase_for("Submitted", "--submitted", 600, Some(&pending))?;

    scenario.run_checked(&["stop", "node"])?;
    let stopped = scenario.wait_for_process_status(Duration::from_secs(120))?;
    assert_processes_not_running(&stopped, &["node"])?;

    scenario.run_checked(&["start", "node"])?;
    let status = scenario.wait_for_status(Duration::from_secs(240))?;
    assert_cluster_available(&status)?;

    let executed =
        scenario.wait_for_withdrawal_phase_for("Executed", "--executed", 720, Some(&pending))?;
    assert_withdrawal_progression(&pending, &ready, &submitted, &executed)?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_sequencer_idle(&status)
}

#[test]
#[ignore = "requires Tenderly VNET credentials, R2 credentials, and release bridge binaries"]
fn withdrawal_sequencer_rebuilds_from_r2_after_sqlite_wipe() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let Some(r2_journal) = R2ScenarioJournal::from_env("sequencer-sqlite-wipe")? else {
        return Ok(());
    };
    let mut scenario = ScenarioHarness::new("withdrawal-r2-sequencer-recovery")?;
    scenario.extend_env_overrides(r2_journal.env_overrides());

    scenario.spawn_fresh_cluster()?;
    let status = scenario.wait_for_status(Duration::from_secs(60))?;
    assert_cluster_available(&status)?;
    scenario.complete_deposit_on_all_nodes()?;
    scenario.request_withdrawal_after_mint()?;
    let pending = scenario.wait_for_withdrawal_phase("Pending", "--pending", 240)?;
    let ready = scenario.wait_for_withdrawal_phase_for("Ready", "--ready", 480, Some(&pending))?;
    r2_journal.assert_has_events()?;

    scenario.run_checked(&["stop", "node"])?;
    let status = scenario.wait_for_process_status(Duration::from_secs(120))?;
    assert_processes_not_running(&status, &["node"])?;
    let sqlite_path = scenario.sequencer_sqlite_path();
    if !sqlite_path.exists() {
        bail!(
            "sequencer sqlite did not exist before wipe: {}",
            sqlite_path.display()
        );
    }
    scenario.remove_sequencer_sqlite()?;

    scenario.run_checked(&["start", "node"])?;
    let status = scenario.wait_for_status(Duration::from_secs(240))?;
    assert_contains_all(&status, &["bridge_streams:", "sequencer_status:"])?;
    scenario.run_checked(&["start", "bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_status(Duration::from_secs(240))?;
    assert_cluster_available(&status)?;
    let submitted =
        scenario.wait_for_withdrawal_phase_for("Submitted", "--submitted", 600, Some(&pending))?;
    let executed =
        scenario.wait_for_withdrawal_phase_for("Executed", "--executed", 720, Some(&pending))?;
    assert_withdrawal_progression(&pending, &ready, &submitted, &executed)?;
    r2_journal.assert_has_events()?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_sequencer_idle(&status)
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn withdrawal_catches_up_after_all_bridge_processes_were_down() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = ScenarioHarness::new("withdrawal-bridge-downtime")?;

    scenario.spawn_fresh_cluster()?;
    scenario.complete_deposit_on_all_nodes()?;
    scenario.run_checked(&["stop", "bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_process_status(Duration::from_secs(120))?;
    assert_processes_not_running(&status, ALL_BRIDGE_COMPONENTS)?;
    scenario.request_withdrawal_after_mint()?;
    scenario.run_checked(&["start", "bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_status(Duration::from_secs(240))?;
    assert_cluster_available(&status)?;
    scenario.wait_for_withdrawal_sequencer_confirmation()?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_sequencer_idle(&status)
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn two_node_degraded_withdrawal_still_executes() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = ScenarioHarness::new("two-node-degraded-withdrawal")?;

    scenario.spawn_fresh_cluster()?;
    scenario.complete_deposit_on_all_nodes()?;
    scenario.run_checked(&["stop", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_process_status(Duration::from_secs(120))?;
    assert_processes_not_running(&status, &["bridge-3", "bridge-4"])?;
    scenario.request_withdrawal_after_mint()?;
    scenario.wait_for_withdrawal_sequencer_confirmation()?;
    scenario.run_checked(&["start", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_processes_running(&status, &["bridge-3", "bridge-4"])?;
    assert_sequencer_idle(&status)
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn two_node_degraded_deposit_still_completes() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = ScenarioHarness::new("two-node-degraded-deposit")?;

    scenario.spawn_fresh_cluster()?;
    scenario.run_checked(&["stop", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_process_status(Duration::from_secs(120))?;
    assert_processes_not_running(&status, &["bridge-3", "bridge-4"])?;
    scenario.run_checked_retry(
        &["deposit", "--amount-nicks", E2E_DEPOSIT_AMOUNT_NICKS],
        Duration::from_secs(E2E_DEPOSIT_SPEND_TIMEOUT_SECS),
    )?;
    let submitted = scenario.wait_for_deposit_on_node(ObservedDepositPhase::Submitted, 0, 240)?;
    let deposit = scenario.wait_for_deposit_on_node(ObservedDepositPhase::Successful, 0, 360)?;
    assert_same_deposit_identity(
        &submitted, &deposit, "degraded submitted", "degraded successful",
    )?;
    scenario.run_checked(&["start", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_processes_running(&status, &["bridge-3", "bridge-4"])?;
    assert_same_deposit(
        &deposit,
        &scenario.wait_for_deposit_on_node(ObservedDepositPhase::Successful, 3, 360)?,
        "bridge-3",
    )?;
    assert_same_deposit(
        &deposit,
        &scenario.wait_for_deposit_on_node(ObservedDepositPhase::Successful, 4, 360)?,
        "bridge-4",
    )?;
    Ok(())
}

fn e2e_guard() -> MutexGuard<'static, ()> {
    E2E_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn e2e_enabled() -> Result<bool> {
    match env::var(E2E_ENABLE_ENV).ok().as_deref() {
        Some("1") | Some("true") | Some("yes") => {
            let missing = REQUIRED_E2E_ENV
                .iter()
                .copied()
                .filter(|key| {
                    env::var(key)
                        .ok()
                        .is_none_or(|value| value.trim().is_empty())
                })
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                bail!(
                    "{E2E_ENABLE_ENV}=1 but required Tenderly env vars are missing: {}",
                    missing.join(", ")
                );
            }
            Ok(true)
        }
        _ => {
            eprintln!("skipping bridge-dev scenario; set {E2E_ENABLE_ENV}=1 to run it");
            Ok(false)
        }
    }
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn required_r2_env(name: &str) -> Result<String> {
    optional_env(name).ok_or_else(|| anyhow!("{name} must be set when {R2_E2E_ENABLE_ENV}=1"))
}

fn r2_endpoint() -> Result<R2Endpoint> {
    if let Some(url) = optional_env(R2_E2E_URL_ENV) {
        let parsed = reqwest::Url::parse(&url).context("BRIDGE_R2_TEST_URL must be a valid URL")?;
        let host = parsed
            .host_str()
            .ok_or_else(|| anyhow!("BRIDGE_R2_TEST_URL must include a host"))?;
        let account_id = host
            .split_once('.')
            .map(|(account_id, _)| account_id.to_string())
            .ok_or_else(|| {
                anyhow!("BRIDGE_R2_TEST_URL host must start with the Cloudflare account id")
            })?;
        let endpoint = format!("{}://{}", parsed.scheme(), host);
        let bucket = parsed.path().trim_matches('/');
        if bucket.is_empty() || bucket.contains('/') {
            bail!("BRIDGE_R2_TEST_URL must include exactly one bucket path segment");
        }
        return Ok(R2Endpoint {
            endpoint,
            account_id,
            bucket: bucket.to_string(),
        });
    }

    let endpoint = required_r2_env(R2_E2E_ENDPOINT_ENV)?;
    let parsed =
        reqwest::Url::parse(&endpoint).context("BRIDGE_R2_TEST_ENDPOINT must be a valid URL")?;
    let account_id = parsed
        .host_str()
        .and_then(|host| {
            host.split_once('.')
                .map(|(account_id, _)| account_id.to_string())
        })
        .ok_or_else(|| {
            anyhow!("BRIDGE_R2_TEST_ENDPOINT host must start with the Cloudflare account id")
        })?;
    Ok(R2Endpoint {
        endpoint,
        account_id,
        bucket: required_r2_env(R2_E2E_BUCKET_ENV)?,
    })
}

fn cloudflare_token_id(account_id: &str, token: &str) -> Result<String> {
    let url = format!("https://api.cloudflare.com/client/v4/accounts/{account_id}/tokens/verify");
    let response = reqwest::blocking::Client::new()
        .get(url)
        .bearer_auth(token)
        .send()
        .context("failed to verify Cloudflare R2 token")?
        .error_for_status()
        .context("Cloudflare R2 token verification returned an error status")?
        .json::<serde_json::Value>()
        .context("Cloudflare R2 token verification returned invalid JSON")?;
    if !response["success"].as_bool().unwrap_or(false) {
        bail!("Cloudflare R2 token verification failed");
    }
    let status = response["result"]["status"].as_str().unwrap_or("");
    if status != "active" {
        bail!("Cloudflare R2 token is not active");
    }
    response["result"]["id"]
        .as_str()
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("Cloudflare R2 token verification did not return a token id"))
}

fn r2_credentials(account_id: &str) -> Result<R2Credentials> {
    if let (Some(access_key_id), Some(secret_access_key)) = (
        optional_env(R2_E2E_ACCESS_KEY_ID_ENV),
        optional_env(R2_E2E_SECRET_ACCESS_KEY_ENV),
    ) {
        return Ok(R2Credentials {
            access_key_id,
            secret_access_key,
        });
    }

    if let Some(token) = optional_env(R2_E2E_TOKEN_ENV) {
        let access_key_id = cloudflare_token_id(account_id, &token)?;
        let secret_access_key = format!("{:x}", Sha256::digest(token.as_bytes()));
        return Ok(R2Credentials {
            access_key_id,
            secret_access_key,
        });
    }

    Ok(R2Credentials {
        access_key_id: required_r2_env(R2_E2E_ACCESS_KEY_ID_ENV)?,
        secret_access_key: required_r2_env(R2_E2E_SECRET_ACCESS_KEY_ENV)?,
    })
}

fn unix_now_for_test() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before unix epoch")?
        .as_secs())
}

fn sanitize_key_segment(raw: &str) -> String {
    raw.trim_matches('/')
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[test]
fn process_state_helpers_read_status_process_rows() {
    let status =
        "processes:\n  bridge-3   exited(0)    pid=123\n  bridge-4   running      pid=456\n";
    assert_eq!(process_state(status, "bridge-3"), Some("exited(0)"));
    assert_eq!(process_state(status, "bridge-4"), Some("running"));
    assert_processes_not_running(status, &["bridge-3"]).unwrap();
    assert_processes_running(status, &["bridge-4"]).unwrap();
    assert!(assert_processes_running(status, &["bridge-3"]).is_err());
    assert!(assert_processes_not_running(status, &["bridge-4"]).is_err());
}

#[test]
fn bridge_stream_helpers_ignore_process_rows() {
    let status = "\
processes:
  bridge-0   running      pid=123
bridge_streams:
  bridge-0 running_state=Running base_height=1 nock_height=2 nockchain_api=Connected batch_status=idle unhealthy_peers=0
";
    assert_eq!(
        bridge_stream_line(status, 0).unwrap(),
        "bridge-0 running_state=Running base_height=1 nock_height=2 nockchain_api=Connected batch_status=idle unhealthy_peers=0"
    );
    assert_bridge_streams_available(status, &[0]).unwrap();
}

#[test]
fn reboot_state_helper_accepts_checkpoint_pma_or_event_log() {
    let tempdir = TempDir::new().unwrap();
    let data_dir = tempdir.path();
    let checkpoint_dir = data_dir.join("checkpoints");
    let pma_dir = data_dir.join("pma");
    fs::create_dir_all(&checkpoint_dir).unwrap();
    fs::create_dir_all(&pma_dir).unwrap();

    assert!(!bridge_data_dir_has_reboot_state(data_dir));
    assert!(!checkpoint_dir_has_nonempty_checkpoint(&checkpoint_dir));
    fs::write(checkpoint_dir.join("0.chkjam"), []).unwrap();
    assert!(!bridge_data_dir_has_reboot_state(data_dir));
    fs::write(checkpoint_dir.join("1.chkjam"), [1u8]).unwrap();
    assert!(checkpoint_dir_has_nonempty_checkpoint(&checkpoint_dir));
    assert!(bridge_data_dir_has_reboot_state(data_dir));

    fs::remove_file(checkpoint_dir.join("1.chkjam")).unwrap();
    fs::write(pma_dir.join("epoch.pma"), [1u8]).unwrap();
    assert!(bridge_data_dir_has_reboot_state(data_dir));

    fs::remove_file(pma_dir.join("epoch.pma")).unwrap();
    fs::write(data_dir.join("event-log.sqlite3"), [1u8]).unwrap();
    assert!(bridge_data_dir_has_reboot_state(data_dir));
}

#[test]
fn parses_successful_deposit_wait_output() {
    let deposit = parse_observed_deposit(
        "deposit successful: nonce=7 amount=42 recipient=0xabc tx_id=deposit-tx\n",
        ObservedDepositPhase::Successful,
    )
    .unwrap();
    assert_eq!(
        deposit,
        ObservedDeposit {
            nonce: 7,
            amount: 42,
            recipient: "0xabc".to_string(),
            tx_id: "deposit-tx".to_string(),
        }
    );
}

#[test]
fn parses_withdrawal_wait_output() {
    let withdrawal = parse_observed_withdrawal(
        "withdrawal Executed: id=aa:bb as_of=aa base_event=bb nonce=9 proposal_status=confirmed sequenced_state=confirmed handoff_owner=bridge-2 transaction_name=tx proposal_hash=hash authorized_transaction_name=authed\n",
        "Executed",
    )
    .unwrap();
    assert_eq!(withdrawal.id, "aa:bb");
    assert_eq!(withdrawal.as_of, "aa");
    assert_eq!(withdrawal.base_event, "bb");
    assert_eq!(withdrawal.nonce, "9");
    assert_eq!(withdrawal.sequenced_state, "confirmed");
    assert_eq!(withdrawal.authorized_transaction_name, "authed");
}
