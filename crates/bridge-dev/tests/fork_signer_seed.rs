use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use alloy::primitives::{keccak256, Address, Bytes, U256};
use anyhow::{anyhow, Context, Result};
use bridge::shared::e2e_environment::BaseSepoliaE2eManifest;
use bridge_dev::anvil::{AnvilBackend, AnvilConfig};
use bridge_dev::environment::BaseE2eEnvironment;
use bridge_dev::fork_preflight::{ForkPreflight, PristineDeploymentFacts, VerifiedPristineFork};
use bridge_dev::fork_seeder::{ForkSeedError, ForkSeeder, OverrideKind};
use serde::Deserialize;
use serde_json::Value;
use tokio::process::Command;

const MANIFEST_JSON: &str = include_str!("../../bridge/e2e/environments/base-sepolia.json");

#[tokio::test]
async fn real_contract_seeding_records_overrides_and_rolls_back_partial_failure() -> Result<()> {
    let backend = AnvilBackend::start(AnvilConfig::empty(), &checked_environment()).await?;
    let checked_manifest = BaseSepoliaE2eManifest::from_json(MANIFEST_JSON)?;
    let deployment = deploy_fixture(&backend, &checked_manifest).await?;
    set_withdrawal_gate(&backend, &deployment, true).await?;
    let (environment, pristine) = verified_environment(
        &backend,
        fixture_manifest(checked_manifest, &deployment, true).await?,
    )
    .await?;
    let current_nodes = pristine
        .facts()
        .bridge_nodes
        .clone()
        .map(|node| node.parse::<Address>())
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| anyhow!("expected five current nodes"))?;

    let block_before_noop = backend.block_number().await?;
    let noop = ForkSeeder::seed(&backend, &pristine, current_nodes).await?;
    assert!(noop.overrides.is_empty());
    assert_eq!(noop.before, noop.after);
    assert_eq!(backend.block_number().await?, block_before_noop);

    set_withdrawal_gate(&backend, &deployment, false).await?;
    let (disabled_environment, disabled_pristine) = verified_environment(
        &backend,
        fixture_manifest(environment.manifest().clone(), &deployment, false).await?,
    )
    .await?;
    let deterministic =
        [address(0x101), address(0x102), address(0x103), address(0x104), address(0x105)];
    let report = ForkSeeder::seed(&backend, &disabled_pristine, deterministic).await?;
    assert_eq!(report.overrides.len(), 6);
    assert!(!report.before.withdrawals_enabled);
    assert!(report.after.withdrawals_enabled);
    assert_eq!(
        report.after.bridge_nodes,
        deterministic.map(|signer| format!("{signer:#x}"))
    );
    assert_eq!(report.before.owner, report.after.owner);
    assert_eq!(report.before.nock_owner, report.after.nock_owner);
    assert_eq!(
        report.before.message_inbox_nock,
        report.after.message_inbox_nock
    );
    assert_eq!(report.before.nock_inbox, report.after.nock_inbox);
    assert!(report.overrides.iter().all(|record| record.receipt.success));
    assert_eq!(
        report
            .overrides
            .iter()
            .filter(|record| matches!(record.kind, OverrideKind::BridgeNode { .. }))
            .count(),
        5
    );
    assert!(report
        .overrides
        .iter()
        .any(|record| matches!(record.kind, OverrideKind::WithdrawalsEnabled)));

    let (seeded_environment, seeded_pristine) = verified_environment(
        &backend,
        manifest_from_seed_report(
            &backend,
            disabled_environment.manifest().clone(),
            &report.after,
        )
        .await?,
    )
    .await?;
    let duplicate = [
        deterministic[0], deterministic[0], deterministic[2], deterministic[3], deterministic[4],
    ];
    let block_before_duplicate = backend.block_number().await?;
    let duplicate_error = ForkSeeder::seed(&backend, &seeded_pristine, duplicate)
        .await
        .expect_err("duplicate signer set must fail");
    assert!(matches!(
        duplicate_error,
        ForkSeedError::InvalidSignerSet(_)
    ));
    assert_eq!(backend.block_number().await?, block_before_duplicate);

    let partial_target = [
        address(0x201),
        deterministic[2],
        deterministic[0],
        deterministic[3],
        deterministic[4],
    ];
    let nonce_epoch_before = backend.nonce_epoch();
    let partial_error = ForkSeeder::seed(&backend, &seeded_pristine, partial_target)
        .await
        .expect_err("transient duplicate must fail midway");
    assert!(matches!(
        partial_error,
        ForkSeedError::RolledBack { reverted: true, .. }
    ));
    assert_eq!(backend.nonce_epoch(), nonce_epoch_before + 1);
    let verified_after_rollback =
        ForkPreflight::verify(backend.http_url(), &seeded_environment).await?;
    assert_eq!(
        verified_after_rollback.facts().bridge_nodes,
        seeded_pristine.facts().bridge_nodes
    );
    assert_eq!(
        verified_after_rollback.facts().message_inbox_owner,
        seeded_pristine.facts().message_inbox_owner
    );
    assert_eq!(
        verified_after_rollback.facts().reciprocal_pairing,
        seeded_pristine.facts().reciprocal_pairing
    );

    let serialized = serde_json::to_string(&report)?;
    assert!(!serialized.contains("rpc_url"));
    assert!(!serialized.contains("private_key"));
    backend.shutdown().await?;
    Ok(())
}

