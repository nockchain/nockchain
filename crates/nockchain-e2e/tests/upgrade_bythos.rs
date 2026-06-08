use std::collections::{HashMap, HashSet};
use std::env;
use std::net::{TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use nockapp::noun::slab::NounSlab;
use nockchain_e2e::grpc::{
    set_mining_enabled, set_mining_pkh_live, transaction_accepted, wait_for_tx_in_block,
};
use nockchain_e2e::runner;
use nockchain_e2e::runner::RunOptions;
use nockchain_e2e::upgrade::{NockchainCluster, UpgradeTestConfig, WalletClient};
use nockchain_math::structs::HoonMapIter;
use nockchain_types::tx_engine::common::{Hash, Name, Signature};
use nockchain_types::tx_engine::v1::note::{NoteData, NoteDataEntry, NoteDataValue};
use nockchain_types::tx_engine::v1::{LockMerkleProof, Spend, Spends, Witness};
use nockvm::noun::{NounAllocator, NounHandle};
use noun_serde::{NounDecode, NounEncode};
use regex::Regex;
use tokio::sync::Mutex;
use tokio::time::{sleep, Instant};

static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[tokio::test]
async fn test_bythos_activation_gating() -> Result<()> {
    let _guard = TEST_LOCK.get_or_init(|| Mutex::new(())).lock().await;

    let Some(nockchain_bin) = resolve_nockchain_bin() else {
        eprintln!("skipping bythos gating test: nockchain binary not available");
        return Ok(());
    };
    let Some(wallet_bin) = resolve_wallet_bin() else {
        eprintln!("skipping bythos gating test: wallet binary not available");
        return Ok(());
    };

    let run_dir = temp_run_dir("bythos-gating");
    std::fs::create_dir_all(&run_dir).context("create bythos run dir")?;

    let activation_height = 35;
    let (base_grpc_port, base_private_port) =
        reserve_distinct_tcp_pairs().context("reserve bythos gRPC ports")?;
    let base_p2p_port = reserve_udp_port_pair().context("reserve bythos p2p ports")?;
    let miner_pkh = initialize_wallet_for_mining(
        &wallet_bin,
        &run_dir.join("wallets").join("miner"),
        base_private_port,
    )
    .await?;
    let mut config = UpgradeTestConfig::new(activation_height, nockchain_bin, run_dir.clone());
    config.base_grpc_port = base_grpc_port;
    config.base_private_grpc_port = base_private_port;
    config.base_p2p_port = base_p2p_port;
    config.mining_pkh = Some(miner_pkh);
    config.update_candidate_interval_secs = Some(1);

    let mut cluster = NockchainCluster::with_activation_height(config).await?;

    let result = run_bythos_gating_steps(&mut cluster, activation_height, wallet_bin).await;

    let shutdown = cluster.shutdown().await;
    match (result, shutdown) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(err), _) => Err(err),
        (Ok(()), Err(err)) => Err(err),
    }
}

#[tokio::test]
async fn test_upgrade_activation_scenario() -> Result<()> {
    let _guard = TEST_LOCK.get_or_init(|| Mutex::new(())).lock().await;

    let old_bin = env::var("NOCKCHAIN_BIN_OLD").ok();
    let new_bin = env::var("NOCKCHAIN_BIN_NEW").ok();
    let Some(wallet_bin) = resolve_wallet_bin() else {
        eprintln!("skipping upgrade scenario: wallet binary not available");
        return Ok(());
    };

    if old_bin.is_none() || new_bin.is_none() {
        eprintln!("skipping upgrade scenario: NOCKCHAIN_BIN_OLD/NEW not set");
        return Ok(());
    }
    if let (Some(old_bin), Some(new_bin)) = (&old_bin, &new_bin) {
        let old_path = PathBuf::from(old_bin);
        if !binary_supports_grpc_flags(&old_path) {
            eprintln!("old binary lacks gRPC flags; using new binary for upgrade scenario",);
            env::set_var("NOCKCHAIN_BIN_OLD", new_bin);
        }
    }

    let (base_grpc_port, base_private_port) =
        reserve_distinct_tcp_pairs().context("reserve upgrade gRPC ports")?;
    let base_p2p_port = reserve_udp_port_pair().context("reserve upgrade p2p ports")?;
    env::set_var("BASE_P2P_PORT", base_p2p_port.to_string());

    let scenario_path = workspace_root()?.join("tests/e2e/scenarios/upgrade_activation.yaml");
    let run_dir = temp_run_dir("bythos-upgrade");
    std::fs::create_dir_all(&run_dir).context("create upgrade run dir")?;

    let options = RunOptions {
        scenario_path,
        nockchain_bin: None,
        wallet_bin: Some(wallet_bin),
        work_dir: run_dir,
        base_grpc_port,
        base_private_grpc_port: base_private_port,
        base_p2p_port,
        docker: false,
        docker_image: None,
        keep_artifacts: false,
    };

    runner::run_scenario(options).await
}

