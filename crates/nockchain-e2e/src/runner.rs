use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash as StdHash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use nockapp::noun::slab::{slab_equality, NounSlab};
use nockapp::noun::IntoSlab;
use nockchain::config::{DEFAULT_FAKENET_BYTHOS_PHASE, DEFAULT_FAKENET_V1_PHASE};
use nockchain_testkit::{
    Action, Assert, NodeSpec, ReqResGenerationExpectation, Scenario, SubmitTxExpect,
    WalletCaptureSource,
};
use nockchain_types::tx_engine::common::Hash;
use nockchain_types::tx_engine::v1::RawTx;
use nockchain_types::{fakenet_blockchain_constants, Seconds};
use nockvm::noun::{NounAllocator, NounHandle};
use noun_serde::NounDecode;
use regex::Regex;
use tokio::process::Command;
use tokio::time::{sleep, Instant};
use tracing::info;

use crate::grpc::{
    poke_private, set_mining_enabled, submit_raw_tx, transaction_accepted, wait_for_tx_in_block,
    SubmitTxOutcome,
};
use crate::node::{NodeManager, NodeMode};
use crate::report::Report;

pub struct RunOptions {
    pub scenario_path: PathBuf,
    pub nockchain_bin: Option<PathBuf>,
    pub wallet_bin: Option<PathBuf>,
    pub work_dir: PathBuf,
    pub base_grpc_port: u16,
    pub base_private_grpc_port: u16,
    pub base_p2p_port: u16,
    pub docker: bool,
    pub docker_image: Option<String>,
    pub keep_artifacts: bool,
}

#[derive(Default)]
struct RunState {
    tx_ids: HashMap<String, Hash>,
    last_tx: Option<Hash>,
    vars: HashMap<String, String>,
}

impl RunState {
    fn record_tx(&mut self, label: Option<&str>, tx_id: Hash) {
        self.last_tx = Some(tx_id.clone());
        if let Some(label) = label {
            self.tx_ids.insert(label.to_string(), tx_id);
        }
    }

    fn set_var(&mut self, key: &str, value: String) {
        self.vars.insert(key.to_string(), value);
    }
}

pub async fn run_scenario(options: RunOptions) -> Result<()> {
    seed_indexed_port_env_vars("BASE_GRPC_PORT", options.base_grpc_port, 16);
    seed_indexed_port_env_vars("BASE_PRIVATE_GRPC_PORT", options.base_private_grpc_port, 16);
    seed_indexed_port_env_vars("BASE_P2P_PORT", options.base_p2p_port, 16);
    seed_binary_env_vars(&options)?;

    let mut scenario = Scenario::load_from_path(&options.scenario_path)
        .map_err(|err| anyhow!("scenario load failed: {err}"))?;
    expand_scenario_env(&mut scenario)?;

    let run_id = format!("{}-{}", sanitize_name(&scenario.name), scenario.seed);

    let work_dir = absolutize_work_dir(&options.work_dir)?;
    let run_dir = work_dir.join(&run_id);
    std::fs::create_dir_all(&run_dir)?;

    let mut report = Report::started(&scenario.name, scenario.seed, &run_id);

    let result = run_inner(&scenario, &run_dir, &options, &mut report).await;
    match &result {
        Ok(_) => report.finish_ok(),
        Err(err) => report.finish_err(&err.to_string()),
    }

    report.write_json(run_dir.join("report.json"))?;

    result
}

fn ensure_env_var(key: &str, value: String) {
    if std::env::var_os(key).is_none() {
        std::env::set_var(key, value);
    }
}

fn seed_indexed_port_env_vars(prefix: &str, base: u16, count: u16) {
    ensure_env_var(prefix, base.to_string());
    for offset in 1..count {
        if let Some(port) = base.checked_add(offset) {
            ensure_env_var(&format!("{prefix}_{offset}"), port.to_string());
        }
    }
}

fn seed_binary_env_vars(options: &RunOptions) -> Result<()> {
    if !options.docker {
        let nockchain_bin = options
            .nockchain_bin
            .clone()
            .unwrap_or_else(|| PathBuf::from("target/release/nockchain"));
        ensure_env_var("NOCKCHAIN_BIN_NEW", env_path(&nockchain_bin)?);
    }

    let wallet_bin = options
        .wallet_bin
        .clone()
        .unwrap_or_else(|| PathBuf::from("target/release/nockchain-wallet"));
    ensure_env_var("NOCKCHAIN_WALLET_BIN", env_path(&wallet_bin)?);

    if std::env::var_os("NOCKCHAIN_E2E_BIN").is_none() {
        if let Ok(current_exe) = std::env::current_exe() {
            ensure_env_var("NOCKCHAIN_E2E_BIN", env_path(&current_exe)?);
        }
    }
    ensure_env_var(
        "NOCKCHAIN_E2E_BIN",
        env_path(&PathBuf::from("target/release/nockchain-e2e"))?,
    );

    Ok(())
}

fn env_path(path: &Path) -> Result<String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("resolve binary environment path cwd")?
            .join(path)
    };
    Ok(path.to_string_lossy().to_string())
}

fn absolutize_work_dir(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir().context("resolve e2e work dir cwd")?;
    Ok(cwd.join(path))
}

fn expand_scenario_env(scenario: &mut Scenario) -> Result<()> {
    for node in &mut scenario.nodes {
        expand_node_env(node)?;
    }
    Ok(())
}

fn expand_node_env(node: &mut nockchain_testkit::scenario::NodeSpec) -> Result<()> {
    if let Some(addr) = node.grpc_public_addr.as_mut() {
        *addr = expand_env_vars(addr)?;
    }
    for peer in node.peers.iter_mut() {
        *peer = expand_env_vars(peer)?;
    }
    for peer in node.force_peers.iter_mut() {
        *peer = expand_env_vars(peer)?;
    }
    for bind in node.bind.iter_mut() {
        *bind = expand_env_vars(bind)?;
    }
    for extra in node.extra_args.iter_mut() {
        *extra = expand_env_vars(extra)?;
    }
    for peer_from in node.peer_from.iter_mut() {
        peer_from.listen = expand_env_vars(&peer_from.listen)?;
    }
    for peer_from in node.restart_peer_from.iter_mut() {
        peer_from.listen = expand_env_vars(&peer_from.listen)?;
    }
    for value in node.env.values_mut() {
        *value = expand_env_vars(value)?;
    }
    if let Some(binary) = node.binary.as_mut() {
        let expanded = expand_env_vars(&binary.to_string_lossy())?;
        *binary = PathBuf::from(expanded);
    }
    if let Some(data_dir) = node.data_dir.as_mut() {
        let expanded = expand_env_vars(&data_dir.to_string_lossy())?;
        *data_dir = PathBuf::from(expanded);
    }
    Ok(())
}

async fn run_inner(
    scenario: &Scenario,
    run_dir: &Path,
    options: &RunOptions,
    report: &mut Report,
) -> Result<()> {
    let mode = if options.docker {
        let image = options
            .docker_image
            .clone()
            .unwrap_or_else(|| "nockchain-e2e:latest".to_string());
        let network = docker_network_name(run_dir);
        NodeMode::Docker { image, network }
    } else {
        NodeMode::Process
    };

    let mut node_manager = NodeManager::new(
        &scenario.nodes,
        run_dir,
        options.nockchain_bin.clone(),
        options.base_grpc_port,
        options.base_private_grpc_port,
        options.base_p2p_port,
        mode,
    )?;

    let mut state = RunState::default();
    seed_run_state(&mut state, run_dir, &options.scenario_path)?;
    let result = run_steps(
        &mut node_manager, scenario, run_dir, options, &mut state, report,
    )
    .await;

    // Collect node summaries before shutdown.
    for spec in &scenario.nodes {
        let head = node_manager.fetch_private_head_if_current(&spec.id).await;
        match head {
            Ok(Some(h)) => report.record_node(
                &spec.id,
                Some(h.height),
                h.block_id.as_ref().map(|id| format!("{:?}", id)),
            ),
            Ok(None) => report.record_node(&spec.id, None, None),
            Err(_) => report.record_node(&spec.id, None, None),
        }
    }

    let shutdown = node_manager.shutdown_all().await;

    match (result, shutdown) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(err), _) => Err(err),
        (Ok(()), Err(err)) => Err(err),
    }
}

