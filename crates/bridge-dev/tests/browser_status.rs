use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::Address;
use anyhow::Result;
use async_trait::async_trait;
use bridge::shared::ingress::proto as ingress_proto;
use bridge_dev::browser_status::{
    BrowserStatusAdapter, BrowserStatusAdapterConfig, BrowserStatusSource,
};
use nockchain_types::common::Hash as NockHash;
use serde_json::{json, Value};

const RUN_ID: &str = "browser-status-test-run";
const POLICY_ID: &str = "withdrawal-policy-v1";
const PROTOCOL_ID: &str = "WithdrawalWireV1";

#[tokio::test]
async fn readiness_and_pending_status_come_from_public_source() -> Result<()> {
    let fixture = Fixture::new(ingress_proto::PublicWithdrawalStatus::WithdrawalPending)?;
    let adapter =
        BrowserStatusAdapter::start_with_source(fixture.config.clone(), fixture.source()).await?;
    let readiness: Value = reqwest::get(adapter.endpoint()).await?.json().await?;
    assert_eq!(readiness["schemaVersion"], 1);
    assert_eq!(readiness["ready"], true);
    assert_eq!(readiness["withdrawalsEnabled"], true);
    assert_eq!(readiness["operatorAdmissionEnabled"], true);
    assert_eq!(readiness["contractGateEnabled"], true);
    assert_eq!(readiness["minimumGrossNocks"], "100000");
    assert_eq!(readiness["minimumGrossNicks"], "6553600000");
    assert_eq!(readiness["minimumGrossBaseUnits"], "1000000000000000000000");
    assert_eq!(readiness["baseUnitsPerNock"], "10000000000000000");
    assert_eq!(
        readiness["nockTokenAddress"],
        format!("{:#x}", fixture.config.nock)
    );
    assert_eq!(readiness["bridgeThreshold"], 3);

    let status: Value = reqwest::get(format!(
        "{}?base_event_id=0x{}&account={:#x}",
        adapter.endpoint(),
        hex::encode(fixture.event_id),
        fixture.account
    ))
    .await?
    .json()
    .await?;
    assert_eq!(status["status"], "submitted");
    assert_eq!(status["schemaVersion"], 2);
    assert_eq!(status["resolution"], "found");
    assert_eq!(status["revision"], "8");
    assert_eq!(status["terminalProof"], false);
    assert_eq!(
        status["baseEventId"],
        format!("0x{}", hex::encode(fixture.event_id))
    );
    adapter.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn confirmed_status_waits_for_matching_terminal_proof() -> Result<()> {
    let fixture = Fixture::new(ingress_proto::PublicWithdrawalStatus::Confirmed)?;
    let adapter =
        BrowserStatusAdapter::start_with_source(fixture.config.clone(), fixture.source()).await?;
    let url = format!(
        "{}?base_event_id=0x{}&account={:#x}",
        adapter.endpoint(),
        hex::encode(fixture.event_id),
        fixture.account
    );
    let before: Value = reqwest::get(&url).await?.json().await?;
    assert_eq!(before["status"], "sequencer_confirmed");
    assert_eq!(before["terminalProof"], false);
    assert_eq!(before["revision"], "8");

    let proof = json!({
        "schema_version": 1,
        "run_id": RUN_ID,
        "terminal": true,
        "nock_transaction_id": fixture.transaction_id,
        "nock_block_id": fixture.block_id.to_base58(),
        "burn_count": 1,
        "payout_count": 1,
        "payout_nicks": "6534153373",
        "proof": {"source": "test"}
    });
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&fixture.config.terminal_proof_path)?;
    file.write_all(&serde_json::to_vec_pretty(&proof)?)?;
    file.sync_all()?;

    let after: Value = reqwest::get(&url).await?.json().await?;
    assert_eq!(after["status"], "terminal");
    assert_eq!(after["terminalProof"], true);
    assert_eq!(after["revision"], "9");
    assert_eq!(after["nockTransactionId"], fixture.transaction_id);
    assert_eq!(after["nockBlockId"], fixture.block_id.to_base58());
    assert_eq!(after["actualPayoutNicks"], "6534153373");
    adapter.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn mismatched_account_is_rejected_without_hiding_the_record() -> Result<()> {
    let fixture = Fixture::new(ingress_proto::PublicWithdrawalStatus::WithdrawalPending)?;
    let adapter =
        BrowserStatusAdapter::start_with_source(fixture.config.clone(), fixture.source()).await?;
    let response = reqwest::get(format!(
        "{}?base_event_id=0x{}&account={:#x}",
        adapter.endpoint(),
        hex::encode(fixture.event_id),
        Address::from([0x99; 20])
    ))
    .await?;
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: Value = response.json().await?;
    assert!(body["error"]
        .as_str()
        .is_some_and(|message| message.contains("does not own")));
    adapter.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn history_and_quote_preserve_public_revisioned_facts() -> Result<()> {
    let fixture = Fixture::new(ingress_proto::PublicWithdrawalStatus::WithdrawalPending)?;
    let adapter =
        BrowserStatusAdapter::start_with_source(fixture.config.clone(), fixture.source()).await?;
    let history: Value = reqwest::get(format!(
        "{}?history=1&account={:#x}&limit=50",
        adapter.endpoint(),
        fixture.account
    ))
    .await?
    .json()
    .await?;
    assert_eq!(history["schemaVersion"], 1);
    assert_eq!(history["revision"], "8");
    assert_eq!(history["records"][0]["schemaVersion"], 2);
    assert_eq!(history["records"][0]["revision"], "8");

    let destination = NockHash::from_limbs(&[9, 8, 7, 6, 5]).to_base58();
    let quote: Value = reqwest::get(format!(
        "{}?quote=1&gross_amount_nicks=6553600000&destination_lock_root={destination}",
        adapter.endpoint()
    ))
    .await?
    .json()
    .await?;
    assert_eq!(quote["schemaVersion"], 1);
    assert_eq!(quote["available"], true);
    assert_eq!(quote["grossAmountNicks"], "6553600000");
    assert_eq!(quote["revision"], "4");
    adapter.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn development_adapter_allows_only_explicit_loopback_browser_origins() -> Result<()> {
    let fixture = Fixture::new(ingress_proto::PublicWithdrawalStatus::WithdrawalPending)?;
    let adapter =
        BrowserStatusAdapter::start_with_source(fixture.config.clone(), fixture.source()).await?;
    let client = reqwest::Client::new();
    let allowed = client
        .get(adapter.endpoint())
        .header("Origin", "http://127.0.0.1:3000")
        .send()
        .await?;
    assert_eq!(allowed.status(), reqwest::StatusCode::OK);
    assert_eq!(
        allowed
            .headers()
            .get(reqwest::header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&reqwest::header::HeaderValue::from_static(
            "http://127.0.0.1:3000"
        ))
    );
    assert_eq!(
        allowed.headers().get(reqwest::header::CACHE_CONTROL),
        Some(&reqwest::header::HeaderValue::from_static(
            "no-store, max-age=0"
        ))
    );

    let denied = client
        .get(adapter.endpoint())
        .header("Origin", "https://attacker.example")
        .send()
        .await?;
    assert_eq!(denied.status(), reqwest::StatusCode::FORBIDDEN);
    assert_ne!(
        denied
            .headers()
            .get(reqwest::header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&reqwest::header::HeaderValue::from_static("*"))
    );
    adapter.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn public_status_route_regresses_confirmed_facts_on_reorg() -> Result<()> {
    let mut fixture = Fixture::new(ingress_proto::PublicWithdrawalStatus::Confirmed)?;
    let record = fixture
        .source
        .withdrawal
        .withdrawal
        .as_mut()
        .expect("fixture withdrawal");
    record.resolution = ingress_proto::PublicWithdrawalResolution::Reorged as i32;
    record.revision = 5;
    record.canonical_base_event = false;
    record.recovery_generation = Some(2);
    record.invalidated_block_height = Some(120);
    record.invalidated_block_id = Some(vec![0x99; 40]);
    record.prior_status = "confirmed".to_owned();
    record.recovery_reason = "confirmed inclusion was orphaned".to_owned();
    record.nock_transaction_name = None;
    record.nock_confirmed_height = None;
    record.nock_confirmed_block_id = None;
    record.net_amount_nicks = None;
    let adapter =
        BrowserStatusAdapter::start_with_source(fixture.config.clone(), fixture.source()).await?;
    let status: Value = reqwest::get(format!(
        "{}?base_event_id=0x{}&account={:#x}",
        adapter.endpoint(),
        hex::encode(fixture.event_id),
        fixture.account
    ))
    .await?
    .json()
    .await?;
    assert_eq!(status["status"], "reorg_hold");
    assert_eq!(status["resolution"], "reorged");
    assert_eq!(status["revision"], "10");
    assert_eq!(status["terminalProof"], false);
    assert_eq!(status["nockTransactionId"], Value::Null);
    assert_eq!(status["invalidatedBlockNumber"], "120");
    assert_eq!(
        status["invalidatedBlockHash"],
        format!("0x{}", "99".repeat(40))
    );
    adapter.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn malformed_public_resolution_is_an_explicit_failed_status() -> Result<()> {
    let mut fixture = Fixture::new(ingress_proto::PublicWithdrawalStatus::WithdrawalPending)?;
    fixture.source.withdrawal = ingress_proto::GetPublicWithdrawalResponse {
        found: false,
        withdrawal: None,
        resolution: ingress_proto::PublicWithdrawalResolution::MalformedBurn as i32,
        support_hint: "missing_calldata_trailer".to_owned(),
        revision: 6,
    };
    let adapter =
        BrowserStatusAdapter::start_with_source(fixture.config.clone(), fixture.source()).await?;
    let status: Value = reqwest::get(format!(
        "{}?base_event_id=0x{}&account={:#x}",
        adapter.endpoint(),
        hex::encode(fixture.event_id),
        fixture.account
    ))
    .await?
    .json()
    .await?;
    assert_eq!(status["status"], "failed");
    assert_eq!(status["resolution"], "malformed_burn");
    assert_eq!(status["revision"], "12");
    assert_eq!(status["terminalProof"], false);
    assert_eq!(status["reason"], "missing_calldata_trailer");
    adapter.shutdown().await?;
    Ok(())
}

#[derive(Clone)]
struct StaticSource {
    readiness: ingress_proto::GetPublicWithdrawalReadinessResponse,
    withdrawal: ingress_proto::GetPublicWithdrawalResponse,
    history: ingress_proto::ListPublicWithdrawalsByBurnerResponse,
    quote: ingress_proto::GetPublicWithdrawalQuoteResponse,
}

#[async_trait]
impl BrowserStatusSource for StaticSource {
    async fn get_readiness(
        &self,
        _request: ingress_proto::GetPublicWithdrawalReadinessRequest,
    ) -> Result<ingress_proto::GetPublicWithdrawalReadinessResponse, String> {
        Ok(self.readiness.clone())
    }

    async fn get_withdrawal(
        &self,
        _request: ingress_proto::GetPublicWithdrawalRequest,
    ) -> Result<ingress_proto::GetPublicWithdrawalResponse, String> {
        Ok(self.withdrawal.clone())
    }

    async fn list_withdrawals_by_burner(
        &self,
        _request: ingress_proto::ListPublicWithdrawalsByBurnerRequest,
    ) -> Result<ingress_proto::ListPublicWithdrawalsByBurnerResponse, String> {
        Ok(self.history.clone())
    }

    async fn get_withdrawal_quote(
        &self,
        _request: ingress_proto::GetPublicWithdrawalQuoteRequest,
    ) -> Result<ingress_proto::GetPublicWithdrawalQuoteResponse, String> {
        Ok(self.quote.clone())
    }
}

struct Fixture {
    config: BrowserStatusAdapterConfig,
    source: StaticSource,
    event_id: [u8; 32],
    account: Address,
    transaction_id: String,
    block_id: NockHash,
    _preserved_root: PathBuf,
}

impl Fixture {
    fn new(status: ingress_proto::PublicWithdrawalStatus) -> Result<Self> {
        let preserved_root = tempfile::TempDir::new()?.keep();
        let terminal_proof_path = preserved_root.join("browser-terminal-proof.json");
        let nock = Address::from([0x11; 20]);
        let message_inbox = Address::from([0x22; 20]);
        let account = Address::from([0x44; 20]);
        let event_id = [0x33; 32];
        let block_id = NockHash::from_limbs(&[5, 4, 3, 2, 1]);
        let transaction_id = NockHash::from_limbs(&[1, 2, 3, 4, 5]).to_base58();
        let deployment = ingress_proto::PublicWithdrawalDeployment {
            base_chain_id: 31_338,
            nock_contract_address: nock.as_slice().to_vec(),
            policy_id: POLICY_ID.to_owned(),
            protocol_id: PROTOCOL_ID.to_owned(),
        };
        let confirmed = status == ingress_proto::PublicWithdrawalStatus::Confirmed;
        let now = unix_ms();
        let record = ingress_proto::PublicWithdrawalRecord {
            schema_version: 1,
            deployment: Some(deployment),
            base: None,
            base_event_id: event_id.to_vec(),
            withdrawal_id: None,
            burner: account.as_slice().to_vec(),
            gross_amount_base_units: "1000010000000000000000".to_owned(),
            gross_amount_nicks: "6553665536".to_owned(),
            net_amount_nicks: confirmed.then(|| "6534153373".to_owned()),
            destination_lock_root: NockHash::from_limbs(&[9, 8, 7, 6, 5])
                .to_be_limb_bytes()
                .to_vec(),
            status: status as i32,
            resolution: ingress_proto::PublicWithdrawalResolution::Found as i32,
            revision: 4,
            canonical_base_event: true,
            base_block_number: Some(100),
            base_block_hash: Some(vec![0x55; 32]),
            observed_at_unix_ms: Some(now),
            updated_at_unix_ms: Some(now),
            nock_transaction_name: confirmed.then(|| transaction_id.clone()),
            nock_confirmed_height: confirmed.then_some(120),
            nock_confirmed_block_id: confirmed.then(|| block_id.to_be_limb_bytes().to_vec()),
            confirmed_at_unix_ms: confirmed.then_some(now),
            support_hint: String::new(),
            recovery_generation: None,
            invalidated_block_height: None,
            invalidated_block_id: None,
            prior_status: String::new(),
            recovery_reason: String::new(),
        };
        let config = BrowserStatusAdapterConfig {
            upstream_endpoint: "http://127.0.0.1:1".to_owned(),
            run_dir: preserved_root.clone(),
            terminal_proof_path,
            run_id: RUN_ID.to_owned(),
            base_chain_id: 31_338,
            nock,
            message_inbox,
            bridge_signer_pkhs: vec![
                "signer-0".to_owned(),
                "signer-1".to_owned(),
                "signer-2".to_owned(),
            ],
            bridge_threshold: 3,
            policy_id: POLICY_ID.to_owned(),
            protocol_id: PROTOCOL_ID.to_owned(),
            iris_sdk_version: "0.3.3".to_owned(),
        };
        Ok(Self {
            config,
            source: StaticSource {
                readiness: ingress_proto::GetPublicWithdrawalReadinessResponse {
                    readiness: ingress_proto::PublicWithdrawalReadiness::Ready as i32,
                    accepting_new_withdrawals: true,
                    reasons: vec![ingress_proto::PublicWithdrawalReadinessReason::Healthy as i32],
                    policy_id: POLICY_ID.to_owned(),
                    protocol_id: PROTOCOL_ID.to_owned(),
                    confirmed_base_height: Some(100),
                    indexed_base_height: Some(100),
                    reconciled_journal_sequence: Some(4),
                    base_observed_at_unix_ms: Some(now),
                    minimum_gross_nocks: "100000".to_owned(),
                    minimum_gross_nicks: "6553600000".to_owned(),
                    minimum_gross_base_units: "1000000000000000000000".to_owned(),
                    base_units_per_nock: "10000000000000000".to_owned(),
                    nicks_per_nock: "65536".to_owned(),
                    base_units_per_nick: "152587890625".to_owned(),
                    maximum_nicks: u64::MAX.to_string(),
                    bridge_fee_nicks_per_started_nock: "195".to_owned(),
                    operator_admission_enabled: true,
                    contract_gate_enabled: Some(true),
                    observed_nockchain_height: Some(120),
                    updated_at_unix_ms: now,
                    support_hint: String::new(),
                },
                withdrawal: ingress_proto::GetPublicWithdrawalResponse {
                    found: true,
                    withdrawal: Some(record.clone()),
                    resolution: ingress_proto::PublicWithdrawalResolution::Found as i32,
                    support_hint: String::new(),
                    revision: 4,
                },
                history: ingress_proto::ListPublicWithdrawalsByBurnerResponse {
                    withdrawals: vec![record],
                    next_page_token: String::new(),
                    snapshot_revision: 4,
                },
                quote: ingress_proto::GetPublicWithdrawalQuoteResponse {
                    available: true,
                    gross_amount_nicks: "6553600000".to_owned(),
                    bridge_fee_nicks: "19500000".to_owned(),
                    transaction_fee_nicks: "1000".to_owned(),
                    net_payout_nicks: "6534099000".to_owned(),
                    snapshot_height: Some(120),
                    snapshot_block_id: Some(block_id.to_be_limb_bytes().to_vec()),
                    reserved_input_count: 1,
                    observed_at_unix_ms: now,
                    revision: 4,
                    reason: String::new(),
                },
            },
            event_id,
            account,
            transaction_id,
            block_id,
            _preserved_root: preserved_root,
        })
    }

    fn source(&self) -> Arc<dyn BrowserStatusSource> {
        Arc::new(self.source.clone())
    }
}

fn unix_ms() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_millis();
    i64::try_from(millis).expect("test timestamp fits i64")
}