async fn run_bythos_gating_steps(
    cluster: &mut NockchainCluster,
    activation_height: u64,
    wallet_bin: PathBuf,
) -> Result<()> {
    cluster.start().await?;
    cluster.wait_for_grpc(Duration::from_secs(30)).await?;
    let constants =
        wait_for_bythos_phase(cluster, activation_height, Duration::from_secs(10)).await?;
    if constants.bythos_phase != activation_height {
        return Err(anyhow!(
            "bythos-phase mismatch: expected {}, got {} (v1-phase {})", activation_height,
            constants.bythos_phase, constants.v1_phase
        ));
    }

    let public_addr = cluster.grpc_public_addr()?.to_string();
    let private_addr = cluster.grpc_private_addr()?.to_string();
    set_mining_enabled(&private_addr, false).await?;

    let miner_wallet = cluster.wallet("miner", wallet_bin.clone())?;
    let recipient_wallet = cluster.wallet("recipient", wallet_bin)?;

    let miner_list = miner_wallet
        .run("list-active-addresses", &Vec::new())
        .await?;
    let miner_combined = format!("{}\n{}", miner_list.stdout, miner_list.stderr);
    let miner_pkh = extract_address(&miner_combined)?;
    recipient_wallet.run("keygen", &Vec::new()).await?;
    let recipient_list = recipient_wallet
        .run("list-active-addresses", &Vec::new())
        .await?;
    let recipient_combined = format!("{}\n{}", recipient_list.stdout, recipient_list.stderr);
    let recipient_pkh = extract_address(&recipient_combined)?;

    let pre_height = activation_height.saturating_sub(15).max(3);
    if pre_height >= activation_height {
        return Err(anyhow!(
            "pre-activation mining target {} reaches activation {}", pre_height, activation_height
        ));
    }
    set_mining_pkh_live(&private_addr, &miner_pkh).await?;
    set_mining_enabled(&private_addr, true).await?;
    cluster.wait_for_grpc(Duration::from_secs(30)).await?;
    cluster
        .mine_to_height(pre_height, Duration::from_secs(180))
        .await?;
    set_mining_enabled(&private_addr, false).await?;
    cluster.wait_for_grpc(Duration::from_secs(30)).await?;

    let notes = list_notes_with_retry(&miner_wallet, 5, Duration::from_millis(500)).await?;
    let current_height = cluster.current_height().await?;
    if current_height >= activation_height {
        return Err(anyhow!(
            "expected pre-activation height below {}, got {}", activation_height, current_height
        ));
    }
    let mut pre_notes: Vec<NoteInfo> = notes
        .into_iter()
        .filter(|note| note.height < activation_height)
        .filter(|note| {
            note.height.saturating_add(constants.coinbase_timelock_min) <= current_height
        })
        .collect();
    pre_notes.sort_by_key(|note| note.height);
    if pre_notes.len() < 3 {
        let heights: Vec<u64> = pre_notes.iter().map(|note| note.height).collect();
        return Err(anyhow!(
            "expected at least 3 spendable pre-activation notes, found {} (heights: {:?}, timelock-min {})",
            pre_notes.len(),
            heights,
            constants.coinbase_timelock_min
        ));
    }
    let note_pre_accept = pre_notes
        .pop()
        .expect("pre_notes length was checked before selecting accept note");
    let note_pre_reject = pre_notes
        .pop()
        .expect("pre_notes length was checked before selecting reject note");
    let note_post_stub = pre_notes
        .pop()
        .expect("pre_notes length was checked before selecting stub note");

    let (pre_accept_min, _) = compute_fee_bounds_for_note(
        &miner_wallet, &note_pre_accept, &recipient_pkh, &miner_pkh, constants.base_fee,
        constants.input_fee_divisor, constants.min_fee,
    )
    .await?;
    let accept_fee = pre_accept_min.saturating_add(1);

    let (pre_reject_min, post_reject_min) = compute_fee_bounds_for_note(
        &miner_wallet, &note_pre_reject, &recipient_pkh, &miner_pkh, constants.base_fee,
        constants.input_fee_divisor, constants.min_fee,
    )
    .await?;
    let reject_fee = choose_mid_fee(pre_reject_min, post_reject_min, "pre-activation reject")?;

    let (pre_post_stub_min, post_post_stub_min) = compute_fee_bounds_for_note(
        &miner_wallet, &note_post_stub, &recipient_pkh, &miner_pkh, constants.base_fee,
        constants.input_fee_divisor, constants.min_fee,
    )
    .await?;
    let post_stub_fee = choose_mid_fee(
        pre_post_stub_min, post_post_stub_min, "post-activation stub",
    )?;

    let accept_amount = choose_amount(note_pre_accept.assets, accept_fee)?;
    let reject_amount = choose_amount(note_pre_reject.assets, reject_fee)?;
    let post_amount = choose_amount(note_post_stub.assets, post_stub_fee)?;

    let tx_pre_accept = create_tx_artifact(
        &miner_wallet, &note_pre_accept.name, &recipient_pkh, note_pre_accept.assets,
        accept_amount, accept_fee, &miner_pkh,
    )
    .await?;
    log_tx_lmp("pre_accept", &tx_pre_accept.spends);
    let pre_accept_chain_height = cluster.current_height().await?;
    if pre_accept_chain_height >= activation_height {
        return Err(anyhow!(
            "pre-accept tx built at height {} (activation {})", pre_accept_chain_height,
            activation_height
        ));
    }
    let pre_accept_id =
        submit_expect_accepted(&miner_wallet, &public_addr, &tx_pre_accept.tx_path).await?;

    let tx_pre_reject = create_tx_artifact(
        &miner_wallet, &note_pre_reject.name, &recipient_pkh, note_pre_reject.assets,
        reject_amount, reject_fee, &miner_pkh,
    )
    .await?;
    log_tx_lmp("pre_reject", &tx_pre_reject.spends);
    let pre_reject_chain_height = cluster.current_height().await?;
    if pre_reject_chain_height >= activation_height {
        return Err(anyhow!(
            "pre-reject tx built at height {} (activation {})", pre_reject_chain_height,
            activation_height
        ));
    }
    let pre_reject_id =
        submit_expect_accepted(&miner_wallet, &public_addr, &tx_pre_reject.tx_path).await?;

    let after_submit_height = cluster.current_height().await?;
    let pre_mine_height = std::cmp::min(
        activation_height.saturating_sub(1),
        std::cmp::max(
            pre_height.saturating_add(1),
            after_submit_height.saturating_add(3),
        ),
    );
    if pre_mine_height >= activation_height {
        return Err(anyhow!(
            "pre-activation mining target {} reaches activation {}", pre_mine_height,
            activation_height
        ));
    }
    set_mining_enabled(&private_addr, true).await?;
    cluster.wait_for_grpc(Duration::from_secs(30)).await?;
    cluster
        .mine_to_height(pre_mine_height, Duration::from_secs(180))
        .await?;
    set_mining_enabled(&private_addr, false).await?;
    cluster.wait_for_grpc(Duration::from_secs(30)).await?;

    let pre_accept_in_block =
        wait_for_tx_in_block(&public_addr, &pre_accept_id, Duration::from_secs(30)).await?;
    if pre_accept_in_block.is_none() {
        return Err(anyhow!(
            "pre-accept tx {} not mined before activation",
            pre_accept_id.to_base58()
        ));
    }

    let pre_reject_in_block =
        wait_for_tx_in_block(&public_addr, &pre_reject_id, Duration::from_secs(5)).await?;
    if pre_reject_in_block.is_some() {
        return Err(anyhow!(
            "transaction {} unexpectedly accepted before activation",
            pre_reject_id.to_base58()
        ));
    }

    set_mining_enabled(&private_addr, true).await?;
    cluster.wait_for_grpc(Duration::from_secs(30)).await?;
    cluster
        .mine_to_height(activation_height + 1, Duration::from_secs(180))
        .await?;
    set_mining_enabled(&private_addr, false).await?;
    cluster.wait_for_grpc(Duration::from_secs(30)).await?;

    let notes_before_post =
        list_notes_with_retry(&miner_wallet, 5, Duration::from_millis(500)).await?;
    let tx_post_stub = create_tx_artifact(
        &miner_wallet, &note_post_stub.name, &recipient_pkh, note_post_stub.assets, post_amount,
        post_stub_fee, &miner_pkh,
    )
    .await?;
    log_tx_lmp("post_stub", &tx_post_stub.spends);
    let post_stub_id =
        submit_expect_accepted(&miner_wallet, &public_addr, &tx_post_stub.tx_path).await?;
    let expected_refund = note_post_stub
        .assets
        .saturating_sub(post_amount.saturating_add(post_stub_fee));

    set_mining_enabled(&private_addr, true).await?;
    cluster.wait_for_grpc(Duration::from_secs(30)).await?;
    cluster
        .mine_to_height(activation_height + 3, Duration::from_secs(180))
        .await?;
    set_mining_enabled(&private_addr, false).await?;
    cluster.wait_for_grpc(Duration::from_secs(30)).await?;

    let post_stub_in_block =
        wait_for_tx_in_block(&public_addr, &post_stub_id, Duration::from_secs(30)).await?;
    if post_stub_in_block.is_none() {
        return Err(anyhow!(
            "post-activation stub tx {} not mined",
            post_stub_id.to_base58()
        ));
    }

    let notes_after = list_notes_with_retry(&miner_wallet, 5, Duration::from_millis(500)).await?;
    let note_full = find_new_note_by_assets(
        &notes_before_post, &notes_after, expected_refund, activation_height,
    )?;
    let (pre_full_min, _) = compute_fee_bounds_for_note(
        &miner_wallet, &note_full, &recipient_pkh, &miner_pkh, constants.base_fee,
        constants.input_fee_divisor, constants.min_fee,
    )
    .await?;
    let full_fee = pre_full_min.saturating_add(1);
    let full_amount = choose_amount(note_full.assets, full_fee)?;
    let tx_full = create_tx_artifact(
        &miner_wallet, &note_full.name, &recipient_pkh, note_full.assets, full_amount, full_fee,
        &miner_pkh,
    )
    .await?;
    log_tx_lmp("full", &tx_full.spends);
    let full_id = submit_expect_accepted(&miner_wallet, &public_addr, &tx_full.tx_path).await?;

    set_mining_enabled(&private_addr, true).await?;
    cluster.wait_for_grpc(Duration::from_secs(30)).await?;
    let current_height = cluster.current_height().await?;
    let target_height = std::cmp::max(activation_height + 5, current_height.saturating_add(4));
    cluster
        .mine_to_height(target_height, Duration::from_secs(180))
        .await?;
    set_mining_enabled(&private_addr, false).await?;
    cluster.wait_for_grpc(Duration::from_secs(30)).await?;

    let full_in_block =
        wait_for_tx_in_block(&public_addr, &full_id, Duration::from_secs(30)).await?;
    if full_in_block.is_none() {
        return Err(anyhow!(
            "post-activation full-proof tx {} not mined",
            full_id.to_base58()
        ));
    }

    Ok(())
}