async fn run_steps(
    node_manager: &mut NodeManager,
    scenario: &Scenario,
    run_dir: &Path,
    options: &RunOptions,
    state: &mut RunState,
    report: &mut Report,
) -> Result<()> {
    use crate::report::StepTimer;

    for (step_index, action) in scenario.steps.iter().enumerate() {
        let timer = StepTimer::start();
        let action_name = step_action_name(action);
        match action {
            Action::StartNodes { ids } => {
                info!("starting nodes: {:?}", ids);
                node_manager.start_nodes(ids).await?;
            }
            Action::StopNodes { ids } => {
                info!("stopping nodes: {:?}", ids);
                node_manager.stop_nodes(ids).await?;
            }
            Action::WaitForGrpc { node, timeout_ms } => {
                let timeout = timeout_ms
                    .map(Duration::from_millis)
                    .unwrap_or_else(|| Duration::from_secs(30));
                node_manager.wait_for_grpc(node, timeout).await?;
            }
            Action::WaitForHeight {
                node,
                height,
                timeout_ms,
            } => {
                let timeout = timeout_ms
                    .map(Duration::from_millis)
                    .unwrap_or_else(|| Duration::from_secs(30));
                node_manager.wait_for_height(node, *height, timeout).await?;
            }
            Action::WaitForHeadsEqual { nodes, timeout_ms } => {
                let timeout = timeout_ms
                    .map(Duration::from_millis)
                    .unwrap_or_else(|| Duration::from_secs(30));
                wait_for_heads_equal(node_manager, nodes, timeout).await?;
            }
            Action::WaitForTxAccepted {
                node,
                tx,
                timeout_ms,
            } => {
                let timeout = timeout_ms
                    .map(Duration::from_millis)
                    .unwrap_or_else(|| Duration::from_secs(30));
                let tx_value = expand_vars(tx, state)?;
                let tx_id = resolve_tx_id(state, &tx_value)?;
                wait_for_tx_accepted(node_manager, node, &tx_id, timeout).await?;
            }
            Action::WaitForTxInBlock {
                node,
                tx,
                timeout_ms,
            } => {
                let timeout = timeout_ms
                    .map(Duration::from_millis)
                    .unwrap_or_else(|| Duration::from_secs(30));
                let tx_value = expand_vars(tx, state)?;
                let tx_id = resolve_tx_id(state, &tx_value)?;
                let addr = node_manager.grpc_public_addr(node)?;
                let Some(height) = wait_for_tx_in_block(addr, &tx_id, timeout).await? else {
                    return Err(anyhow!(
                        "timed out waiting for tx '{}' to land in a block on node '{}'",
                        tx_id.to_base58(),
                        node
                    ));
                };
                info!(
                    node = node.as_str(),
                    tx_id = %tx_id.to_base58(),
                    height,
                    "transaction confirmed in block"
                );
            }
            Action::PeekConstants { node } => {
                let actual = node_manager.fetch_constants(node).await?;
                let output_dir = run_dir.join("peeks").join("constants");
                std::fs::create_dir_all(&output_dir)?;
                let actual_path = output_dir.join(format!("{node}.jam"));
                std::fs::write(&actual_path, &actual)?;
                std::fs::write(output_dir.join(format!("{node}.hex")), hex::encode(&actual))?;

                let spec = find_node_spec(scenario, node)?;
                if let Some(expected) = expected_constants(spec) {
                    let expected_path = output_dir.join(format!("{node}.expected.jam"));
                    std::fs::write(&expected_path, &expected.jam)?;
                    std::fs::write(
                        output_dir.join(format!("{node}.expected.hex")),
                        hex::encode(&expected.jam),
                    )?;
                    let mut actual_slab: NounSlab = NounSlab::new();
                    let actual_noun = actual_slab
                        .cue_into(Bytes::from(actual.clone()))
                        .map_err(|err| anyhow!("failed to cue constants for '{node}': {err}"))?;
                    let actual_space = actual_slab.noun_space();
                    let Some(actual_constants) =
                        decode_optional_optional_noun(actual_noun.in_space(&actual_space))?
                    else {
                        return Err(anyhow!("constants peek returned none for '{node}'"));
                    };
                    let mut actual_constants_slab: NounSlab = NounSlab::new();
                    actual_constants_slab
                        .copy_into(actual_constants.noun(), actual_constants.space());
                    if !slab_equality(&actual_constants_slab, &expected.slab) {
                        return Err(anyhow!(
                            "constants mismatch for '{}': actual {} expected {}",
                            node,
                            actual_path.display(),
                            expected_path.display()
                        ));
                    }
                }
            }
            Action::Sleep { millis } => {
                sleep(Duration::from_millis(*millis)).await;
            }
            Action::SubmitTx {
                node,
                fixture,
                wallet,
                expect,
                tx_id_override,
                store_as,
            } => {
                let fixture_name = expand_vars(fixture, state)?;
                let fixture_path =
                    resolve_submit_tx_path(&options.scenario_path, run_dir, &fixture_name, wallet)?;
                let raw_tx = load_raw_tx_fixture(&fixture_path)?;
                let override_hash = match tx_id_override {
                    Some(raw) => {
                        let raw = expand_vars(raw, state)?;
                        Some(
                            Hash::from_base58(&raw)
                                .map_err(|err| anyhow!("invalid tx_id_override '{raw}': {err}"))?,
                        )
                    }
                    None => None,
                };
                let addr = node_manager.grpc_public_addr(node)?;
                let outcome = submit_raw_tx(addr, raw_tx.clone(), override_hash).await?;
                assert_submit_outcome(node, expect, &outcome)?;
                state.record_tx(store_as.as_deref(), raw_tx.id.clone());
            }
            Action::InjectBlock { node, fixture } => {
                let fixture_path = resolve_fixture_path(&options.scenario_path, fixture);
                let payload = build_heard_block_payload(&fixture_path)?;
                let addr = node_manager.grpc_private_addr(node)?;
                poke_private(addr, payload).await?;
            }
            Action::SetMiningPkh { node, value } => {
                let value = expand_vars(value, state)?;
                node_manager.set_mining_pkh(node, value).await?;
            }
            Action::DisableMining { node } => {
                node_manager.disable_mining(node).await?;
            }
            Action::SetMiningEnabled { node, enabled } => {
                let addr = node_manager.grpc_private_addr(node)?;
                set_mining_enabled(addr, *enabled).await?;
            }
            Action::SetNodeEnv { node, key, value } => {
                let key = expand_vars(key, state)?;
                let value = expand_vars(value, state)?;
                node_manager.set_node_env(node, key, value)?;
            }
            Action::Partition { groups } => {
                node_manager.apply_partition(groups, run_dir).await?;
            }
            Action::Upgrade { node, version } => {
                let binary = resolve_binary_override(scenario, options, version)?;
                node_manager.upgrade_node(node, binary).await?;
            }
            Action::Tick { millis } => {
                sleep(Duration::from_millis(*millis)).await;
            }
            Action::Wallet {
                wallet,
                node,
                command,
                args,
                expect,
                expect_exit_code,
                capture,
            } => {
                let resolved_args = expand_args(args, state)?;
                let resolved_expect = match expect {
                    Some(value) => Some(expand_vars(value, state)?),
                    None => None,
                };
                let retryable = wallet_command_is_retryable(command)
                    && (resolved_expect.is_some() || capture.is_some());
                let mut attempt = 0usize;
                loop {
                    let output = run_wallet_command(
                        options, node_manager, run_dir, step_index, wallet, node, command,
                        &resolved_args, *expect_exit_code,
                    )
                    .await?;
                    let context = WalletCommandContext {
                        run_dir,
                        wallet,
                        command,
                        args: &resolved_args,
                    };
                    let validation = validate_wallet_output(
                        &context,
                        resolved_expect.as_deref(),
                        capture.as_ref(),
                        &output,
                        state,
                    );
                    match validation {
                        Ok(()) => break,
                        Err(err) if retryable && attempt + 1 < WALLET_OUTPUT_RETRY_ATTEMPTS => {
                            attempt += 1;
                            sleep(WALLET_OUTPUT_RETRY_DELAY).await;
                        }
                        Err(err) => return Err(err),
                    }
                }
            }
            Action::CloneWallet { from, to } => {
                let from = expand_vars(from, state)?;
                let to = expand_vars(to, state)?;
                let source_dir = run_dir.join("wallets").join(&from);
                let dest_dir = run_dir.join("wallets").join(&to);
                copy_dir_recursive(&source_dir, &dest_dir).map_err(|err| {
                    anyhow!("failed to clone wallet '{}' to '{}': {}", from, to, err)
                })?;
            }
            Action::Command {
                command,
                args,
                env,
                cwd,
                expect,
                expect_exit_code,
            } => {
                let command = expand_vars(command, state)?;
                let args = expand_args(args, state)?;
                let env = expand_command_env(env, state)?;
                let cwd = resolve_command_cwd(run_dir, cwd.as_ref(), state)?;
                let expect = expect
                    .as_deref()
                    .map(|value| expand_vars(value, state))
                    .transpose()?;
                run_command_action(
                    run_dir,
                    step_index,
                    &command,
                    &args,
                    &env,
                    &cwd,
                    expect.as_deref(),
                    *expect_exit_code,
                    state,
                )
                .await?;
            }
        }
        report.record_step(step_index, &action_name, timer.elapsed_ms(), true);
    }

    for assert in &scenario.asserts {
        let assert_name = assert_type_name(assert);
        let result: Result<()> = async {
            match assert {
                Assert::GrpcReady { node } => {
                    node_manager
                        .wait_for_grpc(node, Duration::from_secs(5))
                        .await?;
                }
                Assert::HeadsEqual { nodes, timeout_ms } => {
                    if let Some(timeout_ms) = timeout_ms {
                        wait_for_heads_equal(
                            node_manager,
                            nodes,
                            Duration::from_millis(*timeout_ms),
                        )
                        .await?;
                    } else {
                        assert_heads_equal(node_manager, nodes).await?;
                    }
                }
                Assert::HeadsNotEqual { nodes } => {
                    assert_heads_not_equal(node_manager, nodes).await?;
                }
                Assert::HeightAtLeast { node, height } => {
                    let head = node_manager.fetch_head(node).await?;
                    if head.height < *height {
                        return Err(anyhow!(
                            "node '{}' height {} below expected {}", node, head.height, height
                        ));
                    }
                }
                Assert::TxAccepted { node, tx } => {
                    let tx_value = expand_vars(tx, state)?;
                    let tx_id = resolve_tx_id(state, &tx_value)?;
                    let addr = node_manager.grpc_public_addr(node)?;
                    let accepted = transaction_accepted(addr, &tx_id).await?;
                    if !accepted {
                        return Err(anyhow!("tx '{}' was not accepted on node '{}'", tx, node));
                    }
                }
                Assert::TxInBlock { node, tx } => {
                    let tx_value = expand_vars(tx, state)?;
                    let tx_id = resolve_tx_id(state, &tx_value)?;
                    let addr = node_manager.grpc_public_addr(node)?;
                    if wait_for_tx_in_block(addr, &tx_id, Duration::ZERO)
                        .await?
                        .is_none()
                    {
                        return Err(anyhow!(
                            "tx '{}' was not observed in a block on node '{}'", tx, node
                        ));
                    }
                }
                Assert::TxNotAccepted { node, tx } => {
                    let tx_value = expand_vars(tx, state)?;
                    let tx_id = resolve_tx_id(state, &tx_value)?;
                    let addr = node_manager.grpc_public_addr(node)?;
                    let accepted = transaction_accepted(addr, &tx_id).await?;
                    if accepted {
                        return Err(anyhow!(
                            "tx '{}' unexpectedly accepted on node '{}'", tx, node
                        ));
                    }
                }
                Assert::ReqResGeneration {
                    node,
                    peer,
                    generation,
                    timeout_ms,
                } => {
                    if let Some(timeout_ms) = timeout_ms {
                        wait_for_req_res_generation(
                            node_manager,
                            node,
                            peer,
                            *generation,
                            Duration::from_millis(*timeout_ms),
                        )
                        .await?;
                    } else {
                        assert_req_res_generation(node_manager, node, peer, *generation).await?;
                    }
                }
            }
            Ok(())
        }
        .await;
        let ok = result.is_ok();
        let detail = result.as_ref().err().map(|e| e.to_string());
        report.record_assert(&assert_name, ok, detail);
        result?;
    }

    Ok(())
}