async fn verified_environment(
    backend: &AnvilBackend,
    provisional_manifest: BaseSepoliaE2eManifest,
) -> Result<(BaseE2eEnvironment, VerifiedPristineFork)> {
    let provisional = BaseE2eEnvironment::from_manifest(provisional_manifest)?;
    let observed = ForkPreflight::observe(backend.http_url(), &provisional).await?;
    let exact_manifest = manifest_from_facts(provisional.manifest().clone(), &observed)?;
    let environment = BaseE2eEnvironment::from_manifest(exact_manifest)?;
    let verified = ForkPreflight::verify(backend.http_url(), &environment).await?;
    Ok((environment, verified))
}

async fn fixture_manifest(
    mut manifest: BaseSepoliaE2eManifest,
    deployment: &FixtureDeployment,
    withdrawals_enabled: bool,
) -> Result<BaseSepoliaE2eManifest> {
    let block = read_latest_block(&deployment.endpoint).await?;
    manifest.source_chain.fork_block.number = block.number;
    manifest.source_chain.fork_block.hash = block.hash;
    manifest.source_chain.fork_block.explorer_url =
        format!("https://base-sepolia.blockscout.com/block/{}", block.number);
    manifest.contracts.message_inbox.proxy.address = deployment.message_inbox_proxy.clone();
    manifest.contracts.message_inbox.implementation.address =
        deployment.message_inbox_implementation.clone();
    manifest.contracts.nock.address = deployment.nock.clone();
    manifest
        .contracts
        .message_inbox
        .proxy
        .deployment
        .block_number = 1;
    manifest
        .contracts
        .message_inbox
        .implementation
        .deployment
        .block_number = 1;
    manifest.contracts.nock.deployment.block_number = 1;
    manifest.pristine_state.message_inbox_owner = deployment.deployer.clone();
    manifest.pristine_state.nock_owner = deployment.deployer.clone();
    manifest.pristine_state.withdrawals_enabled = withdrawals_enabled;
    manifest
        .pristine_state
        .reciprocal_pairing
        .message_inbox_nock = deployment.nock.clone();
    manifest.pristine_state.reciprocal_pairing.nock_inbox = deployment.message_inbox_proxy.clone();
    manifest.artifacts.erc1967_proxy.verification_url =
        verification_url(&deployment.message_inbox_proxy);
    manifest.artifacts.message_inbox.verification_url =
        verification_url(&deployment.message_inbox_implementation);
    manifest.artifacts.nock.verification_url = verification_url(&deployment.nock);
    manifest.validate()?;
    Ok(manifest)
}