fn resolve_nockchain_bin() -> Option<PathBuf> {
    if let Ok(path) = env::var("NOCKCHAIN_BIN_NEW") {
        return absolutize_path(PathBuf::from(path));
    }
    let root = workspace_root().ok()?;
    let release = root.join("target/release/nockchain");
    if release.exists() {
        return Some(release);
    }
    let debug = root.join("target/debug/nockchain");
    if debug.exists() {
        return Some(debug);
    }
    None
}

fn resolve_wallet_bin() -> Option<PathBuf> {
    if let Ok(path) = env::var("NOCKCHAIN_WALLET_BIN") {
        return absolutize_path(PathBuf::from(path));
    }
    let root = workspace_root().ok()?;
    let release = root.join("target/release/nockchain-wallet");
    if release.exists() {
        return Some(release);
    }
    let debug = root.join("target/debug/nockchain-wallet");
    if debug.exists() {
        return Some(debug);
    }
    None
}

fn binary_supports_grpc_flags(path: &Path) -> bool {
    let output = Command::new(path).arg("--help").output();
    let Ok(output) = output else {
        return false;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    stdout.contains("--bind-public-grpc-addr") || stderr.contains("--bind-public-grpc-addr")
}

fn temp_run_dir(label: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let pid = std::process::id();
    let mut path = env::temp_dir();
    path.push(format!("nockchain-e2e-{}-{}-{}", label, pid, now));
    path
}

fn reserve_udp_port() -> Result<u16> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    Ok(socket.local_addr()?.port())
}

fn reserve_udp_port_pair() -> Result<u16> {
    for _ in 0..25 {
        let base = reserve_udp_port()?;
        if base < u16::MAX - 1 && UdpSocket::bind(("127.0.0.1", base + 1)).is_ok() {
            return Ok(base);
        }
    }
    Err(anyhow!("unable to reserve udp port pair"))
}