fn step_action_name(action: &Action) -> String {
    match action {
        Action::StartNodes { ids } => format!("start_nodes:{}", ids.join(",")),
        Action::StopNodes { ids } => format!("stop_nodes:{}", ids.join(",")),
        Action::WaitForGrpc { node, .. } => format!("wait_for_grpc:{node}"),
        Action::WaitForHeight { node, height, .. } => format!("wait_for_height:{node}@{height}"),
        Action::WaitForHeadsEqual { nodes, .. } => {
            format!("wait_for_heads_equal:{}", nodes.join(","))
        }
        Action::WaitForTxAccepted { node, tx, .. } => format!("wait_for_tx_accepted:{node}:{tx}"),
        Action::WaitForTxInBlock { node, tx, .. } => format!("wait_for_tx_in_block:{node}:{tx}"),
        Action::PeekConstants { node } => format!("peek_constants:{node}"),
        Action::Sleep { millis } => format!("sleep:{millis}ms"),
        Action::SubmitTx { node, fixture, .. } => format!("submit_tx:{node}:{fixture}"),
        Action::InjectBlock { node, fixture } => format!("inject_block:{node}:{fixture}"),
        Action::SetMiningPkh { node, .. } => format!("set_mining_pkh:{node}"),
        Action::DisableMining { node } => format!("disable_mining:{node}"),
        Action::SetMiningEnabled { node, enabled } => {
            format!("set_mining_enabled:{node}:{enabled}")
        }
        Action::SetNodeEnv { node, key, .. } => format!("set_node_env:{node}:{key}"),
        Action::Partition { groups } => format!("partition:{} groups", groups.len()),
        Action::Upgrade { node, version } => format!("upgrade:{node}:{version}"),
        Action::Tick { millis } => format!("tick:{millis}ms"),
        Action::Wallet {
            wallet, command, ..
        } => format!("wallet:{wallet}:{command}"),
        Action::CloneWallet { from, to } => format!("clone_wallet:{from}->{to}"),
        Action::Command { command, .. } => format!("command:{command}"),
    }
}

fn assert_type_name(assert: &Assert) -> String {
    match assert {
        Assert::GrpcReady { node } => format!("grpc_ready:{node}"),
        Assert::HeadsEqual { nodes, .. } => format!("heads_equal:{}", nodes.join(",")),
        Assert::HeadsNotEqual { nodes } => format!("heads_not_equal:{}", nodes.join(",")),
        Assert::HeightAtLeast { node, height } => format!("height_at_least:{node}@{height}"),
        Assert::TxAccepted { node, tx } => format!("tx_accepted:{node}:{tx}"),
        Assert::TxInBlock { node, tx } => format!("tx_in_block:{node}:{tx}"),
        Assert::TxNotAccepted { node, tx } => format!("tx_not_accepted:{node}:{tx}"),
        Assert::ReqResGeneration {
            node,
            peer,
            generation,
            ..
        } => {
            format!(
                "req_res_generation:{node}->{peer}:{}",
                req_res_generation_label(*generation)
            )
        }
    }
}

fn seed_run_state(state: &mut RunState, run_dir: &Path, scenario_path: &Path) -> Result<()> {
    let repo_root = std::env::current_dir().context("resolve repo root cwd")?;
    let scenario_path = if scenario_path.is_absolute() {
        scenario_path.to_path_buf()
    } else {
        repo_root.join(scenario_path)
    };
    let scenario_dir = scenario_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| repo_root.clone());

    state.set_var("RUN_DIR", run_dir.display().to_string());
    state.set_var("REPO_ROOT", repo_root.display().to_string());
    state.set_var("SCENARIO_PATH", scenario_path.display().to_string());
    state.set_var("SCENARIO_DIR", scenario_dir.display().to_string());
    Ok(())
}

async fn assert_heads_equal(node_manager: &NodeManager, nodes: &[String]) -> Result<()> {
    if nodes.len() < 2 {
        return Err(anyhow!("heads_equal requires at least two nodes"));
    }

    let mut heads = Vec::new();
    for node in nodes {
        let head = node_manager.fetch_private_head(node).await?;
        heads.push((node, head));
    }

    let (first_id, first_head) = &heads[0];
    for (node_id, head) in &heads[1..] {
        if head.height != first_head.height || head.block_id != first_head.block_id {
            return Err(anyhow!(
                "head mismatch: {}={:?} {}={:?}", first_id, first_head, node_id, head
            ));
        }
    }
    Ok(())
}

async fn assert_req_res_generation(
    node_manager: &NodeManager,
    node: &str,
    peer: &str,
    generation: ReqResGenerationExpectation,
) -> Result<()> {
    let peer_id = node_manager.wait_for_peer_id(peer).await?;
    let logs = node_manager.combined_logs(node).await?;

    if req_res_generation_logged(&logs, &peer_id, generation) {
        return Ok(());
    }

    Err(anyhow!(
        "node '{}' did not log a completed req-res exchange with peer '{}' ({}) using {}. \
looked for '{}' with peer={} and generation={}. recent log tail:\n{}",
        node,
        peer,
        peer_id,
        req_res_generation_label(generation),
        REQ_RES_EXCHANGE_COMPLETED_MARKER,
        peer_id,
        req_res_generation_log_value(generation),
        log_tail(&logs, 12)
    ))
}

async fn wait_for_req_res_generation(
    node_manager: &NodeManager,
    node: &str,
    peer: &str,
    generation: ReqResGenerationExpectation,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match assert_req_res_generation(node_manager, node, peer, generation).await {
            Ok(()) => return Ok(()),
            Err(err) => {
                if Instant::now() >= deadline {
                    return Err(err.context(format!(
                        "timed out waiting for req-res generation {} from '{}' to '{}'",
                        req_res_generation_label(generation),
                        node,
                        peer
                    )));
                }
            }
        }
        sleep(Duration::from_millis(200)).await;
    }
}

async fn wait_for_heads_equal(
    node_manager: &NodeManager,
    nodes: &[String],
    timeout: Duration,
) -> Result<()> {
    if nodes.len() < 2 {
        return Err(anyhow!("heads_equal requires at least two nodes"));
    }

    let deadline = Instant::now() + timeout;
    loop {
        match assert_heads_equal(node_manager, nodes).await {
            Ok(()) => return Ok(()),
            Err(err) => {
                if Instant::now() >= deadline {
                    return Err(
                        err.context(format!("timed out waiting for heads_equal on {:?}", nodes))
                    );
                }
            }
        }
        sleep(Duration::from_millis(200)).await;
    }
}

const REQ_RES_EXCHANGE_COMPLETED_MARKER: &str = "Nous req-res exchange completed";

fn req_res_generation_logged(
    logs: &str,
    peer_id: &str,
    generation: ReqResGenerationExpectation,
) -> bool {
    let generation = req_res_generation_log_value(generation);
    logs.lines().any(|line| {
        line.contains(REQ_RES_EXCHANGE_COMPLETED_MARKER)
            && line.contains(&format!("peer={peer_id}"))
            && line.contains(&format!("generation={generation}"))
    })
}

fn req_res_generation_label(generation: ReqResGenerationExpectation) -> &'static str {
    match generation {
        ReqResGenerationExpectation::Gen1 => "gen1",
        ReqResGenerationExpectation::Gen2 => "gen2",
    }
}

fn req_res_generation_log_value(generation: ReqResGenerationExpectation) -> &'static str {
    match generation {
        ReqResGenerationExpectation::Gen1 => "Gen1",
        ReqResGenerationExpectation::Gen2 => "Gen2",
    }
}

fn log_tail(logs: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = logs
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return "<no logs captured>".to_string();
    }
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

