use std::fs;
use std::path::{Path, PathBuf};

use alloy::primitives::{Address, U256};
use anyhow::{anyhow, Context, Result};
use bridge::shared::e2e_environment::BaseSepoliaE2eManifest;
use bridge_dev::anvil::{AnvilBackend, AnvilConfig};
use bridge_dev::environment::BaseE2eEnvironment;
use bridge_dev::hermetic_deploy::{
    HermeticDeployConfig, HermeticDeployError, HermeticDeployment, HERMETIC_ENVIRONMENT_ID,
};
use serde_json::Value;
use tempfile::TempDir;

const MANIFEST_JSON: &str = include_str!("../../bridge/e2e/environments/base-sepolia.json");

#[tokio::test]
async fn deploys_current_contracts_and_resets_exact_facts() -> Result<()> {
    let backend = AnvilBackend::start(AnvilConfig::empty(), &checked_environment()).await?;
    let workspace = workspace_root()?;
    let signers = deterministic_signers()?;
    let config = HermeticDeployConfig::discover(&workspace, signers);
    let mut deployment = HermeticDeployment::deploy(&backend, config).await?;
    let facts = deployment.facts().clone();

    assert_eq!(facts.environment_id, HERMETIC_ENVIRONMENT_ID);
    assert_eq!(
        facts.bridge_state.bridge_nodes,
        signers.map(|address| format!("{address:#x}"))
    );
    assert_eq!(
        facts.bridge_state.owner,
        format!("{:#x}", facts.addresses.deployer)
    );
    assert_eq!(
        facts.bridge_state.nock_owner,
        format!("{:#x}", facts.addresses.deployer)
    );
    assert_eq!(facts.bridge_state.threshold, 3);
    assert!(facts.bridge_state.withdrawals_enabled);
    assert_eq!(facts.receipts.len(), 5);
    assert!(facts.receipts.iter().all(|record| record.receipt.success));
    assert!(facts
        .receipts
        .iter()
        .any(|record| record.role == "withdrawal_gate_enabled"));
    assert!(facts
        .runtime_artifacts
        .iter()
        .all(|artifact| artifact.runtime_matches_artifact));
    assert!(facts
        .runtime_artifacts
        .iter()
        .any(|artifact| artifact.contract_name == "MessageInbox"
            && artifact.immutable_reference_count > 0));

    let nonce_epoch = backend.nonce_epoch();
    backend.mine(3).await?;
    backend
        .set_balance(address(0x777), U256::from(42u64))
        .await?;
    let first_reset = deployment.reset(&backend).await?;
    assert_eq!(first_reset, facts);
    assert_eq!(backend.nonce_epoch(), nonce_epoch + 1);
    backend.mine(1).await?;
    let second_reset = deployment.reset(&backend).await?;
    assert_eq!(second_reset, facts);
    assert_eq!(backend.nonce_epoch(), nonce_epoch + 2);

    let serialized = serde_json::to_string(&facts)?;
    assert!(serialized.contains(HERMETIC_ENVIRONMENT_ID));
    assert!(!serialized.contains("base-sepolia-fork"));
    assert!(!serialized.contains("rpc_url"));
    backend.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn invalid_signers_and_artifacts_fail_before_chain_mutation() -> Result<()> {
    let backend = AnvilBackend::start(AnvilConfig::empty(), &checked_environment()).await?;
    let workspace = workspace_root()?;
    let initial_block = backend.block_number().await?;

    let mut invalid_signers = deterministic_signers()?;
    invalid_signers[4] = Address::ZERO;
    let invalid_config = HermeticDeployConfig::discover(&workspace, invalid_signers);
    assert!(matches!(
        HermeticDeployment::deploy(&backend, invalid_config).await,
        Err(HermeticDeployError::InvalidSignerSet)
    ));
    assert_eq!(backend.block_number().await?, initial_block);

    let mut duplicate_signers = deterministic_signers()?;
    duplicate_signers[4] = duplicate_signers[0];
    let duplicate_config = HermeticDeployConfig::discover(&workspace, duplicate_signers);
    assert!(matches!(
        HermeticDeployment::deploy(&backend, duplicate_config).await,
        Err(HermeticDeployError::InvalidSignerSet)
    ));
    assert_eq!(backend.block_number().await?, initial_block);

    let tempdir = TempDir::new()?;
    let mut missing = HermeticDeployConfig::discover(&workspace, deterministic_signers()?);
    missing.nock_artifact = tempdir.path().join("missing-nock.json");
    assert!(matches!(
        HermeticDeployment::deploy(&backend, missing).await,
        Err(HermeticDeployError::ArtifactRead { .. })
    ));
    assert_eq!(backend.block_number().await?, initial_block);

    let original_nock = workspace.join("crates/bridge/contracts/out/Nock.sol/Nock.json");
    let mut nock_json: Value = serde_json::from_str(&fs::read_to_string(&original_nock)?)?;
    nock_json["bytecode"]["object"] = Value::String("0x".to_owned());
    let empty_bytecode = tempdir.path().join("empty-bytecode.json");
    fs::write(&empty_bytecode, serde_json::to_vec(&nock_json)?)?;
    let mut empty_config = HermeticDeployConfig::discover(&workspace, deterministic_signers()?);
    empty_config.nock_artifact = empty_bytecode;
    assert!(matches!(
        HermeticDeployment::deploy(&backend, empty_config).await,
        Err(HermeticDeployError::InvalidArtifact(_))
    ));
    assert_eq!(backend.block_number().await?, initial_block);

    let abi = nock_json["abi"]
        .as_array_mut()
        .context("Nock ABI is not an array")?;
    abi.retain(|entry| entry.get("name").and_then(Value::as_str) != Some("mint"));
    nock_json["bytecode"]["object"] =
        serde_json::from_str::<Value>(&fs::read_to_string(&original_nock)?)?["bytecode"]["object"]
            .clone();
    let wrong_abi = tempdir.path().join("wrong-abi.json");
    fs::write(&wrong_abi, serde_json::to_vec(&nock_json)?)?;
    let mut abi_config = HermeticDeployConfig::discover(&workspace, deterministic_signers()?);
    abi_config.nock_artifact = wrong_abi;
    assert!(matches!(
        HermeticDeployment::deploy(&backend, abi_config).await,
        Err(HermeticDeployError::InvalidArtifact(_))
    ));
    assert_eq!(backend.block_number().await?, initial_block);

    backend.shutdown().await?;
    Ok(())
}

fn deterministic_signers() -> Result<[Address; 5]> {
    let manifest = BaseSepoliaE2eManifest::from_json(MANIFEST_JSON)?;
    manifest
        .pristine_state
        .bridge_nodes
        .map(|address| address.parse::<Address>())
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| anyhow!("expected five deterministic signers"))
}

fn checked_environment() -> BaseE2eEnvironment {
    BaseE2eEnvironment::from_json(MANIFEST_JSON)
        .expect("checked-in Base Sepolia environment must validate")
}

fn workspace_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("bridge-dev is not under workspace crates directory")
}

fn address(value: u64) -> Address {
    let mut bytes = [0u8; 20];
    bytes[12..].copy_from_slice(&value.to_be_bytes());
    Address::from(bytes)
}