fn reserve_distinct_tcp_pairs() -> Result<(u16, u16)> {
    for _ in 0..100 {
        let public = TcpListener::bind("127.0.0.1:0")?;
        let public_base = public.local_addr()?.port();
        if public_base >= u16::MAX - 1 {
            continue;
        }
        let public_next = TcpListener::bind(("127.0.0.1", public_base + 1));
        if public_next.is_err() {
            continue;
        }
        let private = TcpListener::bind("127.0.0.1:0")?;
        let private_base = private.local_addr()?.port();
        if private_base >= u16::MAX - 1 {
            continue;
        }
        let private_next = TcpListener::bind(("127.0.0.1", private_base + 1));
        if private_next.is_err() {
            continue;
        }
        let public_range = public_base..=public_base + 1;
        let private_range = private_base..=private_base + 1;
        if public_range.contains(&private_base)
            || public_range.contains(&(private_base + 1))
            || private_range.contains(&public_base)
            || private_range.contains(&(public_base + 1))
        {
            continue;
        }
        drop(public);
        drop(private);
        return Ok((public_base, private_base));
    }
    Err(anyhow!("unable to reserve distinct tcp port pairs"))
}

fn extract_address(output: &str) -> Result<String> {
    let sanitized = strip_ansi(output);
    let lines: Vec<&str> = sanitized.lines().collect();
    if let Some(address) = extract_wrapped_address_after_label(&lines, "### Address")
        .or_else(|| extract_wrapped_address_after_label(&lines, "Address"))
        .or_else(|| extract_wrapped_address_after_active_master(&lines))
    {
        return Ok(address);
    }

    let regex = Regex::new(r"(?m)^\s*- Address:\s*([A-Za-z0-9]+)\s*\r?$")?;
    if let Some(caps) = regex.captures(&sanitized) {
        let addr = caps
            .get(1)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default();
        if !addr.is_empty() {
            return Ok(addr);
        }
    }
    Err(anyhow!("failed to parse keygen output"))
}

fn extract_wrapped_address_after_label(lines: &[&str], label: &str) -> Option<String> {
    lines.iter().enumerate().find_map(|(index, line)| {
        if line.trim() == label {
            collect_wrapped_address(lines, index + 1)
        } else {
            None
        }
    })
}

fn extract_wrapped_address_after_active_master(lines: &[&str]) -> Option<String> {
    lines.iter().enumerate().find_map(|(index, line)| {
        let trimmed = line.trim();
        let remainder = trimmed
            .strip_prefix("- Active master key is set to")
            .or_else(|| trimmed.strip_prefix("Active master key is set to"))?;
        let inline = address_chunk(remainder);
        let wrapped = collect_wrapped_address(lines, index + 1);
        match (inline, wrapped) {
            (Some(inline), Some(wrapped)) => Some(format!("{inline}{wrapped}")),
            (Some(inline), None) => Some(inline.to_string()),
            (None, Some(wrapped)) => Some(wrapped),
            (None, None) => None,
        }
    })
}

fn collect_wrapped_address(lines: &[&str], start: usize) -> Option<String> {
    let mut address = String::new();
    let mut started = false;
    for line in &lines[start..] {
        match address_chunk(line) {
            Some(chunk) => {
                started = true;
                address.push_str(chunk);
            }
            None if started => break,
            None => continue,
        }
    }
    if address.is_empty() {
        None
    } else {
        Some(address)
    }
}

fn address_chunk(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let trimmed = trimmed.strip_suffix('.').unwrap_or(trimmed);
    if trimmed.is_empty() || !trimmed.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return None;
    }
    Some(trimmed)
}

fn strip_ansi(input: &str) -> String {
    let regex = Regex::new(r"\x1b\[[0-9;]*m").expect("valid ansi regex");
    regex.replace_all(input, "").to_string()
}

async fn initialize_wallet_for_mining(
    wallet_bin: &Path,
    wallet_dir: &Path,
    private_port: u16,
) -> Result<String> {
    run_wallet_command_offline(wallet_bin, wallet_dir, private_port, "keygen", &[]).await?;
    let output = run_wallet_command_offline(
        wallet_bin,
        wallet_dir,
        private_port,
        "list-active-addresses",
        &[],
    )
    .await?;
    extract_address(&format!("{}\n{}", output.0, output.1))
}

async fn run_wallet_command_offline(
    wallet_bin: &Path,
    wallet_dir: &Path,
    private_port: u16,
    command: &str,
    args: &[&str],
) -> Result<(String, String)> {
    std::fs::create_dir_all(wallet_dir)?;
    let output = tokio::process::Command::new(wallet_bin)
        .current_dir(wallet_dir)
        .env("NOCKAPP_HOME", wallet_dir)
        .arg("--client")
        .arg("private")
        .arg("--private-grpc-server-port")
        .arg(private_port.to_string())
        .arg("--fakenet")
        .arg(command)
        .args(args)
        .output()
        .await?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(anyhow!(
            "wallet '{}' exited with {:?}: {}",
            command,
            output.status.code(),
            stderr.trim()
        ));
    }
    Ok((stdout, stderr))
}

fn workspace_root() -> Result<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(|path| path.parent())
        .and_then(|path| path.parent())
        .ok_or_else(|| anyhow!("failed to resolve workspace root"))?;
    Ok(root.to_path_buf())
}

