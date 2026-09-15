use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use alloy::primitives::{keccak256, Address, Bytes, U256};
use anyhow::{anyhow, Context, Result};
use bridge::shared::e2e_environment::BaseSepoliaE2eManifest;
use bridge::shared::types::WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK;
use bridge_dev::anvil::{AnvilBackend, AnvilConfig};
use bridge_dev::environment::BaseE2eEnvironment;
use bridge_dev::fork_preflight::{ForkPreflight, PristineDeploymentFacts};
use bridge_dev::fork_seeder::{
    ForkBalanceSeedError, ForkBalanceSeedRequest, ForkBalanceSeeder, ForkContractState,
};
use bridge_dev::fork_state::ForkState;
use serde::Deserialize;
use serde_json::Value;
use tokio::process::Command;

const MANIFEST_JSON: &str = include_str!("../../bridge/e2e/environments/base-sepolia.json");

#[tokio::test]
async fn exact_mint_gas_funding_and_repeated_baseline_reset_use_real_anvil() -> Result<()> {
    let backend = AnvilBackend::start(AnvilConfig::empty(), &checked_environment()).await?;
    let checked_manifest = BaseSepoliaE2eManifest::from_json(MANIFEST_JSON)?;
    let deployment = deploy_fixture(&backend, &checked_manifest).await?;
    let environment = exact_environment(
        &backend,
        fixture_manifest(checked_manifest, &deployment).await?,
    )
    .await?;
    let pristine = ForkPreflight::verify(backend.http_url(), &environment).await?;
    let configured_state = state_from_facts(pristine.facts());
    let holder = address(0x901);
    let extra_gas_account = address(0x902);
    let required_nicks = 6_553_600_000u64;
    let headroom_nicks = 65_536u64;
    let gas_balance_wei = U256::from(1_000_000_000_000_000_000u64);
    let request = ForkBalanceSeedRequest {
        holder,
        required_nicks,
        headroom_nicks,
        gas_accounts: vec![extra_gas_account],
        gas_balance_wei,
    };
    let inbox: Address = configured_state.nock_inbox.parse()?;
    assert_eq!(backend.balance(inbox).await?, U256::ZERO);

    let report = ForkBalanceSeeder::seed(&backend, &configured_state, request.clone()).await?;
    let target_nicks = required_nicks + headroom_nicks;
    let target_base_units =
        U256::from(target_nicks) * U256::from(WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK);
    assert_eq!(report.mint.target_nicks, target_nicks);
    assert_eq!(report.mint.target_base_units, target_base_units);
    assert_eq!(report.mint.holder_after_base_units, target_base_units);
    assert_eq!(report.mint.minted_base_units, target_base_units);
    assert_eq!(
        report.mint.total_supply_after_base_units - report.mint.total_supply_before_base_units,
        target_base_units
    );
    assert!(report
        .mint
        .receipt
        .as_ref()
        .is_some_and(|receipt| receipt.success));
    assert_eq!(report.nock_inbox_before, configured_state.nock_inbox);
    assert_eq!(report.nock_inbox_after, configured_state.nock_inbox);
    assert_eq!(report.bridge_state_after, configured_state);
    assert!(report
        .gas_funding
        .iter()
        .all(|record| record.after_wei == gas_balance_wei));
    assert!(report
        .gas_funding
        .iter()
        .any(|record| record.address == inbox && record.before_wei == U256::ZERO));

    let tracked_gas_accounts = report
        .gas_funding
        .iter()
        .map(|record| record.address)
        .collect::<Vec<_>>();
    let mut state =
        ForkState::capture(&backend, &configured_state, holder, tracked_gas_accounts).await?;
    let baseline = state.baseline().clone();
    assert_eq!(baseline.holder_balance_base_units, target_base_units);
    assert_eq!(baseline.bridge_state, configured_state);

    mutate_scenario_state(&backend, &deployment, holder).await?;
    let first_reset = state.reset_to_baseline(&backend).await?;
    assert_eq!(first_reset, baseline);
    mutate_scenario_state(&backend, &deployment, holder).await?;
    let second_reset = state.reset_to_baseline(&backend).await?;
    assert_eq!(second_reset, baseline);

    let first_mining = state.mine_base_blocks(&backend, 2).await?;
    assert_eq!(first_mining.after_height, first_mining.before_height + 2);
    let reset_after_mining = state.reset_to_baseline(&backend).await?;
    assert_eq!(reset_after_mining, baseline);
    let second_mining = state.mine_base_blocks(&backend, 2).await?;
    assert_eq!(second_mining.before_height, first_mining.before_height);
    assert_eq!(second_mining.before_hash, first_mining.before_hash);
    assert_eq!(second_mining.after_height, first_mining.after_height);
    assert_eq!(
        backend.block_hash(second_mining.after_height).await?,
        second_mining.after_hash
    );
    state.reset_to_baseline(&backend).await?;

    let block_before_invalid = backend.block_number().await?;
    let invalid = ForkBalanceSeedRequest {
        required_nicks: 0,
        ..request.clone()
    };
    assert!(matches!(
        ForkBalanceSeeder::seed(&backend, &configured_state, invalid).await,
        Err(ForkBalanceSeedError::InvalidRequest(_))
    ));
    let overflow = ForkBalanceSeedRequest {
        required_nicks: u64::MAX,
        headroom_nicks: 1,
        ..request.clone()
    };
    assert!(matches!(
        ForkBalanceSeeder::seed(&backend, &configured_state, overflow).await,
        Err(ForkBalanceSeedError::InvalidRequest(_))
    ));
    let below_existing = ForkBalanceSeedRequest {
        required_nicks: 1,
        headroom_nicks: 0,
        ..request
    };
    assert!(matches!(
        ForkBalanceSeeder::seed(&backend, &configured_state, below_existing).await,
        Err(ForkBalanceSeedError::HolderAboveTarget { .. })
    ));
    assert_eq!(backend.block_number().await?, block_before_invalid);

    let serialized = serde_json::to_string(&report)?;
    assert!(!serialized.contains("snapshot"));
    assert!(!serialized.contains("rpc_url"));
    backend.shutdown().await?;
    Ok(())
}

