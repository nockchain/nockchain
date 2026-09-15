use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use alloy::primitives::{Address, U256};
use anyhow::{anyhow, Result};
use bridge::shared::e2e_environment::BASE_SEPOLIA_E2E_CHAIN_ID;
use bridge_dev::anvil::{AnvilBackend, AnvilConfig, AnvilMode, AnvilStartError};
use bridge_dev::environment::BaseE2eEnvironment;
use tempfile::TempDir;

const MANIFEST_JSON: &str = include_str!("../../bridge/e2e/environments/base-sepolia.json");

fn environment() -> BaseE2eEnvironment {
    BaseE2eEnvironment::from_json(MANIFEST_JSON)
        .expect("checked-in Base Sepolia environment must validate")
}

#[tokio::test]
async fn empty_anvil_starts_exposes_facts_and_shuts_down() -> Result<()> {
    let anvil = AnvilBackend::start(AnvilConfig::empty(), &environment()).await?;
    let facts = anvil.facts().clone();

    assert_eq!(facts.chain_id, BASE_SEPOLIA_E2E_CHAIN_ID);
    assert_eq!(facts.mode, "empty");
    assert_eq!(facts.fork_block_number, None);
    assert!(facts.binary_version.starts_with("anvil Version: "));
    assert!(facts.rpc_client_version.starts_with("anvil/"));
    assert!(facts.snapshot_round_trip);
    assert_eq!(anvil.http_url().as_url().host_str(), Some("127.0.0.1"));
    assert_eq!(anvil.ws_url(), format!("ws://127.0.0.1:{}", facts.port));
    assert_eq!(anvil.block_number().await?, 0);
    let _ = anvil.block_hash(0).await?;

    anvil.shutdown().await?;
    let rebound = TcpListener::bind(("127.0.0.1", facts.port));
    assert!(rebound.is_ok(), "Anvil port remained bound after shutdown");
    Ok(())
}

#[tokio::test]
async fn snapshot_revert_invalidates_nonce_epoch_and_mining_stays_deterministic() -> Result<()> {
    let anvil = AnvilBackend::start(AnvilConfig::empty(), &environment()).await?;
    let initial = anvil.block_number().await?;
    let snapshot = anvil.snapshot().await?;

    anvil.mine(2).await?;
    assert_eq!(anvil.block_number().await?, initial + 2);
    assert_eq!(anvil.nonce_epoch(), 0);
    assert!(anvil.revert(&snapshot).await?);
    assert_eq!(anvil.block_number().await?, initial);
    assert_eq!(anvil.nonce_epoch(), 1);
    assert!(!anvil.revert(&snapshot).await?);
    assert_eq!(anvil.nonce_epoch(), 1);
    anvil.mine(1).await?;
    assert_eq!(anvil.block_number().await?, initial + 1);
    assert!(anvil.mine(0).await.is_err());

    anvil.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn balance_and_impersonation_controls_hit_real_anvil() -> Result<()> {
    let anvil = AnvilBackend::start(AnvilConfig::empty(), &environment()).await?;
    let account: Address = "0x1000000000000000000000000000000000000001".parse()?;
    let balance = U256::from(123_456_789u64);

    anvil.set_balance(account, balance).await?;
    assert_eq!(anvil.balance(account).await?, balance);
    anvil.impersonate(account).await?;
    anvil.stop_impersonating(account).await?;

    anvil.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn concurrent_anvil_instances_use_distinct_ports() -> Result<()> {
    let first_environment = environment();
    let second_environment = environment();
    let (first, second) = tokio::join!(
        AnvilBackend::start(AnvilConfig::empty(), &first_environment),
        AnvilBackend::start(AnvilConfig::empty(), &second_environment)
    );
    let first = first?;
    let second = second?;
    assert_ne!(first.facts().port, second.facts().port);
    let (first_shutdown, second_shutdown) = tokio::join!(first.shutdown(), second.shutdown());
    first_shutdown?;
    second_shutdown?;
    Ok(())
}

#[tokio::test]
async fn collision_wrong_chain_and_missing_binary_fail_before_backend_exposure() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let occupied_port = listener.local_addr()?.port();
    let mut collision = AnvilConfig::empty();
    collision.port = Some(occupied_port);
    assert!(matches!(
        AnvilBackend::start(collision, &environment()).await,
        Err(AnvilStartError::PortUnavailable { port }) if port == occupied_port
    ));

    let mut wrong_chain = AnvilConfig::empty();
    wrong_chain.chain_id = 84_532;
    assert!(matches!(
        AnvilBackend::start(wrong_chain, &environment()).await,
        Err(AnvilStartError::InvalidChainId {
            expected: BASE_SEPOLIA_E2E_CHAIN_ID,
            observed: 84_532
        })
    ));

    let mut missing = AnvilConfig::empty();
    missing.binary = Path::new("/definitely/missing/anvil").to_path_buf();
    assert!(matches!(
        AnvilBackend::start(missing, &environment()).await,
        Err(AnvilStartError::VersionProbe(_))
    ));
    Ok(())
}

#[tokio::test]
async fn early_exit_wrong_version_and_direct_fork_are_diagnostic_and_redacted() -> Result<()> {
    let tempdir = TempDir::new()?;
    let early_exit = write_executable(
        tempdir.path().join("early-anvil"),
        br##"#!/bin/sh
if [ "${1:-}" = "--version" ]; then
  printf 'anvil Version: fixture\n'
  exit 0
fi
printf 'fixture exited before listen\n' >&2
exit 7
"##,
    )?;
    let mut early_config = AnvilConfig::empty();
    early_config.binary = early_exit;
    let error = AnvilBackend::start(early_config, &environment())
        .await
        .err()
        .ok_or_else(|| anyhow!("early exit unexpectedly became ready"))?;
    assert!(matches!(error, AnvilStartError::ExitedBeforeReady { .. }));
    assert!(error.to_string().contains("fixture exited before listen"));

    let wrong_version = write_executable(
        tempdir.path().join("wrong-version"),
        b"#!/bin/sh\nprintf 'not-anvil\\n'\n",
    )?;
    let mut version_config = AnvilConfig::empty();
    version_config.binary = wrong_version;
    assert!(matches!(
        AnvilBackend::start(version_config, &environment()).await,
        Err(AnvilStartError::InvalidVersion(_))
    ));

    let source_rpc_url = format!(
        "http://127.0.0.1:9/{}",
        ["sensitive", "source", "value"].join("-")
    );
    let fork_config = AnvilConfig::fork(source_rpc_url.clone(), 42);
    let error = AnvilBackend::start(fork_config, &environment())
        .await
        .err()
        .ok_or_else(|| anyhow!("direct fork unexpectedly became ready"))?;
    let rendered = error.to_string();
    assert!(matches!(
        error,
        AnvilStartError::ForkRequiresPinnedPreflight
    ));
    assert!(!rendered.contains(&source_rpc_url));

    let debug = format!(
        "{:?}",
        AnvilMode::Fork {
            source_rpc_url: source_rpc_url.clone(),
            block_number: 42,
        }
    );
    assert!(!debug.contains(&source_rpc_url));
    Ok(())
}

fn write_executable(path: impl AsRef<Path>, contents: &[u8]) -> Result<std::path::PathBuf> {
    let path = path.as_ref();
    fs::write(path, contents)?;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(path.to_path_buf())
}