fn absolutize_path(path: PathBuf) -> Option<PathBuf> {
    if path.is_absolute() {
        return Some(path);
    }
    let root = workspace_root().ok()?;
    let candidate = root.join(path);
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

fn choose_amount(assets: u64, fee: u64) -> Result<u64> {
    if assets <= fee + 1 {
        return Err(anyhow!("note assets {} too small for fee {}", assets, fee));
    }
    let amount = if fee + 10_000 < assets {
        10_000
    } else {
        assets.saturating_sub(fee + 1)
    };
    if amount == 0 {
        return Err(anyhow!("computed zero spend amount"));
    }
    Ok(amount)
}

fn parse_notes(output: &str) -> Result<Vec<NoteInfo>> {
    let regex = Regex::new(
        r"(?s)- Name: (\[[^\]]+\]).*?- Assets \(nicks\): ([0-9,._]+).*?- Block Height: ([0-9]+)",
    )?;
    let mut notes = Vec::new();
    for caps in regex.captures_iter(output) {
        let name = caps
            .get(1)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default();
        let assets_raw = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        let height_raw = caps.get(3).map(|m| m.as_str()).unwrap_or("");
        let assets = parse_number(assets_raw)?;
        let height = parse_number(height_raw)?;
        notes.push(NoteInfo {
            name,
            height,
            assets,
        });
    }
    if notes.is_empty() {
        let sanitized = strip_ansi(output);
        let preview: String = sanitized.chars().take(400).collect();
        return Err(anyhow!("no notes parsed from wallet output: {}", preview));
    }
    Ok(notes)
}

fn find_new_note_by_assets(
    before: &[NoteInfo],
    after: &[NoteInfo],
    expected_assets: u64,
    min_height: u64,
) -> Result<NoteInfo> {
    let existing: HashSet<String> = before.iter().map(|note| note.name.clone()).collect();
    let mut matches: Vec<NoteInfo> = after
        .iter()
        .filter(|note| !existing.contains(&note.name))
        .filter(|note| note.height >= min_height)
        .filter(|note| note.assets == expected_assets)
        .cloned()
        .collect();
    matches.sort_by_key(|note| note.height);
    if let Some(note) = matches.pop() {
        return Ok(note);
    }
    let new_notes: Vec<String> = after
        .iter()
        .filter(|note| !existing.contains(&note.name))
        .map(|note| format!("{}:{}:{}", note.name, note.height, note.assets))
        .collect();
    Err(anyhow!(
        "no post-activation refund note found with assets {} (new notes: {:?})", expected_assets,
        new_notes
    ))
}

async fn list_notes_with_retry(
    wallet: &WalletClient,
    attempts: usize,
    delay: Duration,
) -> Result<Vec<NoteInfo>> {
    let mut last_err = None;
    for _ in 0..attempts {
        let output = wallet.run("list-notes", &Vec::new()).await?;
        let combined = format!("{}\n{}", output.stdout, output.stderr);
        match parse_notes(&combined) {
            Ok(notes) => return Ok(notes),
            Err(err) => last_err = Some(err),
        }
        sleep(delay).await;
    }
    Err(last_err.unwrap_or_else(|| anyhow!("failed to list notes")))
}

struct ChainConstants {
    v1_phase: u64,
    bythos_phase: u64,
    coinbase_timelock_min: u64,
    min_fee: u64,
    base_fee: u64,
    input_fee_divisor: u64,
}

fn extract_constants(constants_bytes: &[u8]) -> Result<ChainConstants> {
    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from(constants_bytes.to_vec()))?;
    let space = slab.noun_space();
    let Some(constants) = decode_optional_optional_noun(noun.in_space(&space))? else {
        return Err(anyhow!("constants peek returned none"));
    };
    let cell = constants
        .as_cell()
        .map_err(|_| anyhow!("constants not a cell"))?;
    let v1_phase = cell
        .head()
        .as_atom()
        .map_err(|_| anyhow!("v1-phase not an atom"))?
        .as_u64()
        .map_err(|_| anyhow!("v1-phase not a u64"))?;
    let tail = cell
        .tail()
        .as_cell()
        .map_err(|_| anyhow!("constants missing bythos-phase"))?;
    let bythos_phase = tail
        .head()
        .as_atom()
        .map_err(|_| anyhow!("bythos-phase not an atom"))?
        .as_u64()
        .map_err(|_| anyhow!("bythos-phase not a u64"))?;
    let tail = tail
        .tail()
        .as_cell()
        .map_err(|_| anyhow!("constants missing note-data constraints"))?;
    let note_data = tail
        .head()
        .as_cell()
        .map_err(|_| anyhow!("note-data not a cell"))?;
    let min_fee = note_data
        .tail()
        .as_atom()
        .map_err(|_| anyhow!("note-data min-fee not an atom"))?
        .as_u64()
        .map_err(|_| anyhow!("note-data min-fee not a u64"))?;
    let tail = tail
        .tail()
        .as_cell()
        .map_err(|_| anyhow!("constants missing base-fee"))?;
    let base_fee = tail
        .head()
        .as_atom()
        .map_err(|_| anyhow!("base-fee not an atom"))?
        .as_u64()
        .map_err(|_| anyhow!("base-fee not a u64"))?;
    let tail = tail
        .tail()
        .as_cell()
        .map_err(|_| anyhow!("constants missing input-fee-divisor"))?;
    let input_fee_divisor = tail
        .head()
        .as_atom()
        .map_err(|_| anyhow!("input-fee-divisor not an atom"))?
        .as_u64()
        .map_err(|_| anyhow!("input-fee-divisor not a u64"))?;
    let v0_constants = tail.tail();
    let coinbase_timelock_min = nth_tuple_atom(v0_constants, 10)
        .map_err(|err| anyhow!("failed to parse coinbase-timelock-min: {err}"))?;
    Ok(ChainConstants {
        v1_phase,
        bythos_phase,
        coinbase_timelock_min,
        min_fee,
        base_fee,
        input_fee_divisor,
    })
}

fn nth_tuple_atom(noun: NounHandle<'_>, index: usize) -> Result<u64> {
    if index == 0 {
        return Err(anyhow!("tuple index must be >= 1"));
    }
    let mut cur = noun;
    let mut idx = 1usize;
    loop {
        if let Ok(cell) = cur.as_cell() {
            if idx == index {
                return cell
                    .head()
                    .as_atom()
                    .map_err(|_| anyhow!("tuple entry {index} is not an atom"))?
                    .as_u64()
                    .map_err(|_| anyhow!("tuple entry {index} is not a u64"));
            }
            cur = cell.tail();
            idx += 1;
            continue;
        }
        if idx == index {
            return cur
                .as_atom()
                .map_err(|_| anyhow!("tuple entry {index} is not an atom"))?
                .as_u64()
                .map_err(|_| anyhow!("tuple entry {index} is not a u64"));
        }
        return Err(anyhow!("tuple has fewer than {index} entries"));
    }
}

async fn wait_for_bythos_phase(
    cluster: &NockchainCluster,
    expected: u64,
    timeout: Duration,
) -> Result<ChainConstants> {
    let deadline = Instant::now() + timeout;
    loop {
        let constants_bytes = cluster.fetch_constants().await?;
        let constants = extract_constants(&constants_bytes)?;
        if constants.bythos_phase == expected {
            return Ok(constants);
        }
        if Instant::now() >= deadline {
            return Ok(constants);
        }
        sleep(Duration::from_millis(200)).await;
    }
}