async fn assert_heads_not_equal(node_manager: &NodeManager, nodes: &[String]) -> Result<()> {
    if nodes.len() < 2 {
        return Err(anyhow!("heads_not_equal requires at least two nodes"));
    }

    let mut heads = Vec::new();
    for node in nodes {
        let head = node_manager.fetch_private_head(node).await?;
        heads.push((node, head));
    }

    let (first_id, first_head) = &heads[0];
    for (_node_id, head) in &heads[1..] {
        if head.height != first_head.height || head.block_id != first_head.block_id {
            return Ok(());
        }
    }

    Err(anyhow!(
        "heads_not_equal failed: all nodes matched {}={:?}", first_id, first_head
    ))
}

async fn wait_for_tx_accepted(
    node_manager: &NodeManager,
    node: &str,
    tx_id: &Hash,
    timeout: Duration,
) -> Result<()> {
    let addr = node_manager.grpc_public_addr(node)?;
    let deadline = Instant::now() + timeout;
    loop {
        let accepted = transaction_accepted(addr, tx_id).await?;
        if accepted {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out waiting for tx '{}' to be accepted on node '{}'",
                tx_id.to_base58(),
                node
            ));
        }
        sleep(Duration::from_millis(200)).await;
    }
}

fn resolve_tx_id(state: &RunState, tx: &str) -> Result<Hash> {
    if tx == "last" {
        return state
            .last_tx
            .clone()
            .ok_or_else(|| anyhow!("no last tx recorded"));
    }
    if let Some(id) = state.tx_ids.get(tx) {
        return Ok(id.clone());
    }
    Hash::from_base58(tx).map_err(|err| anyhow!("invalid tx id '{}': {err}", tx))
}

fn resolve_binary_override(
    scenario: &Scenario,
    options: &RunOptions,
    version: &str,
) -> Result<PathBuf> {
    if let Some(path) = scenario.binaries.get(version) {
        let expanded = expand_env_vars(&path.to_string_lossy())?;
        return Ok(PathBuf::from(expanded));
    }
    if options.docker && version == "current" {
        if let Some(image) = &options.docker_image {
            return Ok(PathBuf::from(image));
        }
    }
    if version == "current" {
        if let Some(path) = &options.nockchain_bin {
            return Ok(path.clone());
        }
    }
    let expanded = expand_env_vars(version)?;
    Ok(PathBuf::from(expanded))
}

fn resolve_fixture_path(scenario_path: &Path, fixture: &str) -> PathBuf {
    let fixture_path = PathBuf::from(fixture);
    if fixture_path.is_absolute() {
        return fixture_path;
    }
    let scenario_dir = scenario_path.parent().unwrap_or_else(|| Path::new("."));
    let fixtures_dir = scenario_dir.join("..").join("fixtures");
    let candidate = fixtures_dir.join(fixture);
    if candidate.exists() {
        candidate
    } else {
        scenario_dir.join(fixture)
    }
}

fn resolve_submit_tx_path(
    scenario_path: &Path,
    run_dir: &Path,
    fixture: &str,
    wallet: &Option<String>,
) -> Result<PathBuf> {
    let base_path = resolve_fixture_path(scenario_path, fixture);
    if base_path.exists() {
        return Ok(base_path);
    }

    let Some(wallet_name) = wallet else {
        return Ok(base_path);
    };

    let file_name = Path::new(fixture)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if stem.is_empty() {
        return Ok(base_path);
    }

    let raw_tx_path = run_dir
        .join("wallets")
        .join(wallet_name)
        .join("txs-debug")
        .join(format!("{stem}.jam"));
    if raw_tx_path.exists() {
        Ok(raw_tx_path)
    } else {
        Ok(base_path)
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        std::fs::remove_dir_all(dst).with_context(|| {
            format!(
                "remove existing destination before clone: {}",
                dst.display()
            )
        })?;
    }
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn expand_command_env(
    env: &BTreeMap<String, String>,
    state: &RunState,
) -> Result<BTreeMap<String, String>> {
    let mut resolved = BTreeMap::new();
    for (key, value) in env {
        resolved.insert(key.clone(), expand_vars(value, state)?);
    }
    Ok(resolved)
}

fn resolve_command_cwd(run_dir: &Path, cwd: Option<&PathBuf>, state: &RunState) -> Result<PathBuf> {
    let Some(cwd) = cwd else {
        return Ok(run_dir.to_path_buf());
    };

    let expanded = expand_vars(&cwd.to_string_lossy(), state)?;
    let cwd = PathBuf::from(expanded);
    if cwd.is_absolute() {
        Ok(cwd)
    } else {
        Ok(run_dir.join(cwd))
    }
}

fn expand_env_vars(input: &str) -> Result<String> {
    expand_template(input, |key| std::env::var(key).ok())
}

fn expand_vars(input: &str, state: &RunState) -> Result<String> {
    expand_template(input, |key| {
        state
            .vars
            .get(key)
            .cloned()
            .or_else(|| std::env::var(key).ok())
    })
}

fn expand_args(args: &[String], state: &RunState) -> Result<Vec<String>> {
    let mut resolved = Vec::with_capacity(args.len());
    for arg in args {
        resolved.push(expand_vars(arg, state)?);
    }
    Ok(resolved)
}

#[allow(clippy::while_let_on_iterator)]
fn expand_template<F>(input: &str, resolver: F) -> Result<String>
where
    F: Fn(&str) -> Option<String>,
{
    if !input.contains("${") {
        return Ok(input.to_string());
    }

    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$' && matches!(chars.peek(), Some('{')) {
            chars.next();
            let mut key = String::new();
            let mut closed = false;
            while let Some(next) = chars.next() {
                if next == '}' {
                    closed = true;
                    break;
                }
                key.push(next);
            }
            if !closed {
                return Err(anyhow!("unterminated variable reference in '{}'", input));
            }
            if key.is_empty() {
                return Err(anyhow!("empty variable reference in '{}'", input));
            }
            let value = resolver(&key)
                .ok_or_else(|| anyhow!("missing variable '{}' in '{}'", key, input))?;
            out.push_str(&value);
        } else {
            out.push(ch);
        }
    }
    Ok(out)
}

fn load_raw_tx_fixture(path: &Path) -> Result<RawTx> {
    let data = std::fs::read(path)
        .map_err(|err| anyhow!("failed to read tx fixture {}: {err}", path.display()))?;
    let mut slab: NounSlab = NounSlab::new();
    let noun = slab
        .cue_into(Bytes::from(data))
        .map_err(|err| anyhow!("failed to cue tx fixture {}: {err}", path.display()))?;
    let space = slab.noun_space();
    RawTx::from_noun(&noun, &space)
        .map_err(|err| anyhow!("failed to decode raw-tx fixture {}: {err}", path.display()))
}

fn build_heard_block_payload(path: &Path) -> Result<Vec<u8>> {
    let data = std::fs::read(path)
        .map_err(|err| anyhow!("failed to read block fixture {}: {err}", path.display()))?;
    let mut page_slab: NounSlab = NounSlab::new();
    let page_noun = page_slab
        .cue_into(Bytes::from(data))
        .map_err(|err| anyhow!("failed to cue block fixture {}: {err}", path.display()))?;
    let page_space = page_slab.noun_space();

    let mut payload_slab: NounSlab = NounSlab::new();
    let page_copy = payload_slab.copy_into(page_noun, &page_space);
    let heard = nockapp::utils::make_tas(&mut payload_slab, "heard-block").as_noun();
    let fact = nockapp::utils::make_tas(&mut payload_slab, "fact").as_noun();
    let heard_cell = nockvm::noun::T(&mut payload_slab, &[heard, page_copy]);
    let payload = nockvm::noun::T(&mut payload_slab, &[fact, nockvm::noun::D(0), heard_cell]);
    payload_slab.set_root(payload);
    Ok(payload_slab.jam().to_vec())
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

fn assert_submit_outcome(
    node: &str,
    expect: &Option<SubmitTxExpect>,
    outcome: &SubmitTxOutcome,
) -> Result<()> {
    let expect = expect.clone().unwrap_or(SubmitTxExpect::Ack);
    match expect {
        SubmitTxExpect::Ack => {
            if outcome.acknowledged {
                Ok(())
            } else {
                Err(anyhow!(
                    "tx submission on '{}' failed: {}",
                    node,
                    outcome
                        .error
                        .clone()
                        .unwrap_or_else(|| "unknown error".to_string())
                ))
            }
        }
        SubmitTxExpect::Error => {
            if outcome.acknowledged {
                Err(anyhow!(
                    "tx submission on '{}' unexpectedly acknowledged", node
                ))
            } else {
                Ok(())
            }
        }
    }
}

struct WalletCommandOutput {
    stdout: String,
    stderr: String,
}

struct CommandActionOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

struct WalletCommandContext<'a> {
    run_dir: &'a Path,
    wallet: &'a str,
    command: &'a str,
    args: &'a [String],
}

const WALLET_OUTPUT_RETRY_ATTEMPTS: usize = 10;
const WALLET_OUTPUT_RETRY_DELAY: Duration = Duration::from_millis(500);

fn wallet_command_is_retryable(command: &str) -> bool {
    command.starts_with("list-")
}

fn validate_wallet_output(
    context: &WalletCommandContext<'_>,
    expected: Option<&str>,
    capture: Option<&nockchain_testkit::scenario::WalletCapture>,
    output: &WalletCommandOutput,
    state: &mut RunState,
) -> Result<()> {
    if let Some(expected) = expected {
        if !wallet_output_contains(output, expected) {
            return Err(anyhow!(
                "wallet '{}' command '{}' missing expected output '{}'", context.wallet,
                context.command, expected
            ));
        }
    }
    if let Some(capture) = capture {
        capture_wallet_output(context, capture, output, state)?;
    }
    Ok(())
}