async fn mutate_scenario_state(
    backend: &AnvilBackend,
    deployment: &FixtureDeployment,
    holder: Address,
) -> Result<()> {
    backend.mine(3).await?;
    backend.set_balance(holder, U256::from(7u64)).await?;
    set_withdrawal_gate(backend, deployment, false).await?;
    Ok(())
}

async fn exact_environment(
    backend: &AnvilBackend,
    provisional_manifest: BaseSepoliaE2eManifest,
) -> Result<BaseE2eEnvironment> {
    let provisional = BaseE2eEnvironment::from_manifest(provisional_manifest)?;
    let observed = ForkPreflight::observe(backend.http_url(), &provisional).await?;
    let exact_manifest = manifest_from_facts(provisional.manifest().clone(), &observed)?;
    Ok(BaseE2eEnvironment::from_manifest(exact_manifest)?)
}

async fn fixture_manifest(
    mut manifest: BaseSepoliaE2eManifest,
    deployment: &FixtureDeployment,
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

fn state_from_facts(facts: &PristineDeploymentFacts) -> ForkContractState {
    ForkContractState {
        owner: facts.message_inbox_owner.clone(),
        nock_owner: facts.nock_owner.clone(),
        bridge_nodes: facts.bridge_nodes.clone(),
        threshold: facts.threshold,
        withdrawals_enabled: facts.withdrawals_enabled,
        message_inbox_nock: facts.reciprocal_pairing.message_inbox_nock.clone(),
        nock_inbox: facts.reciprocal_pairing.nock_inbox.clone(),
    }
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
        "fork-balance-{}-{}.json",
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
        .env("DEPLOY_TARGET_NETWORK", "bridge-e2e-balance-fixture")
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