fn parse_number(raw: &str) -> Result<u64> {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return Err(anyhow!("failed to parse numeric value from '{}'", raw));
    }
    digits
        .parse::<u64>()
        .map_err(|err| anyhow!("invalid numeric value '{}': {}", raw, err))
}

async fn create_tx_artifact(
    wallet: &WalletClient,
    note_name: &str,
    recipient_pkh: &str,
    note_assets: u64,
    amount: u64,
    fee: u64,
    refund_pkh: &str,
) -> Result<TxArtifact> {
    if amount + fee >= note_assets {
        return Err(anyhow!(
            "amount {} plus fee {} exceeds note assets {}", amount, fee, note_assets
        ));
    }

    let recipient = format!(
        "{{\"kind\":\"p2pkh\",\"address\":\"{}\",\"amount\":{}}}",
        recipient_pkh, amount
    );
    let args = vec![
        "--names".to_string(),
        note_name.to_string(),
        "--recipient".to_string(),
        recipient,
        "--fee".to_string(),
        fee.to_string(),
        "--allow-low-fee".to_string(),
        "--refund-pkh".to_string(),
        refund_pkh.to_string(),
    ];

    let prior_txs = list_tx_files(wallet.dir())?;
    let output = wallet.run("create-tx", &args).await?;
    let tx_path = match extract_tx_path(&output.stdout).or_else(|_| extract_tx_path(&output.stderr))
    {
        Ok(tx_path) => match resolve_tx_path(wallet.dir(), &tx_path) {
            Ok(candidate) => candidate,
            Err(_) => newest_tx(wallet.dir(), &prior_txs).map_err(|err| {
                anyhow!(
                    "{}; create-tx stdout: {} stderr: {}",
                    err,
                    output.stdout.trim(),
                    output.stderr.trim()
                )
            })?,
        },
        Err(_) => newest_tx(wallet.dir(), &prior_txs).map_err(|err| {
            anyhow!(
                "{}; create-tx stdout: {} stderr: {}",
                err,
                output.stdout.trim(),
                output.stderr.trim()
            )
        })?,
    };
    load_transaction(&tx_path)
}