fn wallet_output_contains(output: &WalletCommandOutput, needle: &str) -> bool {
    let needle = strip_ansi(needle);
    wallet_output_variants(output, WalletCaptureSource::Stdout)
        .into_iter()
        .any(|candidate| candidate.contains(&needle))
}

fn capture_wallet_output(
    context: &WalletCommandContext<'_>,
    capture: &nockchain_testkit::scenario::WalletCapture,
    output: &WalletCommandOutput,
    state: &mut RunState,
) -> Result<()> {
    let variants = wallet_output_variants(output, capture.source.clone());
    if context.command == "send-tx" {
        for sanitized in &variants {
            if let Some(value) = extract_tx_id(sanitized) {
                state.set_var(&capture.store_as, value);
                return Ok(());
            }
        }
        if let Some(value) =
            extract_tx_id_from_saved_raw_tx(context.run_dir, context.wallet, context.args)?
        {
            state.set_var(&capture.store_as, value);
            return Ok(());
        }
    }
    if context.command == "keygen" {
        for sanitized in &variants {
            if let Some(value) = extract_wallet_address(sanitized)? {
                state.set_var(&capture.store_as, value);
                return Ok(());
            }
        }
    }
    if context.command == "create-tx" {
        for sanitized in &variants {
            if let Some(value) = extract_saved_tx_path(sanitized) {
                state.set_var(&capture.store_as, value);
                return Ok(());
            }
        }
    }
    if context.command == "list-notes" {
        for sanitized in &variants {
            if let Some(value) = extract_oldest_note_name(sanitized)? {
                state.set_var(&capture.store_as, value);
                return Ok(());
            }
        }
    }

    let regex = Regex::new(&capture.regex).map_err(|err| {
        anyhow!(
            "wallet '{}' command '{}' invalid capture regex '{}': {}", context.wallet,
            context.command, capture.regex, err
        )
    })?;
    for sanitized in &variants {
        if let Some(caps) = regex.captures(sanitized) {
            let value = caps
                .get(1)
                .or_else(|| caps.get(0))
                .map(|m| m.as_str().trim())
                .unwrap_or_default();
            let value = normalize_wallet_capture_value(value);
            if value.is_empty() {
                return Err(anyhow!(
                    "wallet '{}' command '{}' capture regex '{}' produced empty value",
                    context.wallet, context.command, capture.regex
                ));
            }
            state.set_var(&capture.store_as, value);
            return Ok(());
        }
    }
    let preview: String = variants
        .first()
        .map(|value| value.chars().take(400).collect())
        .unwrap_or_default();
    Err(anyhow!(
        "wallet '{}' command '{}' capture regex '{}' did not match (output preview: {})",
        context.wallet, context.command, capture.regex, preview
    ))
}

fn extract_oldest_note_name(output: &str) -> Result<Option<String>> {
    let regex = Regex::new(r"(?s)- Name:\s*(\[[^\]]+\]).*?- Block Height:\s*([0-9]+)")
        .with_context(|| "invalid wallet note capture regex")?;
    let mut selected: Option<(u64, String)> = None;
    for caps in regex.captures_iter(output) {
        let Some(raw_name) = caps.get(1).map(|m| m.as_str()) else {
            continue;
        };
        let Some(raw_height) = caps.get(2).map(|m| m.as_str()) else {
            continue;
        };
        let height = raw_height
            .parse::<u64>()
            .with_context(|| format!("invalid wallet note block height '{raw_height}'"))?;
        let name = normalize_wallet_capture_value(raw_name);
        if name.is_empty() {
            continue;
        }
        match &selected {
            Some((selected_height, _)) if *selected_height <= height => {}
            _ => selected = Some((height, name)),
        }
    }
    Ok(selected.map(|(_, name)| name))
}

fn normalize_wallet_capture_value(value: &str) -> String {
    let normalized = value.replace(['\n', '\r'], "");
    if normalized.is_empty() {
        value.to_string()
    } else {
        normalized
    }
}

fn extract_tx_id_from_saved_raw_tx(
    run_dir: &Path,
    wallet: &str,
    args: &[String],
) -> Result<Option<String>> {
    let Some(tx_arg) = args.first() else {
        return Ok(None);
    };

    let wallet_dir = run_dir.join("wallets").join(wallet);
    let tx_path = resolve_wallet_tx_path(&wallet_dir, tx_arg);
    let Some(raw_tx_path) = tx_path_to_raw_tx_path(&tx_path) else {
        return Ok(None);
    };
    if !raw_tx_path.exists() {
        return Ok(None);
    }

    let raw_tx = load_raw_tx_fixture(&raw_tx_path)?;
    Ok(Some(raw_tx.id.to_base58()))
}

fn resolve_wallet_tx_path(wallet_dir: &Path, tx_arg: &str) -> PathBuf {
    let path = PathBuf::from(tx_arg);
    if path.is_absolute() {
        path
    } else {
        wallet_dir.join(path)
    }
}

fn tx_path_to_raw_tx_path(tx_path: &Path) -> Option<PathBuf> {
    let txs_dir = tx_path.parent()?;
    if txs_dir.file_name()?.to_str()? != "txs" {
        return None;
    }
    let tx_name = tx_path.file_stem()?.to_str()?;
    Some(
        txs_dir
            .parent()?
            .join("txs-debug")
            .join(format!("{tx_name}.jam")),
    )
}

fn wallet_output_variants(
    output: &WalletCommandOutput,
    preferred_source: WalletCaptureSource,
) -> Vec<String> {
    let primary = match preferred_source {
        WalletCaptureSource::Stdout => &output.stdout,
        WalletCaptureSource::Stderr => &output.stderr,
    };
    let secondary = match preferred_source {
        WalletCaptureSource::Stdout => &output.stderr,
        WalletCaptureSource::Stderr => &output.stdout,
    };
    vec![
        strip_ansi(primary),
        strip_ansi(secondary),
        strip_ansi(&format!("{}\n{}", output.stdout, output.stderr)),
    ]
}

fn strip_ansi(input: &str) -> String {
    let regex = Regex::new(r"\x1b\[[0-9;]*m").expect("valid ansi regex");
    let cleaned = regex.replace_all(input, "").to_string();
    cleaned
        .chars()
        .filter(|ch| {
            if matches!(*ch, '\n' | '\r' | '\t') {
                return true;
            }
            !ch.is_control()
        })
        .collect()
}

fn extract_tx_id(output: &str) -> Option<String> {
    let sanitized = strip_ansi(output);
    let regex = Regex::new(r"(?s)Validation for TX\s*([A-Za-z0-9\s]+?)\s+passed").ok()?;
    let caps = regex.captures(&sanitized)?;
    let id: String = caps
        .get(1)?
        .as_str()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect();
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

fn extract_wallet_address(output: &str) -> Result<Option<String>> {
    let sanitized = strip_ansi(output);
    let lines: Vec<&str> = sanitized.lines().collect();
    if let Some(address) = extract_wrapped_address_after_label(&lines, "### Address")
        .or_else(|| extract_wrapped_address_after_label(&lines, "Address"))
        .or_else(|| extract_wrapped_address_after_active_master(&lines))
    {
        return Ok(Some(address));
    }

    let regex = Regex::new(r"(?m)^\s*- Address:\s*([A-Za-z0-9]+)\s*\r?$")
        .with_context(|| "invalid wallet address regex '- Address:'")?;
    if let Some(caps) = regex.captures(&sanitized) {
        let address = caps
            .get(1)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default();
        if !address.is_empty() {
            return Ok(Some(address));
        }
    }
    Ok(None)
}

fn extract_saved_tx_path(output: &str) -> Option<String> {
    let sanitized = strip_ansi(output);
    let lines: Vec<&str> = sanitized.lines().collect();
    lines.iter().enumerate().find_map(|(index, line)| {
        let trimmed = line.trim();
        let remainder = trimmed
            .strip_prefix("- Saved transaction to")
            .or_else(|| trimmed.strip_prefix("Saved transaction to"))?;
        let inline = path_chunk(remainder);
        let wrapped = collect_wrapped_path(lines.as_slice(), index + 1);
        match (inline, wrapped) {
            (Some(inline), Some(wrapped)) => Some(format!("{inline}{wrapped}")),
            (Some(inline), None) => Some(inline.to_string()),
            (None, Some(wrapped)) => Some(wrapped),
            (None, None) => None,
        }
    })
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

fn collect_wrapped_path(lines: &[&str], start: usize) -> Option<String> {
    let mut path = String::new();
    let mut started = false;
    for line in &lines[start..] {
        match path_chunk(line) {
            Some(chunk) => {
                started = true;
                path.push_str(chunk);
            }
            None if started => break,
            None => continue,
        }
    }
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

fn path_chunk(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '/'))
    {
        return None;
    }
    Some(trimmed)
}

fn resolve_command_binary(command: &str, state: &RunState) -> Result<PathBuf> {
    let binary = PathBuf::from(command);
    if binary.is_absolute() {
        return Ok(binary);
    }
    if command.contains('/') {
        let repo_root = state
            .vars
            .get("REPO_ROOT")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("missing REPO_ROOT for command '{}'", command))?;
        return Ok(repo_root.join(binary));
    }
    Ok(binary)
}

fn command_output_contains(output: &CommandActionOutput, needle: &str) -> bool {
    let needle = strip_ansi(needle);
    let stdout = strip_ansi(&output.stdout);
    let stderr = strip_ansi(&output.stderr);
    stdout.contains(&needle)
        || stderr.contains(&needle)
        || format!("{stdout}\n{stderr}").contains(&needle)
}