fn manifest_from_facts(
    mut manifest: BaseSepoliaE2eManifest,
    facts: &PristineDeploymentFacts,
) -> Result<BaseSepoliaE2eManifest> {
    manifest.source_chain.fork_block.number = facts.source_block.number;
    manifest.source_chain.fork_block.hash = facts.source_block.hash.clone();
    manifest.source_chain.fork_block.explorer_url = format!(
        "https://base-sepolia.blockscout.com/block/{}",
        facts.source_block.number
    );
    manifest
        .contracts
        .message_inbox
        .proxy
        .runtime_code_keccak256 = facts.message_inbox_proxy.keccak256.clone();
    manifest
        .contracts
        .message_inbox
        .implementation
        .runtime_code_keccak256 = facts.message_inbox_implementation.keccak256.clone();
    manifest.contracts.nock.runtime_code_keccak256 = facts.nock.keccak256.clone();
    manifest.pristine_state.message_inbox_owner = facts.message_inbox_owner.clone();
    manifest.pristine_state.nock_owner = facts.nock_owner.clone();
    manifest.pristine_state.bridge_nodes = facts.bridge_nodes.clone();
    manifest.pristine_state.threshold = facts.threshold;
    manifest.pristine_state.withdrawals_enabled = facts.withdrawals_enabled;
    manifest.pristine_state.reciprocal_pairing = facts.reciprocal_pairing.clone();
    manifest.validate()?;
    Ok(manifest)
}

async fn manifest_from_seed_report(
    backend: &AnvilBackend,
    mut manifest: BaseSepoliaE2eManifest,
    state: &bridge_dev::fork_seeder::ForkContractState,
) -> Result<BaseSepoliaE2eManifest> {
    let block = read_latest_block(backend.http_url().as_url().as_str()).await?;
    manifest.source_chain.fork_block.number = block.number;
    manifest.source_chain.fork_block.hash = block.hash;
    manifest.source_chain.fork_block.explorer_url =
        format!("https://base-sepolia.blockscout.com/block/{}", block.number);
    manifest.pristine_state.message_inbox_owner = state.owner.clone();
    manifest.pristine_state.nock_owner = state.nock_owner.clone();
    manifest.pristine_state.bridge_nodes = state.bridge_nodes.clone();
    manifest.pristine_state.threshold = state.threshold;
    manifest.pristine_state.withdrawals_enabled = state.withdrawals_enabled;
    manifest
        .pristine_state
        .reciprocal_pairing
        .message_inbox_nock = state.message_inbox_nock.clone();
    manifest.pristine_state.reciprocal_pairing.nock_inbox = state.nock_inbox.clone();
    manifest.validate()?;
    Ok(manifest)
}

async fn set_withdrawal_gate(
    backend: &AnvilBackend,
    deployment: &FixtureDeployment,
    enabled: bool,
) -> Result<()> {
    let owner: Address = deployment.deployer.parse()?;
    let proxy: Address = deployment.message_inbox_proxy.parse()?;
    backend
        .set_balance(owner, U256::from(1_000_000_000_000_000_000u64))
        .await?;
    backend.impersonate(owner).await?;
    let mut data = keccak256("setWithdrawalsEnabled(bool)".as_bytes()).as_slice()[..4].to_vec();
    data.extend_from_slice(&word_u64(u64::from(enabled)));
    let hash = backend
        .backend()
        .send_transaction(owner, proxy, Bytes::from(data))
        .await?;
    let receipt = backend
        .backend()
        .wait_for_receipt(hash, Duration::from_secs(10))
        .await?;
    backend.stop_impersonating(owner).await?;
    if !receipt.success {
        return Err(anyhow!("gate fixture transaction reverted"));
    }
    Ok(())
}