fn extract_tx_path(output: &str) -> Result<PathBuf> {
    let sanitized = strip_ansi(output);
    let patterns = [
        r"(?m)^\s*- Saved transaction to ([^\s]+)", r"(?m)^\s*Saved transaction to ([^\s]+)",
        r"(?m)Saved transaction to ([^\s]+)",
    ];
    for pattern in patterns {
        let regex = Regex::new(pattern)?;
        if let Some(caps) = regex.captures(&sanitized) {
            let path = caps.get(1).map(|m| m.as_str().trim()).unwrap_or_default();
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
    }
    Err(anyhow!("failed to parse create-tx output"))
}

fn extract_tx_id(output: &str) -> Result<String> {
    let sanitized = strip_ansi(output);
    let patterns =
        [r"Validation for TX\s*([A-Za-z0-9]+)\s*passed", r"TX\s*([A-Za-z0-9]+)\s*passed"];
    for pattern in patterns {
        let regex = Regex::new(pattern)?;
        if let Some(caps) = regex.captures(&sanitized) {
            let tx_id = caps
                .get(1)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();
            if !tx_id.is_empty() {
                return Ok(tx_id);
            }
        }
    }
    Err(anyhow!("failed to parse send-tx output"))
}

fn resolve_tx_path(wallet_dir: &Path, tx_path: &Path) -> Result<PathBuf> {
    if tx_path.is_absolute() {
        return Ok(tx_path.to_path_buf());
    }
    let candidate = wallet_dir.join(tx_path);
    if candidate.exists() {
        return Ok(candidate);
    }
    Err(anyhow!("transaction path not found: {}", tx_path.display()))
}

fn list_tx_files(wallet_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for tx_dir in tx_dirs(wallet_dir) {
        if !tx_dir.exists() {
            continue;
        }
        for entry in std::fs::read_dir(&tx_dir)? {
            let path = entry?.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("tx") {
                continue;
            }
            if seen.insert(path.clone()) {
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

fn tx_dirs(wallet_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    dirs.push(wallet_dir.join("txs"));
    dirs.push(wallet_dir.join("wallet").join("txs"));
    let system_dir = nockapp::system_data_dir().join("wallet").join("txs");
    if !dirs.contains(&system_dir) {
        dirs.push(system_dir);
    }
    dirs
}

fn newest_tx(wallet_dir: &Path, prior: &[PathBuf]) -> Result<PathBuf> {
    let mut candidates: Vec<PathBuf> = list_tx_files(wallet_dir)?
        .into_iter()
        .filter(|path| !prior.contains(path))
        .collect();
    if candidates.is_empty() {
        candidates = list_tx_files(wallet_dir)?;
    }
    candidates.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    candidates
        .pop()
        .ok_or_else(|| anyhow!("no transaction output found in txs"))
}

struct ParsedTransaction {
    spends: Spends,
    witness_data: WitnessData,
}

enum WitnessData {
    Legacy(Vec<(Name, Signature)>),
    Witness(Vec<(Name, Witness)>),
}

impl ParsedTransaction {
    fn from_noun(noun: NounHandle<'_>) -> Result<Self> {
        let cell = noun
            .as_cell()
            .map_err(|_| anyhow!("transaction not a cell"))?;
        let tag = cell
            .head()
            .as_atom()
            .map_err(|_| anyhow!("transaction tag not an atom"))?
            .as_u64()
            .map_err(|_| anyhow!("transaction tag not a u64"))?;
        if tag != 1 {
            return Err(anyhow!("unsupported transaction tag {}", tag));
        }
        let tail = cell.tail();
        let cell = tail
            .as_cell()
            .map_err(|_| anyhow!("transaction missing name"))?;
        let tail = cell.tail();
        let cell = tail
            .as_cell()
            .map_err(|_| anyhow!("transaction missing spends"))?;
        let spends_noun = cell.head();
        let tail = cell.tail();
        let cell = tail
            .as_cell()
            .map_err(|_| anyhow!("transaction missing display"))?;
        let witness_noun = cell.tail();
        let spends = Spends::from_noun_handle(&spends_noun)
            .map_err(|err| anyhow!("failed to decode spends: {}", err))?;
        let witness_data = WitnessData::from_noun(witness_noun)?;
        Ok(Self {
            spends,
            witness_data,
        })
    }
}

impl WitnessData {
    fn from_noun(noun: NounHandle<'_>) -> Result<Self> {
        let cell = noun
            .as_cell()
            .map_err(|_| anyhow!("witness-data not a cell"))?;
        let tag = cell
            .head()
            .as_atom()
            .map_err(|_| anyhow!("witness-data tag not an atom"))?
            .as_u64()
            .map_err(|_| anyhow!("witness-data tag not a u64"))?;
        let map_noun = cell.tail();
        match tag {
            0 => {
                let entries = decode_witness_map::<Signature>(map_noun)
                    .map_err(|err| anyhow!("failed to decode legacy witness-data: {}", err))?;
                Ok(Self::Legacy(entries))
            }
            1 => {
                let entries = decode_witness_map::<Witness>(map_noun)
                    .map_err(|err| anyhow!("failed to decode witness-data: {}", err))?;
                Ok(Self::Witness(entries))
            }
            _ => Err(anyhow!("unsupported witness-data tag {}", tag)),
        }
    }
}

fn decode_witness_map<T: NounDecode>(noun: NounHandle<'_>) -> Result<Vec<(Name, T)>> {
    let entries = HoonMapIter::new(&noun)
        .filter(|entry| entry.is_cell())
        .map(|entry| {
            let cell = entry
                .as_cell()
                .map_err(|_| noun_serde::NounDecodeError::Custom("map entry not a pair".into()))?;
            let name = Name::from_noun_handle(&cell.head())?;
            let value = T::from_noun_handle(&cell.tail())?;
            Ok((name, value))
        })
        .collect::<Result<Vec<_>, noun_serde::NounDecodeError>>()?;
    Ok(entries)
}

fn apply_witness_data(spends: Spends, witness_data: WitnessData) -> Result<Spends> {
    let mut signed = Vec::with_capacity(spends.0.len());
    match witness_data {
        WitnessData::Legacy(entries) => {
            for (name, spend) in spends.0 {
                match spend {
                    Spend::Legacy(mut spend0) => {
                        let signature = entries
                            .iter()
                            .find(|(entry_name, _)| entry_name == &name)
                            .map(|(_, sig)| sig)
                            .ok_or_else(|| {
                                anyhow!("missing legacy witness-data for spend {:?}", name)
                            })?;
                        spend0.signature = signature.clone();
                        signed.push((name, Spend::Legacy(spend0)));
                    }
                    Spend::Witness(_) => {
                        return Err(anyhow!(
                            "legacy witness-data provided for v1 spend {:?}", name
                        ));
                    }
                }
            }
        }
        WitnessData::Witness(entries) => {
            for (name, spend) in spends.0 {
                match spend {
                    Spend::Witness(mut spend1) => {
                        let witness = entries
                            .iter()
                            .find(|(entry_name, _)| entry_name == &name)
                            .map(|(_, wit)| wit)
                            .ok_or_else(|| anyhow!("missing witness-data for spend {:?}", name))?;
                        spend1.witness = witness.clone();
                        signed.push((name, Spend::Witness(spend1)));
                    }
                    Spend::Legacy(_) => {
                        return Err(anyhow!(
                            "v1 witness-data provided for legacy spend {:?}", name
                        ));
                    }
                }
            }
        }
    }
    Ok(Spends(signed))
}

fn load_transaction(path: &Path) -> Result<TxArtifact> {
    let data = std::fs::read(path)?;
    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from(data.clone()))?;
    let space = slab.noun_space();
    let parsed = ParsedTransaction::from_noun(noun.in_space(&space))?;
    let spends = apply_witness_data(parsed.spends, parsed.witness_data)?;
    Ok(TxArtifact {
        tx_path: path.to_path_buf(),
        spends,
    })
}

fn compute_fee_bounds(
    spends: &Spends,
    base_fee: u64,
    input_fee_divisor: u64,
    min_fee: u64,
) -> Result<(u64, u64)> {
    let (seed_words, witness_words) = count_words_from_spends(spends)?;
    let pre_bythos_base_fee = base_fee.saturating_mul(2);
    let seed_fee_pre = seed_words.saturating_mul(pre_bythos_base_fee);
    let witness_fee_pre = witness_words.saturating_mul(pre_bythos_base_fee);
    let seed_fee_post = seed_words.saturating_mul(base_fee);
    let witness_fee_post = witness_words
        .saturating_mul(base_fee)
        .saturating_div(input_fee_divisor.max(1));
    let pre_min = seed_fee_pre.saturating_add(witness_fee_pre).max(min_fee);
    let post_min = seed_fee_post.saturating_add(witness_fee_post).max(min_fee);
    Ok((pre_min, post_min))
}

async fn compute_fee_bounds_for_note(
    wallet: &WalletClient,
    note: &NoteInfo,
    recipient_pkh: &str,
    refund_pkh: &str,
    base_fee: u64,
    input_fee_divisor: u64,
    min_fee: u64,
) -> Result<(u64, u64)> {
    // Use a tiny fee so the probe tx keeps a refund output and matches real tx structure.
    let analysis_fee = 1u64;
    if note.assets <= analysis_fee + 1 {
        return Err(anyhow!(
            "note assets {} too small to build analysis transaction", note.assets
        ));
    }
    let analysis_amount = choose_amount(note.assets, analysis_fee)?;
    let analysis_tx = create_tx_artifact(
        wallet, &note.name, recipient_pkh, note.assets, analysis_amount, analysis_fee, refund_pkh,
    )
    .await?;
    compute_fee_bounds(&analysis_tx.spends, base_fee, input_fee_divisor, min_fee)
}

fn choose_mid_fee(pre_min: u64, post_min: u64, label: &str) -> Result<u64> {
    if pre_min <= post_min {
        return Err(anyhow!(
            "expected pre-activation fee {} to exceed post-activation fee {} for {}", pre_min,
            post_min, label
        ));
    }
    let mid_fee = post_min + 1;
    if mid_fee >= pre_min {
        return Err(anyhow!(
            "no fee gap for {} (pre {}, post {})", label, pre_min, post_min
        ));
    }
    Ok(mid_fee)
}

fn count_words_from_spends(spends: &Spends) -> Result<(u64, u64)> {
    let seed_words = count_seed_words_from_spends(spends)?;
    let witness_words = count_witness_words_from_spends(spends)?;
    Ok((seed_words, witness_words))
}

fn count_seed_words_from_spends(spends: &Spends) -> Result<u64> {
    let mut note_data_by_root: HashMap<Hash, HashMap<String, NoteDataValue>> = HashMap::new();
    for (_name, spend) in &spends.0 {
        let seeds = match spend {
            Spend::Legacy(spend0) => &spend0.seeds.0,
            Spend::Witness(spend1) => &spend1.seeds.0,
        };
        for seed in seeds {
            let entry = note_data_by_root.entry(seed.lock_root.clone()).or_default();
            for note_entry in seed.note_data.iter() {
                entry.insert(note_entry.key.clone(), note_entry.value.clone());
            }
        }
    }
    let mut total = 0u64;
    for entries in note_data_by_root.values() {
        let note_entries: Vec<NoteDataEntry> = entries
            .iter()
            .map(|(key, blob)| NoteDataEntry::new(key.clone(), blob.clone()))
            .collect();
        let note_data = NoteData::new(note_entries);
        let mut slab: NounSlab = NounSlab::new();
        let noun = note_data.to_noun(&mut slab);
        let space = slab.noun_space();
        total = total.saturating_add(count_leaves(noun.in_space(&space)));
    }
    Ok(total)
}

fn count_witness_words_from_spends(spends: &Spends) -> Result<u64> {
    let mut total = 0u64;
    for (_name, spend) in &spends.0 {
        let mut slab: NounSlab = NounSlab::new();
        let noun = match spend {
            Spend::Legacy(spend0) => spend0.signature.to_noun(&mut slab),
            Spend::Witness(spend1) => spend1.witness.to_noun(&mut slab),
        };
        let space = slab.noun_space();
        total = total.saturating_add(count_leaves(noun.in_space(&space)));
    }
    Ok(total)
}

fn count_leaves(noun: NounHandle<'_>) -> u64 {
    if noun.is_atom() {
        return 1;
    }
    let cell = noun.as_cell().expect("cell expected");
    count_leaves(cell.head()) + count_leaves(cell.tail())
}

fn decode_optional_optional_noun<'a>(noun: NounHandle<'a>) -> Result<Option<NounHandle<'a>>> {
    let Some(inner) = decode_option_noun(noun)? else {
        return Ok(None);
    };
    decode_option_noun(inner)
}

fn decode_option_noun<'a>(noun: NounHandle<'a>) -> Result<Option<NounHandle<'a>>> {
    if let Ok(atom) = noun.as_atom() {
        let value = atom
            .as_u64()
            .map_err(|err| anyhow!("invalid option atom: {err}"))?;
        if value == 0 {
            return Ok(None);
        }
        return Err(anyhow!("invalid option atom {value}, expected 0"));
    }

    let cell = noun
        .as_cell()
        .map_err(|_| anyhow!("option should be atom 0 or [0 value] cell"))?;
    let tag = cell
        .head()
        .as_atom()
        .map_err(|_| anyhow!("option cell tag should be an atom"))?
        .as_u64()
        .map_err(|err| anyhow!("option cell tag should be u64: {err}"))?;
    if tag != 0 {
        return Err(anyhow!("invalid option cell tag {tag}, expected 0"));
    }
    Ok(Some(cell.tail()))
}