#[allow(clippy::too_many_arguments)]
async fn run_command_action(
    run_dir: &Path,
    step_index: usize,
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: &Path,
    expect: Option<&str>,
    expect_exit_code: Option<i32>,
    state: &RunState,
) -> Result<CommandActionOutput> {
    std::fs::create_dir_all(cwd)?;

    let binary = resolve_command_binary(command, state)?;
    if is_explicit_path(&binary) && !binary.exists() {
        return Err(anyhow!("command binary not found at {}", binary.display()));
    }

    let mut cmd = Command::new(&binary);
    cmd.current_dir(cwd).args(args);
    for (key, value) in env {
        cmd.env(key, value);
    }

    let output = cmd
        .output()
        .await
        .with_context(|| format!("failed to run command '{}'", command))?;
    let exit_code = output
        .status
        .code()
        .ok_or_else(|| anyhow!("command '{}' terminated by signal", command))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let output = CommandActionOutput {
        stdout,
        stderr,
        exit_code,
    };

    let logs_dir = run_dir.join("commands");
    std::fs::create_dir_all(&logs_dir)?;
    let label = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .map(sanitize_name)
        .unwrap_or_else(|| sanitize_name(command));
    let stdout_path = logs_dir.join(format!("step-{step_index:03}-{label}.stdout.log"));
    let stderr_path = logs_dir.join(format!("step-{step_index:03}-{label}.stderr.log"));
    std::fs::write(&stdout_path, &output.stdout)?;
    std::fs::write(&stderr_path, &output.stderr)?;

    let expected_code = expect_exit_code.unwrap_or(0);
    if output.exit_code != expected_code {
        return Err(anyhow!(
            "command '{}' exited with {} (expected {}): {}",
            command,
            output.exit_code,
            expected_code,
            output.stderr.trim()
        ));
    }

    if let Some(expect) = expect {
        if !command_output_contains(&output, expect) {
            return Err(anyhow!(
                "command '{}' missing expected output '{}' (stdout: {}, stderr: {})",
                command,
                expect,
                output.stdout.trim(),
                output.stderr.trim()
            ));
        }
    }

    Ok(output)
}

#[allow(clippy::too_many_arguments)]
async fn run_wallet_command(
    options: &RunOptions,
    node_manager: &NodeManager,
    run_dir: &Path,
    step_index: usize,
    wallet: &str,
    node: &str,
    command: &str,
    args: &[String],
    expect_exit_code: Option<i32>,
) -> Result<WalletCommandOutput> {
    let wallet_bin = resolve_wallet_bin(options)?;
    if is_explicit_path(&wallet_bin) && !wallet_bin.exists() {
        return Err(anyhow!(
            "wallet binary not found at {}",
            wallet_bin.display()
        ));
    }
    // Canonicalize the wallet binary path so it works regardless of the command's current_dir
    let wallet_bin = if wallet_bin.exists() {
        wallet_bin.canonicalize()?
    } else {
        wallet_bin
    };

    let wallet_dir = run_dir.join("wallets").join(wallet);
    std::fs::create_dir_all(&wallet_dir)?;
    // Canonicalize wallet_dir for NOCKAPP_HOME to avoid path doubling
    // (system_data_dir joins relative NOCKAPP_HOME with current_dir)
    let wallet_dir_canonical = wallet_dir.canonicalize()?;
    let port = node_manager.grpc_private_port(node)?;
    let is_fakenet = node_manager.is_fakenet(node)?;
    let (fakenet_v1_phase, fakenet_bythos_phase) = node_manager.fakenet_phase_overrides(node)?;

    let mut cmd = Command::new(&wallet_bin);
    cmd.current_dir(&wallet_dir)
        .env("NOCKAPP_HOME", &wallet_dir_canonical)
        .arg("--client")
        .arg("private")
        .arg("--private-grpc-server-port")
        .arg(port.to_string());
    if is_fakenet {
        cmd.arg("--fakenet");
        if let Some(phase) = fakenet_v1_phase {
            cmd.arg("--fakenet-v1-phase").arg(phase.to_string());
        }
        if let Some(phase) = fakenet_bythos_phase {
            cmd.arg("--fakenet-bythos-phase").arg(phase.to_string());
        }
    }
    cmd.arg(command).args(args);

    let output = cmd.output().await?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    let label = sanitize_name(command);
    let stdout_path = wallet_dir.join(format!("step-{step_index:03}-{label}.stdout.log"));
    let stderr_path = wallet_dir.join(format!("step-{step_index:03}-{label}.stderr.log"));
    std::fs::write(stdout_path, &stdout)?;
    std::fs::write(stderr_path, &stderr)?;

    let exit_code = output
        .status
        .code()
        .ok_or_else(|| anyhow!("wallet '{}' command '{}' terminated by signal", wallet, command))?;
    let expected_code = expect_exit_code.unwrap_or(0);
    if exit_code != expected_code {
        return Err(anyhow!(
            "wallet '{}' command '{}' exited with {} (expected {}): {}",
            wallet,
            command,
            exit_code,
            expected_code,
            stderr.trim()
        ));
    }

    Ok(WalletCommandOutput { stdout, stderr })
}
fn resolve_wallet_bin(options: &RunOptions) -> Result<PathBuf> {
    if let Some(path) = &options.wallet_bin {
        return Ok(path.clone());
    }

    let release_path = PathBuf::from("target/release/nockchain-wallet");
    if release_path.exists() {
        return Ok(release_path);
    }

    let debug_path = PathBuf::from("target/debug/nockchain-wallet");
    if debug_path.exists() {
        return Ok(debug_path);
    }

    Ok(PathBuf::from("nockchain-wallet"))
}

fn is_explicit_path(path: &Path) -> bool {
    path.components().count() > 1 || path.is_absolute()
}

fn sanitize_name(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if ch == '-' || ch == '_' {
            out.push(ch);
        } else if ch.is_whitespace() {
            out.push('_');
        }
    }
    if out.is_empty() {
        "scenario".to_string()
    } else {
        out
    }
}

fn docker_network_name(run_dir: &Path) -> String {
    let run_name = run_dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "nockchain-e2e".to_string());
    let mut hasher = DefaultHasher::new();
    run_dir.to_string_lossy().hash(&mut hasher);
    let namespace = hasher.finish();
    format!(
        "nockchain-e2e-{}-{namespace:016x}",
        sanitize_name(&run_name)
    )
}

fn find_node_spec<'a>(scenario: &'a Scenario, id: &str) -> Result<&'a NodeSpec> {
    scenario
        .nodes
        .iter()
        .find(|spec| spec.id == id)
        .ok_or_else(|| anyhow!("unknown node id '{id}'"))
}

struct ExpectedConstants {
    jam: Vec<u8>,
    slab: NounSlab,
}