async fn deploy_fixture(
    backend: &AnvilBackend,
    manifest: &BaseSepoliaE2eManifest,
) -> Result<FixtureDeployment> {
    let workspace = workspace_root()?;
    let contracts = workspace.join("crates/bridge/contracts");
    let deployment_dir = contracts.join("deployments");
    fs::create_dir_all(&deployment_dir)?;
    let deployment_path = deployment_dir.join(format!(
        "fork-seed-{}-{}.json",
        std::process::id(),
        backend.facts().port
    ));
    let endpoint = backend.http_url().as_url().to_string();
    let deployer = first_anvil_account(&endpoint).await?;
    let forge = std::env::var_os("FORGE_BIN").unwrap_or_else(|| "forge".into());
    let mut command = Command::new(forge);
    command
        .current_dir(&contracts)
        .args([
            "script", "forge/Deploy.s.sol:Deploy", "--rpc-url", &endpoint, "--broadcast",
            "--unlocked", "--sender", &deployer, "--non-interactive", "--quiet",
        ])
        .env("DEPLOYMENTS_PATH", &deployment_path)
        .env("DEPLOY_TARGET_NETWORK", "bridge-e2e-seed-fixture")
        .env("DEPLOYER_ADDRESS", &deployer)
        .env("NOCK_NAME", "Nock")
        .env("NOCK_SYMBOL", "NOCK")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (index, node) in manifest.pristine_state.bridge_nodes.iter().enumerate() {
        command.env(format!("BRIDGE_NODE_{index}"), node);
    }
    let output = command.output().await?;
    if !output.status.success() {
        return Err(anyhow!(
            "fixture deployment failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let deployed: DeploymentFile = serde_json::from_str(&fs::read_to_string(deployment_path)?)?;
    Ok(FixtureDeployment {
        message_inbox_implementation: deployed.message_inbox_implementation.to_ascii_lowercase(),
        message_inbox_proxy: deployed.message_inbox_proxy.to_ascii_lowercase(),
        nock: deployed.nock.to_ascii_lowercase(),
        deployer,
        endpoint,
    })
}

async fn first_anvil_account(endpoint: &str) -> Result<String> {
    let cast = std::env::var_os("CAST_BIN").unwrap_or_else(|| "cast".into());
    let output = Command::new(cast)
        .args(["rpc", "--rpc-url", endpoint, "eth_accounts"])
        .output()
        .await?;
    if !output.status.success() {
        return Err(anyhow!("cast eth_accounts failed"));
    }
    let accounts: Vec<String> = serde_json::from_slice(&output.stdout)?;
    accounts
        .first()
        .map(|account| account.to_ascii_lowercase())
        .context("Anvil returned no accounts")
}

async fn read_latest_block(endpoint: &str) -> Result<LatestBlock> {
    let cast = std::env::var_os("CAST_BIN").unwrap_or_else(|| "cast".into());
    let output = Command::new(cast)
        .args(["block", "latest", "--rpc-url", endpoint, "--json"])
        .output()
        .await?;
    if !output.status.success() {
        return Err(anyhow!("cast block latest failed"));
    }
    let value: Value = serde_json::from_slice(&output.stdout)?;
    let number = value["number"]
        .as_str()
        .context("missing latest block number")?;
    let hash = value["hash"]
        .as_str()
        .context("missing latest block hash")?;
    Ok(LatestBlock {
        number: u64::from_str_radix(number.trim_start_matches("0x"), 16)?,
        hash: hash.to_ascii_lowercase(),
    })
}

fn verification_url(address: &str) -> String {
    format!("https://base-sepolia.blockscout.com/api/v2/smart-contracts/{address}")
}

fn word_u64(value: u64) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[24..].copy_from_slice(&value.to_be_bytes());
    word
}

fn address(value: u64) -> Address {
    let mut bytes = [0u8; 20];
    bytes[12..].copy_from_slice(&value.to_be_bytes());
    Address::from(bytes)
}

fn workspace_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("bridge-dev is not under workspace crates directory")
}

fn checked_environment() -> BaseE2eEnvironment {
    BaseE2eEnvironment::from_json(MANIFEST_JSON)
        .expect("checked-in Base Sepolia environment must validate")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeploymentFile {
    message_inbox_implementation: String,
    message_inbox_proxy: String,
    nock: String,
}

struct FixtureDeployment {
    message_inbox_implementation: String,
    message_inbox_proxy: String,
    nock: String,
    deployer: String,
    endpoint: String,
}

struct LatestBlock {
    number: u64,
    hash: String,
}