async fn submit_expect_accepted(
    wallet: &WalletClient,
    public_addr: &str,
    tx_path: &Path,
) -> Result<Hash> {
    let tx_arg = tx_path.to_string_lossy().to_string();
    let output = wallet.run("send-tx", &[tx_arg]).await?;
    let combined = format!("{}\n{}", output.stdout, output.stderr);
    let tx_id_text = extract_tx_id(&combined)?;
    let tx_id = Hash::from_base58(&tx_id_text)
        .map_err(|err| anyhow!("failed to parse tx id {}: {}", tx_id_text, err))?;
    let accepted = wait_for_tx_accepted(public_addr, &tx_id, Duration::from_secs(10)).await?;
    if !accepted {
        return Err(anyhow!("transaction {} not accepted", tx_id.to_base58()));
    }
    Ok(tx_id)
}

fn log_tx_lmp(label: &str, spends: &Spends) {
    if env::var("DEBUG_LMP").is_err() {
        return;
    }
    eprintln!("DEBUG_LMP {}", label);
    for (name, spend) in &spends.0 {
        match spend {
            Spend::Witness(spend1) => {
                let lmp = &spend1.witness.lock_merkle_proof;
                let version = match lmp {
                    LockMerkleProof::Full(_) => "full",
                    LockMerkleProof::Stub(_) => "stub",
                };
                eprintln!(
                    "  spend={:?} lmp_version={} axis={} pkh_sigs={}",
                    name,
                    version,
                    lmp.axis(),
                    spend1.witness.pkh_signature.0.len()
                );
            }
            Spend::Legacy(_) => {
                eprintln!("  spend={:?} legacy", name);
            }
        }
    }
}

async fn wait_for_tx_accepted(addr: &str, tx_id: &Hash, timeout: Duration) -> Result<bool> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let accepted = transaction_accepted(addr, tx_id).await?;
        if accepted {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        sleep(Duration::from_millis(200)).await;
    }
}

#[derive(Clone)]
struct TxArtifact {
    tx_path: PathBuf,
    spends: Spends,
}

#[derive(Clone)]
struct NoteInfo {
    name: String,
    height: u64,
    assets: u64,
}