fn expected_constants(spec: &NodeSpec) -> Option<ExpectedConstants> {
    if !spec.fakenet {
        return None;
    }
    let pow_len = spec.fakenet_pow_len.unwrap_or(2);
    let log_difficulty = spec.fakenet_log_difficulty.unwrap_or(1);
    let v1_phase = spec.fakenet_v1_phase.unwrap_or(DEFAULT_FAKENET_V1_PHASE);
    let bythos_phase = spec
        .fakenet_bythos_phase
        .unwrap_or(DEFAULT_FAKENET_BYTHOS_PHASE);
    let mut constants = fakenet_blockchain_constants(pow_len, log_difficulty)
        .with_v1_phase(v1_phase)
        .with_bythos_phase(bythos_phase);
    if let Some(interval_secs) = spec.fakenet_update_candidate_interval_secs {
        constants = constants.with_update_candidate_timestamp_interval(Seconds(interval_secs));
    }
    let slab = constants.into_slab();
    let jam = slab.jam().to_vec();
    Some(ExpectedConstants { jam, slab })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use nockapp::noun::slab::NounSlab;
    use nockapp::noun::IntoSlab;
    use nockchain::config::{DEFAULT_FAKENET_BYTHOS_PHASE, DEFAULT_FAKENET_V1_PHASE};
    use nockchain_testkit::scenario::{
        ReqResGenerationExpectation, WalletCapture, WalletCaptureSource,
    };
    use nockchain_testkit::{Assert, NodeSpec};
    use nockchain_types::fakenet_blockchain_constants;
    use noun_serde::NounEncode;

    fn test_node_spec() -> NodeSpec {
        NodeSpec {
            id: "node-a".to_string(),
            grpc_public_addr: None,
            grpc_private_port: None,
            grpc_enabled: true,
            data_dir: None,
            fakenet: true,
            mine: false,
            mining_pkh: None,
            peers: Vec::new(),
            peer_from: Vec::new(),
            restart_peer_from: Vec::new(),
            force_peers: Vec::new(),
            bind: Vec::new(),
            new_state: false,
            no_default_peers: false,
            allowed_peers_path: None,
            fakenet_pow_len: Some(2),
            fakenet_log_difficulty: Some(1),
            fakenet_v1_phase: None,
            fakenet_bythos_phase: None,
            fakenet_update_candidate_interval_secs: None,
            fakenet_genesis_jam_path: None,
            extra_args: Vec::new(),
            env: Default::default(),
            binary: None,
        }
    }

    #[test]
    fn expected_constants_follow_fakenet_cli_phase_defaults() {
        let spec = test_node_spec();

        let expected = crate::runner::expected_constants(&spec).expect("fakenet constants");
        let reference = fakenet_blockchain_constants(2, 1)
            .with_v1_phase(DEFAULT_FAKENET_V1_PHASE)
            .with_bythos_phase(DEFAULT_FAKENET_BYTHOS_PHASE)
            .into_slab()
            .jam()
            .to_vec();

        assert_eq!(expected.jam, reference);
    }

    #[test]
    fn absolutize_work_dir_joins_relative_paths_to_cwd() {
        let relative = PathBuf::from("target/nockchain-e2e");
        let expected = std::env::current_dir()
            .expect("cwd")
            .join("target/nockchain-e2e");

        let actual =
            crate::runner::absolutize_work_dir(&relative).expect("relative path should absolutize");

        assert_eq!(actual, expected);
        assert!(actual.is_absolute());
    }

    #[test]
    fn absolutize_work_dir_preserves_absolute_paths() {
        let absolute = std::env::current_dir()
            .expect("cwd")
            .join("target/nockchain-e2e-absolute");

        let actual = crate::runner::absolutize_work_dir(&absolute)
            .expect("absolute path should pass through");

        assert_eq!(actual, absolute);
    }

    #[test]
    fn docker_network_name_uses_full_run_dir_namespace() {
        let left = PathBuf::from("/tmp/pipeline-a/double_spend_rejected-7");
        let right = PathBuf::from("/tmp/pipeline-b/double_spend_rejected-7");

        let left_name = crate::runner::docker_network_name(&left);
        let right_name = crate::runner::docker_network_name(&right);

        assert_ne!(left_name, right_name);
        assert!(left_name.starts_with("nockchain-e2e-double_spend_rejected-7-"));
        assert!(right_name.starts_with("nockchain-e2e-double_spend_rejected-7-"));
    }

    #[test]
    fn seed_run_state_registers_path_variables() {
        let run_dir = std::env::temp_dir().join(format!(
            "nockchain-e2e-runner-seed-state-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&run_dir).expect("create run dir");

        let mut state = crate::runner::RunState::default();
        crate::runner::seed_run_state(
            &mut state,
            &run_dir,
            Path::new("tests/e2e/scenarios/nous_testnet_gen2_send.yaml"),
        )
        .expect("seed run state");

        let repo_root = std::env::current_dir().expect("cwd");
        assert_eq!(
            state.vars.get("RUN_DIR"),
            Some(&run_dir.display().to_string())
        );
        assert_eq!(
            state.vars.get("REPO_ROOT"),
            Some(&repo_root.display().to_string())
        );
        assert_eq!(
            state.vars.get("SCENARIO_PATH"),
            Some(
                &repo_root
                    .join("tests/e2e/scenarios/nous_testnet_gen2_send.yaml")
                    .display()
                    .to_string()
            )
        );
        assert_eq!(
            state.vars.get("SCENARIO_DIR"),
            Some(&repo_root.join("tests/e2e/scenarios").display().to_string())
        );

        std::fs::remove_dir_all(&run_dir).expect("cleanup run dir");
    }

    #[test]
    fn resolve_command_cwd_joins_relative_paths_to_run_dir() {
        let run_dir = PathBuf::from("/tmp/nockchain-e2e-runner-command-cwd");
        let actual = crate::runner::resolve_command_cwd(
            &run_dir,
            Some(&PathBuf::from("commands/work")),
            &crate::runner::RunState::default(),
        )
        .expect("resolve relative cwd");

        assert_eq!(actual, run_dir.join("commands/work"));
    }

    #[test]
    fn seed_indexed_port_env_vars_populates_numbered_suffixes() {
        let prefix = format!("NOCKCHAIN_E2E_TEST_PORTS_{}", std::process::id());
        for key in [
            prefix.clone(),
            format!("{prefix}_1"),
            format!("{prefix}_2"),
            format!("{prefix}_3"),
        ] {
            std::env::remove_var(&key);
        }

        crate::runner::seed_indexed_port_env_vars(&prefix, 4800, 4);

        assert_eq!(std::env::var(&prefix).ok().as_deref(), Some("4800"));
        assert_eq!(
            std::env::var(format!("{prefix}_1")).ok().as_deref(),
            Some("4801")
        );
        assert_eq!(
            std::env::var(format!("{prefix}_2")).ok().as_deref(),
            Some("4802")
        );
        assert_eq!(
            std::env::var(format!("{prefix}_3")).ok().as_deref(),
            Some("4803")
        );

        for key in [
            prefix.clone(),
            format!("{prefix}_1"),
            format!("{prefix}_2"),
            format!("{prefix}_3"),
        ] {
            std::env::remove_var(&key);
        }
    }

    #[test]
    fn seed_binary_env_vars_sets_absolute_paths_and_preserves_overrides() {
        let keys = ["NOCKCHAIN_BIN_NEW", "NOCKCHAIN_WALLET_BIN", "NOCKCHAIN_E2E_BIN"];
        let saved = keys.map(|key| (key, std::env::var_os(key)));
        for key in keys {
            std::env::remove_var(key);
        }

        let repo_root = std::env::current_dir().expect("cwd");
        let explicit_e2e = repo_root.join("custom/nockchain-e2e");
        std::env::set_var("NOCKCHAIN_E2E_BIN", explicit_e2e.display().to_string());

        let options = crate::runner::RunOptions {
            scenario_path: PathBuf::from("scenario.yaml"),
            nockchain_bin: Some(PathBuf::from("bazel-bin/open/crates/nockchain/nockchain")),
            wallet_bin: Some(PathBuf::from(
                "bazel-bin/open/crates/nockchain-wallet/nockchain-wallet",
            )),
            work_dir: PathBuf::from("target/nockchain-e2e"),
            base_grpc_port: 6100,
            base_private_grpc_port: 7100,
            base_p2p_port: 4100,
            docker: false,
            docker_image: None,
            keep_artifacts: false,
        };

        crate::runner::seed_binary_env_vars(&options).expect("seed binary env");

        let expected_nockchain = repo_root
            .join("bazel-bin/open/crates/nockchain/nockchain")
            .display()
            .to_string();
        let expected_wallet = repo_root
            .join("bazel-bin/open/crates/nockchain-wallet/nockchain-wallet")
            .display()
            .to_string();
        let expected_e2e = explicit_e2e.display().to_string();

        assert_eq!(
            std::env::var("NOCKCHAIN_BIN_NEW").ok().as_deref(),
            Some(expected_nockchain.as_str())
        );
        assert_eq!(
            std::env::var("NOCKCHAIN_WALLET_BIN").ok().as_deref(),
            Some(expected_wallet.as_str())
        );
        assert_eq!(
            std::env::var("NOCKCHAIN_E2E_BIN").ok().as_deref(),
            Some(expected_e2e.as_str())
        );

        let second_options = crate::runner::RunOptions {
            scenario_path: PathBuf::from("scenario.yaml"),
            nockchain_bin: Some(PathBuf::from("other/nockchain")),
            wallet_bin: Some(PathBuf::from("other/nockchain-wallet")),
            work_dir: PathBuf::from("target/nockchain-e2e"),
            base_grpc_port: 6100,
            base_private_grpc_port: 7100,
            base_p2p_port: 4100,
            docker: false,
            docker_image: None,
            keep_artifacts: false,
        };
        crate::runner::seed_binary_env_vars(&second_options).expect("preserve binary env");

        assert_eq!(
            std::env::var("NOCKCHAIN_BIN_NEW").ok().as_deref(),
            Some(expected_nockchain.as_str())
        );

        for (key, value) in saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    #[tokio::test]
    async fn run_command_action_writes_logs_and_passes_env() {
        let run_dir = std::env::temp_dir().join(format!(
            "nockchain-e2e-runner-command-action-{}",
            std::process::id()
        ));
        let cwd = run_dir.join("nested/work");
        std::fs::create_dir_all(&run_dir).expect("create run dir");

        let mut env = BTreeMap::new();
        env.insert("COMMAND_TEST_VALUE".to_string(), "from-test".to_string());

        let output = crate::runner::run_command_action(
            &run_dir,
            7,
            "/bin/sh",
            &[
                "-lc".to_string(),
                "printf 'env=%s cwd=%s' \"$COMMAND_TEST_VALUE\" \"$PWD\"".to_string(),
            ],
            &env,
            &cwd,
            Some("env=from-test"),
            None,
            &crate::runner::RunState::default(),
        )
        .await
        .expect("run command action");

        let expected_cwd = cwd.canonicalize().expect("canonicalize command cwd");
        assert_eq!(
            output.stdout,
            format!("env=from-test cwd={}", expected_cwd.display())
        );
        assert_eq!(output.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(run_dir.join("commands/step-007-sh.stdout.log"))
                .expect("read stdout log"),
            output.stdout
        );
        assert_eq!(
            std::fs::read_to_string(run_dir.join("commands/step-007-sh.stderr.log"))
                .expect("read stderr log"),
            output.stderr
        );

        std::fs::remove_dir_all(&run_dir).expect("cleanup run dir");
    }

    #[test]
    fn extract_wallet_address_accepts_legacy_keygen_output() {
        let output = "### Address\nabc123xyz\n";

        let actual =
            crate::runner::extract_wallet_address(output).expect("legacy keygen output parses");

        assert_eq!(actual.as_deref(), Some("abc123xyz"));
    }

    #[test]
    fn extract_wallet_address_accepts_active_master_key_output() {
        let output = "Generated New Master Key (version 1)\n- Active master key is set to\nDWKSsPmHPHwKS4n4QA9J5rBwxCMWeA57949mvfHunCNgeiFocPTJHY5.\n";

        let actual =
            crate::runner::extract_wallet_address(output).expect("current keygen output parses");

        assert_eq!(
            actual.as_deref(),
            Some("DWKSsPmHPHwKS4n4QA9J5rBwxCMWeA57949mvfHunCNgeiFocPTJHY5")
        );
    }

    #[test]
    fn extract_wallet_address_accepts_wrapped_keygen_output() {
        let output = "Generated New Master Key (version 1)\n- Added keys to wallet.\n- Active master key is set to \n9hoayXZopWh6LX7szarDAMVuFYZUiFwnhEF5jtoVKNJwSTjiA5\nTMw3v.\n\nAddress\n9hoayXZopWh6LX7szarDAMVuFYZUiFwnhEF5jtoVKNJwSTjiA5\nTMw3v\n";

        let actual =
            crate::runner::extract_wallet_address(output).expect("wrapped keygen output parses");

        assert_eq!(
            actual.as_deref(),
            Some("9hoayXZopWh6LX7szarDAMVuFYZUiFwnhEF5jtoVKNJwSTjiA5TMw3v")
        );
    }

    #[test]
    fn extract_saved_tx_path_accepts_wrapped_output() {
        let output = "Create Tx\n - Saved transaction to \n./txs/67HETkuyoF9AninxW4DwUxBhxSXqa6uqCW2X6wy1MJe8\nU5QapvEPTiH.tx\n\nTransaction Information\n";

        let actual = crate::runner::extract_saved_tx_path(output);

        assert_eq!(
            actual.as_deref(),
            Some("./txs/67HETkuyoF9AninxW4DwUxBhxSXqa6uqCW2X6wy1MJe8U5QapvEPTiH.tx")
        );
    }

    #[test]
    fn extract_tx_id_accepts_wrapped_send_output() {
        let output = "Sent Tx\n- Validation for TX \nA7gtG4Ku4kDhtxrQz21bUx1eaZ9U65ZWNuq7TM3WxmPd29G6dc\nRCToh passed. TX has been submitted to node.\n";

        let actual = crate::runner::extract_tx_id(output);

        assert_eq!(
            actual.as_deref(),
            Some("A7gtG4Ku4kDhtxrQz21bUx1eaZ9U65ZWNuq7TM3WxmPd29G6dcRCToh")
        );
    }

    #[test]
    fn extract_tx_id_from_saved_raw_tx_uses_txs_debug_artifact() {
        let run_dir =
            std::env::temp_dir().join(format!("nockchain-e2e-runner-tx-id-{}", std::process::id()));
        let wallet_dir = run_dir.join("wallets/miner");
        let raw_tx_dir = wallet_dir.join("txs-debug");
        std::fs::create_dir_all(&raw_tx_dir).expect("create raw tx dir");

        let raw_tx = crate::runner::RawTx {
            version: nockchain_types::tx_engine::common::Version::V1,
            id: crate::runner::Hash::from_limbs(&[11, 22, 33, 44, 55]),
            spends: nockchain_types::tx_engine::v1::Spends(Vec::new()),
        };
        let mut slab: NounSlab = NounSlab::new();
        let noun = raw_tx.to_noun(&mut slab);
        slab.set_root(noun);
        std::fs::write(raw_tx_dir.join("example.jam"), slab.jam()).expect("write raw tx fixture");

        let actual = crate::runner::extract_tx_id_from_saved_raw_tx(
            &run_dir,
            "miner",
            &[String::from("./txs/example.tx")],
        )
        .expect("extract tx id")
        .expect("tx id should exist");

        assert_eq!(actual, raw_tx.id.to_base58());

        std::fs::remove_dir_all(&run_dir).expect("cleanup temp run dir");
    }

    #[test]
    fn copy_dir_recursive_replaces_existing_destination_contents() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time after unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "nockchain-e2e-copy-dir-{}-{unique}",
            std::process::id()
        ));
        let src = root.join("src");
        let dst = root.join("dst");
        let nested_src = src.join("wallet/checkpoints");
        let nested_dst = dst.join("wallet");

        std::fs::create_dir_all(&nested_src).expect("create nested source dir");
        std::fs::write(nested_src.join("0.chkjam"), "fresh-wallet-state")
            .expect("write fresh source file");

        std::fs::create_dir_all(&nested_dst).expect("create nested destination dir");
        std::fs::write(nested_dst.join("stale.chkjam"), "stale-wallet-state")
            .expect("write stale destination file");

        crate::runner::copy_dir_recursive(&src, &dst).expect("clone should replace destination");

        assert_eq!(
            std::fs::read_to_string(dst.join("wallet/checkpoints/0.chkjam"))
                .expect("fresh cloned wallet file"),
            "fresh-wallet-state"
        );
        assert!(
            !dst.join("wallet/stale.chkjam").exists(),
            "clone should discard stale destination contents"
        );

        std::fs::remove_dir_all(&root).expect("cleanup temp root");
    }

    #[test]
    fn wallet_output_contains_checks_both_streams() {
        let output = crate::runner::WalletCommandOutput {
            stdout: "stdout only".to_string(),
            stderr: "stderr has wallet notes".to_string(),
        };

        assert!(crate::runner::wallet_output_contains(
            &output, "wallet notes"
        ));
    }

    #[test]
    fn capture_wallet_output_can_match_combined_streams() {
        let output = crate::runner::WalletCommandOutput {
            stdout: "- Name: [note-part-a\n".to_string(),
            stderr: "note-part-b]".to_string(),
        };
        let capture = WalletCapture {
            regex: "(?s)- Name:\\s*(\\[[^\\]]+\\])".to_string(),
            store_as: "note_name".to_string(),
            source: WalletCaptureSource::Stdout,
        };
        let args = Vec::new();
        let context = crate::runner::WalletCommandContext {
            run_dir: Path::new("."),
            wallet: "miner",
            command: "list-notes",
            args: &args,
        };
        let mut state = crate::runner::RunState::default();

        crate::runner::capture_wallet_output(&context, &capture, &output, &mut state)
            .expect("capture should match combined output");

        assert_eq!(
            state.vars.get("note_name").map(String::as_str),
            Some("[note-part-anote-part-b]")
        );
    }

    #[test]
    fn capture_wallet_output_selects_oldest_listed_note() {
        let output = crate::runner::WalletCommandOutput {
            stdout: "\
Wallet Notes
Note Information
- Name:
[new-note-a
 new-note-b]
- Block Height: 10
Note Information
- Name:
[old-note-a
 old-note-b]
- Block Height: 3
"
            .to_string(),
            stderr: String::new(),
        };
        let capture = WalletCapture {
            regex: "(?s)- Name:\\s*(\\[[^\\]]+\\])".to_string(),
            store_as: "note_name".to_string(),
            source: WalletCaptureSource::Stdout,
        };
        let args = Vec::new();
        let context = crate::runner::WalletCommandContext {
            run_dir: Path::new("."),
            wallet: "miner",
            command: "list-notes",
            args: &args,
        };
        let mut state = crate::runner::RunState::default();

        crate::runner::capture_wallet_output(&context, &capture, &output, &mut state)
            .expect("capture should select an oldest note");

        assert_eq!(
            state.vars.get("note_name").map(String::as_str),
            Some("[old-note-a old-note-b]")
        );
    }

    #[test]
    fn req_res_generation_logged_matches_completed_exchange_line() {
        let logs = "\
[INFO  nockchain_libp2p_io::driver] Nous req-res exchange completed peer=peer-a request_id=1 generation=Gen1 request_shape=\"request\"\n\
[INFO  nockchain_libp2p_io::driver] Nous req-res outbound request sent peer=peer-b request_id=2 generation=Gen2 request_shape=\"batch-request\"\n";

        assert!(crate::runner::req_res_generation_logged(
            logs,
            "peer-a",
            ReqResGenerationExpectation::Gen1
        ));
        assert!(!crate::runner::req_res_generation_logged(
            logs,
            "peer-a",
            ReqResGenerationExpectation::Gen2
        ));
        assert!(!crate::runner::req_res_generation_logged(
            logs,
            "peer-b",
            ReqResGenerationExpectation::Gen2
        ));
    }

    #[test]
    fn req_res_generation_logged_matches_ansi_styled_completed_exchange_line() {
        let logs = "\
\u{1b}[32mI\u{1b}[0m \u{1b}[38;5;246m(15:52:58)\u{1b}[0m \u{1b}[3;90mdriver\u{1b}[0m: \0Nous req-res exchange completed peer=peer-a request_id=1 generation=Gen1 request_shape=\"request\"\n\
\u{1b}[32mI\u{1b}[0m \u{1b}[38;5;246m(15:52:58)\u{1b}[0m \u{1b}[3;90mdriver\u{1b}[0m: Nous req-res outbound request sent peer=peer-b request_id=2 generation=Gen2 request_shape=\"batch-request\"\n";

        assert!(crate::runner::req_res_generation_logged(
            logs,
            "peer-a",
            ReqResGenerationExpectation::Gen1
        ));
        assert!(!crate::runner::req_res_generation_logged(
            logs,
            "peer-a",
            ReqResGenerationExpectation::Gen2
        ));
        assert!(!crate::runner::req_res_generation_logged(
            logs,
            "peer-b",
            ReqResGenerationExpectation::Gen2
        ));
    }

    #[test]
    fn assert_type_name_formats_req_res_generation_assert() {
        let assert = Assert::ReqResGeneration {
            node: "node-b".to_string(),
            peer: "node-a".to_string(),
            generation: ReqResGenerationExpectation::Gen1,
            timeout_ms: Some(30_000),
        };

        assert_eq!(
            crate::runner::assert_type_name(&assert),
            "req_res_generation:node-b->node-a:gen1"
        );
    }
}
