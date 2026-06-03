use std::collections::{HashMap, HashSet};
use std::net::{SocketAddr, TcpListener as StdTcpListener, UdpSocket as StdUdpSocket};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use nockapp_grpc_proto::pb::common::v1::Hash as PbHash;
use nockchain_testkit::{NodeSpec, PeerFrom};
use testcontainers::core::{AccessMode, IntoContainerPort, Mount, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{GenericImage, ImageExt, TestcontainersError};
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout, Instant};
use tracing::info;

use crate::grpc::{
    fetch_constants_private, fetch_head, fetch_heaviest_private, wait_for_height,
    wait_for_height_private, wait_for_ready, HeadInfo, PrivateHeadInfo,
};

const PEER_ID_FILENAME: &str = ".nockchain_identity.peerid";
const DOCKER_WORKDIR: &str = "/var/nockchain";
const PEER_ID_WAIT_TIMEOUT: Duration = Duration::from_secs(120);
const PROCESS_SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub enum NodeMode {
    Process,
    Docker { image: String, network: String },
}

pub struct NodeManager {
    mode: NodeMode,
    configs: HashMap<String, NodeConfig>,
    handles: HashMap<String, NodeHandle>,
}

impl NodeManager {
    pub fn new(
        specs: &[NodeSpec],
        run_dir: &Path,
        nockchain_bin: Option<PathBuf>,
        base_grpc_port: u16,
        base_private_grpc_port: u16,
        base_p2p_port: u16,
        mode: NodeMode,
    ) -> Result<Self> {
        let p2p_ports = resolve_p2p_ports(specs, base_p2p_port)?;
        let mut configs = HashMap::new();
        for (index, spec) in specs.iter().enumerate() {
            let grpc_public_addr = match &spec.grpc_public_addr {
                Some(addr) => addr.clone(),
                None => format!("127.0.0.1:{}", base_grpc_port + index as u16),
            };
            let _addr: SocketAddr = grpc_public_addr
                .parse()
                .with_context(|| format!("invalid grpc_public_addr for {}", spec.id))?;
            let grpc_public_port = parse_socket_port(&grpc_public_addr)?;

            let grpc_private_port = spec
                .grpc_private_port
                .unwrap_or(base_private_grpc_port + index as u16);
            let grpc_private_addr = format!("127.0.0.1:{grpc_private_port}");
            let p2p_port = *p2p_ports
                .get(&spec.id)
                .ok_or_else(|| anyhow!("missing p2p port assignment for {}", spec.id))?;

            let work_dir = resolve_work_dir(run_dir, spec)?;
            let binary = absolutize_binary(expand_env_path(resolve_binary(
                spec,
                nockchain_bin.as_ref(),
            ))?)?;

            let supports_bythos_phase = match &mode {
                NodeMode::Process => binary_supports_flag(&binary, "--fakenet-bythos-phase")?,
                NodeMode::Docker { .. } => true,
            };

            let config = NodeConfig {
                base_spec: spec.clone(),
                spec: spec.clone(),
                grpc_public_addr,
                grpc_public_port,
                grpc_private_addr,
                grpc_private_port,
                grpc_private_host_port: None,
                p2p_port,
                work_dir,
                binary,
                supports_bythos_phase,
            };
            configs.insert(spec.id.clone(), config);
        }

        if let NodeMode::Docker { network, .. } = &mode {
            create_docker_network(network)?;
        }

        Ok(Self {
            mode,
            configs,
            handles: HashMap::new(),
        })
    }

    pub async fn start_nodes(&mut self, ids: &[String]) -> Result<()> {
        self.ensure_ports_available(ids)?;
        match &self.mode {
            NodeMode::Process => {
                let mut started = Vec::new();
                for id in ids {
                    if self.handles.contains_key(id) {
                        continue;
                    }
                    let config = self
                        .configs
                        .get(id)
                        .ok_or_else(|| anyhow!("unknown node id '{id}'"))?
                        .clone();

                    let peer_from = self.resolve_peer_from(&config.spec).await?;
                    let handle = spawn_process_node(&config, &peer_from).await?;
                    self.handles.insert(id.clone(), handle);
                    started.push(id.clone());
                    if let Some(config) = self.configs.get_mut(id) {
                        config.spec.new_state = false;
                    }
                }
                self.ensure_process_nodes_running(&started).await?;
                Ok(())
            }
            NodeMode::Docker { image, network } => {
                let mut started = Vec::new();
                let port_map = build_p2p_port_map(&self.configs)?;
                for id in ids {
                    if self.handles.contains_key(id) {
                        continue;
                    }
                    let config = self
                        .configs
                        .get(id)
                        .ok_or_else(|| anyhow!("unknown node id '{id}'"))?
                        .clone();

                    let peer_from = self.resolve_peer_from(&config.spec).await?;
                    let docker_peers = self
                        .rewrite_peers_for_docker(&config.spec, &port_map, network)
                        .await?;
                    let container =
                        spawn_docker_node(&config, image, network, &peer_from, &docker_peers)
                            .await?;
                    let host = container.get_host().await?;
                    let grpc_public_host_port = container
                        .get_host_port_ipv4(config.grpc_public_port.tcp())
                        .await?;
                    let grpc_private_host_port = container
                        .get_host_port_ipv4(config.grpc_private_port.tcp())
                        .await?;
                    let config_mut = self
                        .configs
                        .get_mut(id)
                        .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
                    config_mut.grpc_public_addr = format!("{host}:{grpc_public_host_port}");
                    config_mut.grpc_private_addr = format!("{host}:{grpc_private_host_port}");
                    config_mut.grpc_private_host_port = Some(grpc_private_host_port);
                    self.handles
                        .insert(id.clone(), NodeHandle::Docker(container));
                    config_mut.spec.new_state = false;
                    started.push(id.clone());
                }
                self.ensure_docker_nodes_running(&started).await?;
                Ok(())
            }
        }
    }

    pub async fn stop_nodes(&mut self, ids: &[String]) -> Result<()> {
        for id in ids {
            if let Some(handle) = self.handles.remove(id) {
                match handle {
                    NodeHandle::Process(mut handle) => {
                        stop_process_child(&mut handle.child).await;
                    }
                    NodeHandle::Docker(container) => {
                        if let Some(config) = self.configs.get(id) {
                            let _ = persist_docker_logs(config, &container).await;
                        }
                        let _ = container.stop().await;
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn shutdown_all(&mut self) -> Result<()> {
        let ids: Vec<String> = self.handles.keys().cloned().collect();
        self.stop_nodes(&ids).await?;
        if let NodeMode::Docker { network, .. } = &self.mode {
            let _ = remove_docker_network(network);
        }
        Ok(())
    }

    pub async fn restart_nodes(&mut self, ids: &[String]) -> Result<()> {
        self.stop_nodes(ids).await?;
        self.start_nodes(ids).await
    }

    pub fn set_node_env(&mut self, id: &str, key: String, value: String) -> Result<()> {
        let config = self
            .configs
            .get_mut(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        config.spec.env.insert(key, value);
        Ok(())
    }

    pub async fn wait_for_grpc(&self, id: &str, timeout: std::time::Duration) -> Result<()> {
        let addr = self.grpc_public_addr(id)?;
        match wait_for_ready(addr, timeout).await {
            Ok(()) => Ok(()),
            Err(err) => {
                if let Some(diagnostics) = self.capture_docker_diagnostics(id).await? {
                    return Err(err.context(diagnostics));
                }
                Err(err)
            }
        }
    }

    async fn ensure_process_nodes_running(&mut self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        sleep(Duration::from_millis(200)).await;
        for id in ids {
            let Some(handle) = self.handles.get_mut(id) else {
                continue;
            };
            if let NodeHandle::Process(handle) = handle {
                if let Some(status) = handle.child.try_wait()? {
                    let log_path = self
                        .configs
                        .get(id)
                        .map(|config| config.work_dir.join("stderr.log"))
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "<unknown>".to_string());
                    return Err(anyhow!(
                        "node '{}' exited early with status {} (see {})", id, status, log_path
                    ));
                }
            }
        }
        Ok(())
    }

    async fn ensure_docker_nodes_running(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        sleep(Duration::from_millis(200)).await;
        for id in ids {
            let Some(handle) = self.handles.get(id) else {
                continue;
            };
            if let NodeHandle::Docker(container) = handle {
                if !container.is_running().await? {
                    let diagnostics = self
                        .capture_docker_diagnostics(id)
                        .await?
                        .unwrap_or_else(|| format!("docker node '{}' is not running", id));
                    return Err(anyhow!(
                        "docker node '{}' exited early: {}", id, diagnostics
                    ));
                }
            }
        }
        Ok(())
    }

    pub async fn wait_for_height(
        &self,
        id: &str,
        height: u64,
        timeout: std::time::Duration,
    ) -> Result<HeadInfo> {
        let private_addr = self.grpc_private_addr(id)?;
        let public_addr = self.grpc_public_addr(id)?;
        let start = Instant::now();
        match wait_for_height_private(private_addr, height, timeout).await {
            Ok(private_head) => match self.fetch_head(id).await {
                Ok(head) => Ok(head),
                Err(_) => Ok(head_info_from_private(private_head)),
            },
            Err(err) => {
                let remaining = timeout.saturating_sub(start.elapsed());
                let public_result = if remaining.is_zero() {
                    Err(err)
                } else {
                    wait_for_height(public_addr, height, remaining).await
                };
                match public_result {
                    Ok(head) => Ok(head),
                    Err(err) => {
                        if let Some(diagnostics) = self.capture_docker_diagnostics(id).await? {
                            Err(err.context(diagnostics))
                        } else {
                            Err(err)
                        }
                    }
                }
            }
        }
    }

    pub async fn fetch_head(&self, id: &str) -> Result<HeadInfo> {
        let addr = self.grpc_public_addr(id)?;
        fetch_head(addr).await
    }

    pub async fn fetch_private_head(&self, id: &str) -> Result<PrivateHeadInfo> {
        let addr = self.grpc_private_addr(id)?;
        fetch_heaviest_private(addr).await
    }

    pub async fn fetch_private_head_if_current(
        &mut self,
        id: &str,
    ) -> Result<Option<PrivateHeadInfo>> {
        if !self.node_handle_is_live(id).await? {
            return Ok(None);
        }
        self.fetch_private_head(id).await.map(Some)
    }

    pub async fn fetch_constants(&self, id: &str) -> Result<Vec<u8>> {
        let addr = self.grpc_private_addr(id)?;
        fetch_constants_private(addr).await
    }

    pub fn grpc_private_port(&self, id: &str) -> Result<u16> {
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        Ok(config
            .grpc_private_host_port
            .unwrap_or(config.grpc_private_port))
    }

    pub async fn upgrade_node(&mut self, id: &str, binary: PathBuf) -> Result<()> {
        let config = self
            .configs
            .get_mut(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        match &self.mode {
            NodeMode::Process => {
                config.binary = absolutize_binary(binary)?;
                config.supports_bythos_phase =
                    binary_supports_flag(&config.binary, "--fakenet-bythos-phase")?;
            }
            NodeMode::Docker { .. } => {
                config.spec.binary = Some(binary);
                config.supports_bythos_phase = true;
            }
        }
        let ids = vec![id.to_string()];
        self.restart_nodes(&ids).await
    }

    pub async fn set_mining_pkh(&mut self, id: &str, mining_pkh: String) -> Result<()> {
        let config = self
            .configs
            .get_mut(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        config.spec.mining_pkh = Some(mining_pkh);
        if !config.spec.mine {
            config.spec.mine = true;
        }
        if self.handles.contains_key(id) {
            let ids = vec![id.to_string()];
            self.restart_nodes(&ids).await?;
        }
        Ok(())
    }

    pub async fn disable_mining(&mut self, id: &str) -> Result<()> {
        let config = self
            .configs
            .get_mut(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        config.spec.mine = false;
        config.spec.mining_pkh = None;
        if self.handles.contains_key(id) {
            let ids = vec![id.to_string()];
            self.restart_nodes(&ids).await?;
        }
        Ok(())
    }

    fn ensure_ports_available(&self, ids: &[String]) -> Result<()> {
        for id in ids {
            if self.handles.contains_key(id) {
                continue;
            }
            let config = self
                .configs
                .get(id)
                .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
            if config.spec.grpc_enabled {
                ensure_tcp_port_available(
                    &config.spec.id, "public gRPC", &config.grpc_public_addr,
                    "choose a different --base-grpc-port block or stop the stale process",
                )?;
                ensure_tcp_port_available(
                    &config.spec.id, "private gRPC", &config.grpc_private_addr,
                    "choose a different --base-private-grpc-port block or stop the stale process",
                )?;
            }
            ensure_udp_port_available(
                &config.spec.id, "p2p", config.p2p_port,
                "choose a different --base-p2p-port block or stop the stale process",
            )?;
        }
        Ok(())
    }

    async fn node_handle_is_live(&mut self, id: &str) -> Result<bool> {
        let Some(handle) = self.handles.get_mut(id) else {
            return Ok(false);
        };
        match handle {
            NodeHandle::Process(handle) => Ok(handle.child.try_wait()?.is_none()),
            NodeHandle::Docker(container) => container.is_running().await.map_err(Into::into),
        }
    }

    pub async fn apply_partition(&mut self, groups: &[Vec<String>], _run_dir: &Path) -> Result<()> {
        if groups.is_empty() {
            return Err(anyhow!("partition requires at least one group"));
        }

        let all_nodes: Vec<String> = self.configs.keys().cloned().collect();
        let mut group_index: HashMap<String, usize> = HashMap::new();
        for (idx, group) in groups.iter().enumerate() {
            for node in group {
                if group_index.insert(node.clone(), idx).is_some() {
                    return Err(anyhow!(
                        "node '{node}' appears in multiple partition groups"
                    ));
                }
            }
        }

        let mut unknown_nodes = Vec::new();
        for node in group_index.keys() {
            if !self.configs.contains_key(node) {
                unknown_nodes.push(node.clone());
            }
        }
        if !unknown_nodes.is_empty() {
            return Err(anyhow!(
                "partition groups include unknown nodes: {:?}", unknown_nodes
            ));
        }

        let mut target_nodes: Vec<String> = group_index.keys().cloned().collect();
        target_nodes.sort();
        target_nodes.dedup();

        let clear_partition = groups.len() == 1 && {
            let group = &groups[0];
            let mut group_sorted = group.clone();
            group_sorted.sort();
            let mut all_sorted = all_nodes.clone();
            all_sorted.sort();
            group_sorted == all_sorted
        };

        if clear_partition {
            for node in &target_nodes {
                let config = self
                    .configs
                    .get_mut(node)
                    .ok_or_else(|| anyhow!("unknown node id '{node}'"))?;
                config.spec.no_default_peers = config.base_spec.no_default_peers;
                config.spec.allowed_peers_path = config.base_spec.allowed_peers_path.clone();
            }
            return self.restart_nodes(&target_nodes).await;
        }

        let mut peer_ids = HashMap::new();
        for node in &target_nodes {
            let config = self
                .configs
                .get(node)
                .ok_or_else(|| anyhow!("unknown node id '{node}'"))?;
            let peer_id = wait_for_peer_id(&config.work_dir).await?;
            peer_ids.insert(node.clone(), peer_id);
        }

        for node in &target_nodes {
            let idx = group_index
                .get(node)
                .ok_or_else(|| anyhow!("missing group for node '{node}'"))?;
            let allowlist = groups
                .get(*idx)
                .ok_or_else(|| anyhow!("missing partition group {idx}"))?;
            let mut entries = Vec::new();
            for peer in allowlist {
                if let Some(peer_id) = peer_ids.get(peer) {
                    entries.push(peer_id.clone());
                }
            }

            let peers_dir = self
                .configs
                .get(node)
                .ok_or_else(|| anyhow!("unknown node id '{node}'"))?
                .work_dir
                .join("peers");
            std::fs::create_dir_all(&peers_dir)?;
            let path = peers_dir.join("allowed-peers.txt");
            std::fs::write(&path, entries.join("\n"))?;
            let config = self
                .configs
                .get_mut(node)
                .ok_or_else(|| anyhow!("unknown node id '{node}'"))?;
            config.spec.no_default_peers = true;
            config.spec.allowed_peers_path = Some(path);
        }

        self.restart_nodes(&target_nodes).await
    }

    pub fn grpc_public_addr(&self, id: &str) -> Result<&str> {
        if !self.handles.contains_key(id) {
            return Err(anyhow!("node '{id}' is not running"));
        }
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        if !config.spec.grpc_enabled {
            return Err(anyhow!("node '{id}' has gRPC disabled"));
        }
        Ok(config.grpc_public_addr.as_str())
    }

    pub fn grpc_private_addr(&self, id: &str) -> Result<&str> {
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        if !config.spec.grpc_enabled {
            return Err(anyhow!("node '{id}' has gRPC disabled"));
        }
        Ok(config.grpc_private_addr.as_str())
    }

    pub fn is_fakenet(&self, id: &str) -> Result<bool> {
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        Ok(config.spec.fakenet)
    }

    pub fn fakenet_phase_overrides(&self, id: &str) -> Result<(Option<u64>, Option<u64>)> {
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        if !config.spec.fakenet {
            return Ok((None, None));
        }
        let bythos_phase = if config.supports_bythos_phase {
            config.spec.fakenet_bythos_phase
        } else {
            None
        };
        Ok((config.spec.fakenet_v1_phase, bythos_phase))
    }

    pub async fn wait_for_peer_id(&self, id: &str) -> Result<String> {
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        wait_for_peer_id(&config.work_dir).await
    }

    pub async fn combined_logs(&self, id: &str) -> Result<String> {
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;

        let (stdout, stderr) = match self.handles.get(id) {
            Some(NodeHandle::Process(_)) => (
                read_log_file(&config.work_dir.join("stdout.log"))?,
                read_log_file(&config.work_dir.join("stderr.log"))?,
            ),
            Some(NodeHandle::Docker(container)) => {
                let (_, _, stdout, stderr) = persist_docker_logs(config, container).await?;
                (stdout, stderr)
            }
            None => return Err(anyhow!("node '{id}' is not running")),
        };

        Ok(format!("{stdout}\n{stderr}"))
    }

    async fn capture_docker_diagnostics(&self, id: &str) -> Result<Option<String>> {
        let Some(NodeHandle::Docker(container)) = self.handles.get(id) else {
            return Ok(None);
        };
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;

        std::fs::create_dir_all(&config.work_dir)?;

        let (stdout_path, stderr_path, stdout, stderr) =
            persist_docker_logs(config, container).await?;

        let running = match container.is_running().await {
            Ok(value) => value.to_string(),
            Err(err) => format!("unknown ({err})"),
        };
        let exit_code = match container.exit_code().await {
            Ok(value) => format!("{value:?}"),
            Err(err) => format!("unknown ({err})"),
        };

        let stderr_tail = log_tail_preview(&stderr, 12);
        let stdout_tail = log_tail_preview(&stdout, 12);
        let preview = if stderr_tail.is_empty() {
            stdout_tail
        } else {
            stderr_tail
        };
        let preview = if preview.is_empty() {
            "<no container logs captured>".to_string()
        } else {
            preview
        };

        Ok(Some(format!(
            "docker node '{}' status running={} exit_code={} stdout_log={} stderr_log={} log_tail:\n{}",
            id,
            running,
            exit_code,
            stdout_path.display(),
            stderr_path.display(),
            preview
        )))
    }
}

#[derive(Clone)]
struct NodeConfig {
    base_spec: NodeSpec,
    spec: NodeSpec,
    grpc_public_addr: String,
    grpc_public_port: u16,
    grpc_private_addr: String,
    grpc_private_port: u16,
    grpc_private_host_port: Option<u16>,
    p2p_port: u16,
    work_dir: PathBuf,
    binary: PathBuf,
    supports_bythos_phase: bool,
}

struct ProcessHandle {
    child: Child,
}

#[allow(clippy::large_enum_variant)]
enum NodeHandle {
    Process(ProcessHandle),
    Docker(testcontainers::ContainerAsync<GenericImage>),
}

struct DockerPeers {
    peers: Vec<String>,
    force_peers: Vec<String>,
}

async fn stop_process_child(child: &mut Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }

    if request_process_shutdown(child)
        && matches!(
            timeout(PROCESS_SHUTDOWN_GRACE, child.wait()).await,
            Ok(Ok(_))
        )
    {
        return;
    }

    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[cfg(unix)]
fn request_process_shutdown(child: &Child) -> bool {
    let Some(pid) = child.id() else {
        return false;
    };

    // SAFETY: pid comes from the live child process handle and SIGTERM is a valid signal.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) == 0 }
}

#[cfg(not(unix))]
fn request_process_shutdown(_: &Child) -> bool {
    false
}

async fn spawn_process_node(config: &NodeConfig, peer_from: &[String]) -> Result<NodeHandle> {
    std::fs::create_dir_all(&config.work_dir)?;

    let stdout_path = config.work_dir.join("stdout.log");
    let stderr_path = config.work_dir.join("stderr.log");
    let stdout = std::fs::File::create(stdout_path)?;
    let stderr = std::fs::File::create(stderr_path)?;

    let args = build_node_args(config, peer_from)?;
    info!(
        "spawning node {}: {} {}",
        config.spec.id,
        config.binary.display(),
        args.join(" ")
    );

    let mut command = Command::new(&config.binary);
    command
        .current_dir(&config.work_dir)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .args(&args);
    for (key, value) in &config.spec.env {
        command.env(key, value);
    }

    let child = command.spawn().context("spawn nockchain")?;

    Ok(NodeHandle::Process(ProcessHandle { child }))
}

async fn spawn_docker_node(
    config: &NodeConfig,
    default_image: &str,
    network: &str,
    peer_from: &[String],
    docker_peers: &DockerPeers,
) -> Result<testcontainers::ContainerAsync<GenericImage>> {
    std::fs::create_dir_all(&config.work_dir)?;

    let image_ref = resolve_docker_image(config, default_image)?;
    let image = parse_image_ref(&image_ref);
    let args = build_node_args_docker(config, peer_from, docker_peers)?;
    let container_name = docker_container_name(network, &config.spec.id);
    remove_stale_docker_container(&container_name)?;

    info!(
        "spawning docker node {}: {}:{} {}",
        config.spec.id,
        image.name,
        image.tag,
        args.join(" ")
    );

    let mount = Mount::bind_mount(config.work_dir.display().to_string(), DOCKER_WORKDIR)
        .with_access_mode(AccessMode::ReadWrite);

    let mut image = GenericImage::new(image.name, image.tag)
        .with_wait_for(WaitFor::Nothing)
        .with_exposed_port(config.p2p_port.udp());

    if config.spec.grpc_enabled {
        image = image
            .with_exposed_port(config.grpc_public_port.tcp())
            .with_exposed_port(config.grpc_private_port.tcp());
    }

    let mut request = image
        .with_entrypoint("/usr/local/bin/nockchain")
        .with_cmd(args)
        .with_network(network)
        .with_container_name(&container_name)
        .with_working_dir(DOCKER_WORKDIR)
        .with_mount(mount);

    #[cfg(unix)]
    {
        request = request.with_user(current_host_user());
    }

    for (key, value) in &config.spec.env {
        request = request.with_env_var(key, value);
    }

    request.start().await.map_err(Into::into)
}

#[cfg(unix)]
fn current_host_user() -> String {
    // SAFETY: geteuid/getegid read process credentials and have no preconditions.
    let uid = unsafe { libc::geteuid() };
    // SAFETY: geteuid/getegid read process credentials and have no preconditions.
    let gid = unsafe { libc::getegid() };
    format!("{uid}:{gid}")
}

#[allow(clippy::vec_init_then_push)]
fn build_node_args(config: &NodeConfig, peer_from: &[String]) -> Result<Vec<String>> {
    let spec = &config.spec;
    let mut args = Vec::new();

    if spec.grpc_enabled {
        args.push("--bind-public-grpc-addr".to_string());
        args.push(config.grpc_public_addr.clone());
        args.push("--bind-private-grpc-port".to_string());
        args.push(config.grpc_private_port.to_string());
    }

    if spec.fakenet {
        args.push("--fakenet".to_string());
    }

    if spec.new_state {
        args.push("--new".to_string());
    }

    if spec.mine {
        args.push("--mine".to_string());
    }

    if let Some(pkh) = &spec.mining_pkh {
        args.push("--mining-pkh".to_string());
        args.push(pkh.clone());
    }

    if spec.no_default_peers {
        args.push("--no-default-peers".to_string());
    }

    args.push("--no-new-peer-id".to_string());

    if let Some(path) = &spec.allowed_peers_path {
        args.push("--allowed-peers-path".to_string());
        args.push(absolutize_path(path)?.display().to_string());
    }

    let topology_peer_arg = topology_peer_arg(spec);
    push_peer_args(&mut args, topology_peer_arg, peer_from);
    push_peer_args(&mut args, topology_peer_arg, &spec.peers);
    push_peer_args(&mut args, "--force-peer", &spec.force_peers);

    if spec.bind.is_empty() {
        args.push("--bind".to_string());
        args.push(format!("/ip4/127.0.0.1/udp/{}/quic-v1", config.p2p_port));
    } else {
        for bind in &spec.bind {
            args.push("--bind".to_string());
            args.push(bind.clone());
        }
    }

    if let Some(pow_len) = spec.fakenet_pow_len {
        args.push("--fakenet-pow-len".to_string());
        args.push(pow_len.to_string());
    }

    if let Some(log_difficulty) = spec.fakenet_log_difficulty {
        args.push("--fakenet-log-difficulty".to_string());
        args.push(log_difficulty.to_string());
    }

    if let Some(phase) = spec.fakenet_v1_phase {
        args.push("--fakenet-v1-phase".to_string());
        args.push(phase.to_string());
    }

    if let Some(phase) = spec.fakenet_bythos_phase {
        if config.supports_bythos_phase {
            args.push("--fakenet-bythos-phase".to_string());
            args.push(phase.to_string());
        }
    }

    if let Some(interval_secs) = spec.fakenet_update_candidate_interval_secs {
        args.push("--fakenet-update-candidate-interval-secs".to_string());
        args.push(interval_secs.to_string());
    }

    if let Some(path) = &spec.fakenet_genesis_jam_path {
        args.push("--fakenet-genesis-jam-path".to_string());
        args.push(absolutize_path(path)?.display().to_string());
    }

    for arg in &spec.extra_args {
        args.push(arg.clone());
    }

    Ok(args)
}

fn build_node_args_docker(
    config: &NodeConfig,
    peer_from: &[String],
    docker_peers: &DockerPeers,
) -> Result<Vec<String>> {
    let spec = &config.spec;
    let mut args = Vec::new();

    if spec.grpc_enabled {
        args.push("--bind-public-grpc-addr".to_string());
        args.push(format!("0.0.0.0:{}", config.grpc_public_port));
        args.push("--bind-private-grpc-addr".to_string());
        args.push(format!("0.0.0.0:{}", config.grpc_private_port));
    }

    if spec.fakenet {
        args.push("--fakenet".to_string());
    }

    if spec.new_state {
        args.push("--new".to_string());
    }

    if spec.mine {
        args.push("--mine".to_string());
    }

    if let Some(pkh) = &spec.mining_pkh {
        args.push("--mining-pkh".to_string());
        args.push(pkh.clone());
    }

    if spec.no_default_peers {
        args.push("--no-default-peers".to_string());
    }

    args.push("--no-new-peer-id".to_string());

    if let Some(path) = &spec.allowed_peers_path {
        args.push("--allowed-peers-path".to_string());
        args.push(container_path_for(config, path)?);
    }

    let topology_peer_arg = topology_peer_arg(spec);
    push_peer_args(&mut args, topology_peer_arg, peer_from);
    push_peer_args(&mut args, topology_peer_arg, &docker_peers.peers);
    push_peer_args(&mut args, "--force-peer", &docker_peers.force_peers);

    if spec.bind.is_empty() {
        args.push("--bind".to_string());
        args.push(format!("/ip4/0.0.0.0/udp/{}/quic-v1", config.p2p_port));
    } else {
        for bind in &spec.bind {
            args.push("--bind".to_string());
            args.push(rewrite_bind_for_docker(bind)?);
        }
    }

    if let Some(pow_len) = spec.fakenet_pow_len {
        args.push("--fakenet-pow-len".to_string());
        args.push(pow_len.to_string());
    }

    if let Some(log_difficulty) = spec.fakenet_log_difficulty {
        args.push("--fakenet-log-difficulty".to_string());
        args.push(log_difficulty.to_string());
    }

    if let Some(phase) = spec.fakenet_v1_phase {
        args.push("--fakenet-v1-phase".to_string());
        args.push(phase.to_string());
    }

    if let Some(phase) = spec.fakenet_bythos_phase {
        if config.supports_bythos_phase {
            args.push("--fakenet-bythos-phase".to_string());
            args.push(phase.to_string());
        }
    }

    if let Some(interval_secs) = spec.fakenet_update_candidate_interval_secs {
        args.push("--fakenet-update-candidate-interval-secs".to_string());
        args.push(interval_secs.to_string());
    }

    if let Some(path) = &spec.fakenet_genesis_jam_path {
        args.push("--fakenet-genesis-jam-path".to_string());
        args.push(container_path_for(config, path)?);
    }

    for arg in &spec.extra_args {
        args.push(arg.clone());
    }

    Ok(args)
}

fn topology_peer_arg(spec: &NodeSpec) -> &'static str {
    if spec.new_state {
        "--peer"
    } else {
        "--force-peer"
    }
}

fn push_peer_args(args: &mut Vec<String>, flag: &str, peers: &[String]) {
    for peer in peers {
        args.push(flag.to_string());
        args.push(peer.clone());
    }
}

fn resolve_work_dir(run_dir: &Path, spec: &NodeSpec) -> Result<PathBuf> {
    if let Some(dir) = &spec.data_dir {
        if dir.is_absolute() {
            return Ok(dir.clone());
        }
        return Ok(run_dir.join(dir));
    }

    Ok(run_dir.join("nodes").join(&spec.id))
}

fn resolve_binary(spec: &NodeSpec, override_bin: Option<&PathBuf>) -> PathBuf {
    if let Some(path) = &spec.binary {
        return path.clone();
    }
    if let Some(path) = override_bin {
        return path.clone();
    }
    PathBuf::from("nockchain")
}

fn expand_env_path(path: PathBuf) -> Result<PathBuf> {
    let raw = path.to_string_lossy();
    if !raw.contains("${") {
        return Ok(path);
    }
    let expanded = expand_env_vars(&raw)?;
    Ok(PathBuf::from(expanded))
}

#[allow(clippy::while_let_on_iterator)]
fn expand_env_vars(input: &str) -> Result<String> {
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
            let value = std::env::var(&key)
                .map_err(|_| anyhow!("missing variable '{}' in '{}'", key, input))?;
            out.push_str(&value);
        } else {
            out.push(ch);
        }
    }

    Ok(out)
}

fn binary_supports_flag(binary: &Path, flag: &str) -> Result<bool> {
    let output = std::process::Command::new(binary)
        .arg("--help")
        .output()
        .with_context(|| format!("failed to run {} --help", binary.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(stdout.contains(flag) || stderr.contains(flag))
}

fn absolutize_binary(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    let cwd = std::env::current_dir().context("resolve binary cwd")?;
    Ok(cwd.join(path))
}

fn absolutize_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir().context("resolve path cwd")?;
    Ok(cwd.join(path))
}

fn read_docker_log_bytes(result: Result<Vec<u8>, TestcontainersError>) -> String {
    match result {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(err) => format!("<failed to read docker logs: {err}>"),
    }
}

async fn persist_docker_logs(
    config: &NodeConfig,
    container: &testcontainers::ContainerAsync<GenericImage>,
) -> Result<(PathBuf, PathBuf, String, String)> {
    std::fs::create_dir_all(&config.work_dir)?;

    let stdout = read_docker_log_bytes(container.stdout_to_vec().await);
    let stderr = read_docker_log_bytes(container.stderr_to_vec().await);

    let stdout_path = config.work_dir.join("stdout.log");
    let stderr_path = config.work_dir.join("stderr.log");
    persist_log_bytes(&stdout_path, &stderr_path, &stdout, &stderr)?;

    Ok((stdout_path, stderr_path, stdout, stderr))
}

fn persist_log_bytes(
    stdout_path: &Path,
    stderr_path: &Path,
    stdout: &str,
    stderr: &str,
) -> Result<()> {
    std::fs::write(stdout_path, stdout.as_bytes())?;
    std::fs::write(stderr_path, stderr.as_bytes())?;
    Ok(())
}

fn log_tail_preview(logs: &str, max_lines: usize) -> String {
    if max_lines == 0 {
        return String::new();
    }

    let lines: Vec<&str> = logs
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return String::new();
    }

    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

fn parse_socket_port(addr: &str) -> Result<u16> {
    let parsed: SocketAddr = addr
        .parse()
        .map_err(|err| anyhow!("invalid socket address '{addr}': {err}"))?;
    Ok(parsed.port())
}

fn extract_udp_port(value: &str) -> Option<u16> {
    let (_, rest) = value.split_once("/udp/")?;
    let port_str = rest.split('/').next()?;
    port_str.parse().ok()
}

fn resolve_p2p_ports(specs: &[NodeSpec], base_p2p_port: u16) -> Result<HashMap<String, u16>> {
    let mut assigned = HashMap::new();
    let mut used_ports = HashSet::new();

    for spec in specs {
        if let Some(bind) = spec.bind.first() {
            let port = extract_udp_port(bind)
                .ok_or_else(|| anyhow!("failed to parse udp port from bind '{bind}'"))?;
            if let Some(existing) = assigned.insert(spec.id.clone(), port) {
                return Err(anyhow!(
                    "node '{}' received conflicting p2p ports {} and {}", spec.id, existing, port
                ));
            }
            if !used_ports.insert(port) {
                let owner = specs
                    .iter()
                    .find(|candidate| {
                        candidate.id != spec.id && assigned.get(&candidate.id) == Some(&port)
                    })
                    .map(|candidate| candidate.id.as_str())
                    .unwrap_or("<unknown>");
                return Err(anyhow!(
                    "duplicate p2p port {} for nodes '{}' and '{}'", port, owner, spec.id
                ));
            }
        }
    }

    for (index, spec) in specs.iter().enumerate() {
        if assigned.contains_key(&spec.id) {
            continue;
        }

        let mut candidate = base_p2p_port + index as u16;
        while used_ports.contains(&candidate) {
            candidate = candidate
                .checked_add(1)
                .ok_or_else(|| anyhow!("ran out of p2p ports while assigning {}", spec.id))?;
        }

        used_ports.insert(candidate);
        assigned.insert(spec.id.clone(), candidate);
    }

    Ok(assigned)
}

fn build_p2p_port_map(configs: &HashMap<String, NodeConfig>) -> Result<HashMap<u16, String>> {
    let mut map = HashMap::new();
    for config in configs.values() {
        if let Some(existing) = map.insert(config.p2p_port, config.spec.id.clone()) {
            return Err(anyhow!(
                "duplicate p2p port {} for nodes '{}' and '{}'", config.p2p_port, existing,
                config.spec.id
            ));
        }
    }
    Ok(map)
}

fn format_docker_peer_addr(
    host_proto: &str,
    host: &str,
    port: u16,
    peer_id: Option<&str>,
) -> String {
    let mut addr = format!("/{host_proto}/{host}/udp/{port}/quic-v1");
    if let Some(peer_id) = peer_id {
        addr.push_str("/p2p/");
        addr.push_str(peer_id);
    }
    addr
}

fn rewrite_bind_for_docker(bind: &str) -> Result<String> {
    if let Some(rest) = bind.strip_prefix("/ip4/") {
        let mut parts = rest.splitn(2, '/');
        let _ = parts.next();
        let tail = parts.next().unwrap_or("");
        return Ok(format!("/ip4/0.0.0.0/{}", tail));
    }
    Ok(bind.to_string())
}

fn container_path_for(config: &NodeConfig, host_path: &Path) -> Result<String> {
    let rel = host_path.strip_prefix(&config.work_dir).map_err(|_| {
        anyhow!(
            "path {} is outside node data dir {}",
            host_path.display(),
            config.work_dir.display()
        )
    })?;
    Ok(Path::new(DOCKER_WORKDIR).join(rel).display().to_string())
}

struct ImageRef {
    name: String,
    tag: String,
}

fn head_info_from_private(head: PrivateHeadInfo) -> HeadInfo {
    HeadInfo {
        height: head.height,
        block_id: head.block_id.map(PbHash::from),
    }
}

fn resolve_docker_image(config: &NodeConfig, default_image: &str) -> Result<String> {
    if let Some(path) = &config.spec.binary {
        let expanded = expand_env_vars(&path.to_string_lossy())?;
        return Ok(expanded);
    }
    Ok(default_image.to_string())
}

fn parse_image_ref(image: &str) -> ImageRef {
    let last_slash = image.rfind('/').unwrap_or(0);
    let tag_split = image[last_slash..].rfind(':');
    if let Some(idx) = tag_split {
        let split_at = last_slash + idx;
        let (name, tag) = image.split_at(split_at);
        return ImageRef {
            name: name.to_string(),
            tag: tag.trim_start_matches(':').to_string(),
        };
    }
    ImageRef {
        name: image.to_string(),
        tag: "latest".to_string(),
    }
}

fn docker_container_name(network: &str, node_id: &str) -> String {
    format!("{}-{}", network, node_id)
}

fn create_docker_network(name: &str) -> Result<()> {
    let output = std::process::Command::new("docker")
        .args(["network", "create", name])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("already exists") {
            return Ok(());
        }
        return Err(anyhow!("docker network create failed: {}", stderr.trim()));
    }
    Ok(())
}

fn remove_docker_network(name: &str) -> Result<()> {
    let _ = std::process::Command::new("docker")
        .args(["network", "rm", name])
        .output();
    Ok(())
}

fn remove_stale_docker_container(name: &str) -> Result<()> {
    let output = std::process::Command::new("docker")
        .args(["rm", "-f", name])
        .output()?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("No such container") {
        return Ok(());
    }

    Err(anyhow!("docker rm -f {} failed: {}", name, stderr.trim()))
}

impl NodeManager {
    async fn rewrite_peers_for_docker(
        &self,
        spec: &NodeSpec,
        port_map: &HashMap<u16, String>,
        network: &str,
    ) -> Result<DockerPeers> {
        let mut peers = Vec::new();
        for peer in &spec.peers {
            peers.push(
                self.rewrite_peer_addr_for_docker(peer, port_map, network)
                    .await?,
            );
        }
        let mut force_peers = Vec::new();
        for peer in &spec.force_peers {
            force_peers.push(
                self.rewrite_peer_addr_for_docker(peer, port_map, network)
                    .await?,
            );
        }
        Ok(DockerPeers { peers, force_peers })
    }

    async fn rewrite_peer_addr_for_docker(
        &self,
        addr: &str,
        port_map: &HashMap<u16, String>,
        network: &str,
    ) -> Result<String> {
        let port = extract_udp_port(addr)
            .ok_or_else(|| anyhow!("failed to parse udp port from peer '{addr}'"))?;
        let node_id = port_map
            .get(&port)
            .ok_or_else(|| anyhow!("peer port {} not found in scenario", port))?;
        let peer_id = self.try_resolve_peer_id(node_id).await?;
        self.docker_peer_addr_for_node(node_id, port, network, peer_id.as_deref())
    }

    fn docker_peer_addr_for_node(
        &self,
        node_id: &str,
        port: u16,
        network: &str,
        peer_id: Option<&str>,
    ) -> Result<String> {
        if let Some(ip) = self.docker_node_ip(node_id, network)? {
            return Ok(format_docker_peer_addr("ip4", &ip, port, peer_id));
        }
        let container_name = docker_container_name(network, node_id);
        Ok(format_docker_peer_addr(
            "dns4", &container_name, port, peer_id,
        ))
    }

    fn docker_node_ip(&self, node_id: &str, network: &str) -> Result<Option<String>> {
        let Some(NodeHandle::Docker(_)) = self.handles.get(node_id) else {
            return Ok(None);
        };

        let container_name = docker_container_name(network, node_id);
        let format = format!(
            "{{{{with index .NetworkSettings.Networks {:?}}}}}{{{{.IPAddress}}}}{{{{end}}}}",
            network
        );
        let output = std::process::Command::new("docker")
            .args(["inspect", "--format", &format, &container_name])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("No such object") {
                return Ok(None);
            }
            return Err(anyhow!(
                "docker inspect for {} failed: {}",
                container_name,
                stderr.trim()
            ));
        }

        let ip = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if ip.is_empty() {
            return Ok(None);
        }
        Ok(Some(ip))
    }

    async fn try_resolve_peer_id(&self, node_id: &str) -> Result<Option<String>> {
        let config = self
            .configs
            .get(node_id)
            .ok_or_else(|| anyhow!("unknown node id '{node_id}'"))?;
        if let Some(peer_id) = read_peer_id_if_present(&config.work_dir)? {
            return Ok(Some(peer_id));
        }
        if self.handles.contains_key(node_id) {
            if let Ok(peer_id) = wait_for_peer_id(&config.work_dir).await {
                return Ok(Some(peer_id));
            }
        }
        Ok(None)
    }

    async fn resolve_peer_from(&self, spec: &NodeSpec) -> Result<Vec<String>> {
        let mut resolved = Vec::new();
        for peer_from in &spec.peer_from {
            resolved.push(self.resolve_peer_entry(peer_from).await?);
        }
        if !spec.new_state {
            for peer_from in &spec.restart_peer_from {
                if let Some(peer) = self.try_resolve_peer_entry(peer_from).await? {
                    resolved.push(peer);
                }
            }
        }
        Ok(resolved)
    }

    async fn try_resolve_peer_entry(&self, peer_from: &PeerFrom) -> Result<Option<String>> {
        let Some(peer_id) = self.try_resolve_peer_id(&peer_from.node).await? else {
            return Ok(None);
        };
        if let NodeMode::Docker { network, .. } = &self.mode {
            let port = extract_udp_port(&peer_from.listen).ok_or_else(|| {
                anyhow!("failed to parse udp port from peer_from listen '{}'", peer_from.listen)
            })?;
            return self
                .docker_peer_addr_for_node(&peer_from.node, port, network, Some(&peer_id))
                .map(Some);
        }
        Ok(Some(format!("{}/p2p/{}", peer_from.listen, peer_id)))
    }

    async fn resolve_peer_entry(&self, peer_from: &PeerFrom) -> Result<String> {
        let config = self
            .configs
            .get(&peer_from.node)
            .ok_or_else(|| anyhow!("unknown peer_from node '{}'", peer_from.node))?;
        let peer_id = wait_for_peer_id(&config.work_dir).await?;
        if let NodeMode::Docker { network, .. } = &self.mode {
            let port = extract_udp_port(&peer_from.listen).ok_or_else(|| {
                anyhow!("failed to parse udp port from peer_from listen '{}'", peer_from.listen)
            })?;
            return self.docker_peer_addr_for_node(&peer_from.node, port, network, Some(&peer_id));
        }
        Ok(format!("{}/p2p/{}", peer_from.listen, peer_id))
    }
}

async fn wait_for_peer_id(work_dir: &Path) -> Result<String> {
    let path = work_dir.join(PEER_ID_FILENAME);
    let deadline = Instant::now() + PEER_ID_WAIT_TIMEOUT;
    loop {
        match std::fs::read_to_string(&path) {
            Ok(contents) => return Ok(contents.trim().to_string()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                if Instant::now() >= deadline {
                    return Err(anyhow!("peer id file not found at {}", path.display()));
                }
            }
            Err(err) => return Err(err.into()),
        }
        sleep(Duration::from_millis(100)).await;
    }
}

fn read_peer_id_if_present(work_dir: &Path) -> Result<Option<String>> {
    let path = work_dir.join(PEER_ID_FILENAME);
    match std::fs::read_to_string(path) {
        Ok(peer_id) => Ok(Some(peer_id.trim().to_string())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn read_log_file(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err.into()),
    }
}

fn ensure_tcp_port_available(
    node_id: &str,
    port_kind: &str,
    addr: &str,
    guidance: &str,
) -> Result<()> {
    let listener = StdTcpListener::bind(addr).map_err(|err| {
        anyhow!(
            "node '{}' cannot start because {} port {} is unavailable: {} ({})", node_id,
            port_kind, addr, err, guidance
        )
    })?;
    drop(listener);
    Ok(())
}

fn ensure_udp_port_available(
    node_id: &str,
    port_kind: &str,
    port: u16,
    guidance: &str,
) -> Result<()> {
    let addr = ("127.0.0.1", port);
    let socket = StdUdpSocket::bind(addr).map_err(|err| {
        anyhow!(
            "node '{}' cannot start because {} port {} is unavailable: {} ({})", node_id,
            port_kind, port, err, guidance
        )
    })?;
    drop(socket);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::{TcpListener, UdpSocket};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use anyhow::Result;
    use tokio::time::{sleep, Duration};

    #[test]
    fn persist_log_bytes_writes_stdout_and_stderr() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("nockchain-e2e-node-tests-{nonce}"));
        std::fs::create_dir_all(&temp_dir).expect("create temp dir");
        let stdout_path = temp_dir.join("stdout.log");
        let stderr_path = temp_dir.join("stderr.log");

        crate::node::persist_log_bytes(&stdout_path, &stderr_path, "stdout body", "stderr body")
            .expect("persist log bytes");

        assert_eq!(
            std::fs::read_to_string(stdout_path).expect("read stdout"),
            "stdout body"
        );
        assert_eq!(
            std::fs::read_to_string(stderr_path).expect("read stderr"),
            "stderr body"
        );
        std::fs::remove_dir_all(temp_dir).expect("cleanup temp dir");
    }

    #[test]
    fn log_tail_preview_keeps_last_non_empty_lines() {
        let logs = "\nfirst\n\nsecond\nthird\n";

        let preview = crate::node::log_tail_preview(logs, 2);

        assert_eq!(preview, "second\nthird");
    }

    #[test]
    fn log_tail_preview_handles_empty_logs() {
        assert_eq!(crate::node::log_tail_preview("", 4), "");
    }

    #[test]
    fn node_args_keep_existing_peer_id() {
        let config = dummy_node_config();

        let args = crate::node::build_node_args(&config, &Vec::new()).expect("build process args");
        let docker_args = crate::node::build_node_args_docker(
            &config,
            &Vec::new(),
            &crate::node::DockerPeers {
                peers: Vec::new(),
                force_peers: Vec::new(),
            },
        )
        .expect("build docker args");

        assert!(args.iter().any(|arg| arg == "--no-new-peer-id"));
        assert!(docker_args.iter().any(|arg| arg == "--no-new-peer-id"));
    }

    #[test]
    fn node_args_use_initial_topology_peers_for_new_state() {
        let mut config = dummy_node_config();
        let peer_from = vec![String::from("/ip4/127.0.0.1/udp/4100/quic-v1/p2p/from")];
        config.spec.peers = vec![String::from("/ip4/127.0.0.1/udp/4101/quic-v1/p2p/static")];
        config.spec.force_peers = vec![String::from("/ip4/127.0.0.1/udp/4102/quic-v1/p2p/force")];

        let args = crate::node::build_node_args(&config, &peer_from).expect("build process args");
        let docker_args = crate::node::build_node_args_docker(
            &config,
            &peer_from,
            &crate::node::DockerPeers {
                peers: vec![String::from("/dns4/static/udp/4101/quic-v1/p2p/static")],
                force_peers: vec![String::from("/dns4/force/udp/4102/quic-v1/p2p/force")],
            },
        )
        .expect("build docker args");

        assert_eq!(
            flag_values(&args, "--peer"),
            vec![
                "/ip4/127.0.0.1/udp/4100/quic-v1/p2p/from",
                "/ip4/127.0.0.1/udp/4101/quic-v1/p2p/static",
            ]
        );
        assert_eq!(
            flag_values(&args, "--force-peer"),
            vec!["/ip4/127.0.0.1/udp/4102/quic-v1/p2p/force"]
        );
        assert_eq!(
            flag_values(&docker_args, "--peer"),
            vec![
                "/ip4/127.0.0.1/udp/4100/quic-v1/p2p/from",
                "/dns4/static/udp/4101/quic-v1/p2p/static",
            ]
        );
        assert_eq!(
            flag_values(&docker_args, "--force-peer"),
            vec!["/dns4/force/udp/4102/quic-v1/p2p/force"]
        );
    }

    #[test]
    fn node_args_use_force_topology_peers_after_restart() {
        let mut config = dummy_node_config();
        config.spec.new_state = false;
        let peer_from = vec![String::from("/ip4/127.0.0.1/udp/4100/quic-v1/p2p/from")];
        config.spec.peers = vec![String::from("/ip4/127.0.0.1/udp/4101/quic-v1/p2p/static")];
        config.spec.force_peers = vec![String::from("/ip4/127.0.0.1/udp/4102/quic-v1/p2p/force")];

        let args = crate::node::build_node_args(&config, &peer_from).expect("build process args");
        let docker_args = crate::node::build_node_args_docker(
            &config,
            &peer_from,
            &crate::node::DockerPeers {
                peers: vec![String::from("/dns4/static/udp/4101/quic-v1/p2p/static")],
                force_peers: vec![String::from("/dns4/force/udp/4102/quic-v1/p2p/force")],
            },
        )
        .expect("build docker args");

        assert!(flag_values(&args, "--peer").is_empty());
        assert_eq!(
            flag_values(&args, "--force-peer"),
            vec![
                "/ip4/127.0.0.1/udp/4100/quic-v1/p2p/from",
                "/ip4/127.0.0.1/udp/4101/quic-v1/p2p/static",
                "/ip4/127.0.0.1/udp/4102/quic-v1/p2p/force",
            ]
        );
        assert!(flag_values(&docker_args, "--peer").is_empty());
        assert_eq!(
            flag_values(&docker_args, "--force-peer"),
            vec![
                "/ip4/127.0.0.1/udp/4100/quic-v1/p2p/from",
                "/dns4/static/udp/4101/quic-v1/p2p/static",
                "/dns4/force/udp/4102/quic-v1/p2p/force",
            ]
        );
    }

    #[test]
    fn format_docker_peer_addr_appends_peer_id_when_present() {
        let addr =
            crate::node::format_docker_peer_addr("ip4", "172.18.0.2", 4801, Some("peer-123"));

        assert_eq!(addr, "/ip4/172.18.0.2/udp/4801/quic-v1/p2p/peer-123");
    }

    #[test]
    fn format_docker_peer_addr_omits_peer_id_when_absent() {
        let addr = crate::node::format_docker_peer_addr("dns4", "nockchain-e2e-node-a", 4801, None);

        assert_eq!(addr, "/dns4/nockchain-e2e-node-a/udp/4801/quic-v1");
    }

    #[test]
    fn resolve_p2p_ports_skips_explicit_bind_ports_for_fallback_nodes() {
        let specs = vec![
            nockchain_testkit::NodeSpec {
                id: "node-a".to_string(),
                bind: vec!["/ip4/127.0.0.1/udp/4101/quic-v1".to_string()],
                ..dummy_node_spec("node-a")
            },
            nockchain_testkit::NodeSpec {
                id: "node-b".to_string(),
                ..dummy_node_spec("node-b")
            },
        ];

        let ports = crate::node::resolve_p2p_ports(&specs, 4100).expect("resolve p2p ports");

        assert_eq!(ports.get("node-a"), Some(&4101));
        assert_eq!(ports.get("node-b"), Some(&4102));
    }

    #[test]
    fn resolve_p2p_ports_rejects_duplicate_explicit_bind_ports() {
        let specs = vec![
            nockchain_testkit::NodeSpec {
                id: "node-a".to_string(),
                bind: vec!["/ip4/127.0.0.1/udp/4101/quic-v1".to_string()],
                ..dummy_node_spec("node-a")
            },
            nockchain_testkit::NodeSpec {
                id: "node-b".to_string(),
                bind: vec!["/ip4/127.0.0.1/udp/4101/quic-v1".to_string()],
                ..dummy_node_spec("node-b")
            },
        ];

        let err =
            crate::node::resolve_p2p_ports(&specs, 4100).expect_err("duplicate bind should fail");

        assert!(err
            .to_string()
            .contains("duplicate p2p port 4101 for nodes 'node-a' and 'node-b'"));
    }

    #[test]
    fn set_node_env_updates_stopped_node_config() {
        let config = dummy_node_config();
        let mut manager = crate::node::NodeManager {
            mode: crate::node::NodeMode::Process,
            configs: HashMap::from([(String::from("node-a"), config)]),
            handles: HashMap::new(),
        };

        manager
            .set_node_env(
                "node-a",
                String::from("NOCKCHAIN_LIBP2P_REQ_RES_GEN2_SEND_ENABLED"),
                String::from("true"),
            )
            .expect("set env should succeed");

        let stored = manager
            .configs
            .get("node-a")
            .and_then(|entry| {
                entry
                    .spec
                    .env
                    .get("NOCKCHAIN_LIBP2P_REQ_RES_GEN2_SEND_ENABLED")
            })
            .map(String::as_str);
        assert_eq!(stored, Some("true"));
    }

    #[tokio::test]
    async fn start_nodes_fails_when_public_grpc_port_is_occupied() -> Result<()> {
        let busy_public = TcpListener::bind("127.0.0.1:0")?;
        let addr = busy_public.local_addr()?.to_string();

        let err = crate::node::ensure_tcp_port_available(
            "node-a", "public gRPC", &addr,
            "choose a different --base-grpc-port block or stop the stale process",
        )
        .expect_err("occupied public gRPC port should fail before spawn");

        assert!(err.to_string().contains("public gRPC port"));
        assert!(err.to_string().contains("base-grpc-port"));
        Ok(())
    }

    #[tokio::test]
    async fn start_nodes_fails_when_private_grpc_port_is_occupied() -> Result<()> {
        let busy_private = TcpListener::bind("127.0.0.1:0")?;
        let addr = busy_private.local_addr()?.to_string();

        let err = crate::node::ensure_tcp_port_available(
            "node-a", "private gRPC", &addr,
            "choose a different --base-private-grpc-port block or stop the stale process",
        )
        .expect_err("occupied private gRPC port should fail before spawn");

        assert!(err.to_string().contains("private gRPC port"));
        assert!(err.to_string().contains("base-private-grpc-port"));
        Ok(())
    }

    #[tokio::test]
    async fn start_nodes_fails_when_p2p_port_is_occupied() -> Result<()> {
        let busy_p2p = UdpSocket::bind("127.0.0.1:0")?;
        let port = busy_p2p.local_addr()?.port();

        let err = crate::node::ensure_udp_port_available(
            "node-a", "p2p", port,
            "choose a different --base-p2p-port block or stop the stale process",
        )
        .expect_err("occupied p2p port should fail before spawn");

        assert!(err.to_string().contains("p2p port"));
        assert!(err.to_string().contains("base-p2p-port"));
        Ok(())
    }

    #[tokio::test]
    async fn fetch_private_head_if_current_skips_unowned_node_ports() -> Result<()> {
        let busy_private = TcpListener::bind("127.0.0.1:0")?;
        let mut config = dummy_node_config();
        config.grpc_private_port = busy_private.local_addr()?.port();
        config.grpc_private_addr = format!("127.0.0.1:{}", config.grpc_private_port);
        let mut manager = crate::node::NodeManager {
            mode: crate::node::NodeMode::Process,
            configs: HashMap::from([(String::from("node-a"), config)]),
            handles: HashMap::new(),
        };

        let head = manager.fetch_private_head_if_current("node-a").await?;
        assert!(head.is_none(), "unowned node ports should not be queried");
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fetch_private_head_if_current_skips_dead_process_handles() -> Result<()> {
        let busy_private = TcpListener::bind("127.0.0.1:0")?;
        let mut config = dummy_node_config();
        config.grpc_private_port = busy_private.local_addr()?.port();
        config.grpc_private_addr = format!("127.0.0.1:{}", config.grpc_private_port);
        let child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .spawn()?;
        let mut manager = crate::node::NodeManager {
            mode: crate::node::NodeMode::Process,
            configs: HashMap::from([(String::from("node-a"), config)]),
            handles: HashMap::from([(
                String::from("node-a"),
                crate::node::NodeHandle::Process(crate::node::ProcessHandle { child }),
            )]),
        };

        sleep(Duration::from_millis(25)).await;

        let head = manager.fetch_private_head_if_current("node-a").await?;
        assert!(
            head.is_none(),
            "dead process handles should suppress final summaries"
        );
        Ok(())
    }

    #[tokio::test]
    async fn resolve_peer_from_adds_restart_peers_after_peer_id_exists() -> Result<()> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("nockchain-e2e-peer-from-{nonce}"));
        let initial_dir = temp_dir.join("initial");
        let restart_dir = temp_dir.join("restart");
        std::fs::create_dir_all(&initial_dir)?;
        std::fs::create_dir_all(&restart_dir)?;
        std::fs::write(
            initial_dir.join(crate::node::PEER_ID_FILENAME),
            "initial-peer\n",
        )?;

        let mut initial = dummy_node_config();
        initial.spec.id = String::from("initial");
        initial.base_spec.id = String::from("initial");
        initial.work_dir = initial_dir;
        let mut restart = dummy_node_config();
        restart.spec.id = String::from("restart");
        restart.base_spec.id = String::from("restart");
        restart.work_dir = restart_dir.clone();

        let manager = crate::node::NodeManager {
            mode: crate::node::NodeMode::Process,
            configs: HashMap::from([
                (String::from("initial"), initial),
                (String::from("restart"), restart),
            ]),
            handles: HashMap::new(),
        };
        let mut spec = dummy_node_spec("node-a");
        spec.peer_from = vec![nockchain_testkit::PeerFrom {
            node: String::from("initial"),
            listen: String::from("/ip4/127.0.0.1/udp/4100/quic-v1"),
        }];
        spec.restart_peer_from = vec![nockchain_testkit::PeerFrom {
            node: String::from("restart"),
            listen: String::from("/ip4/127.0.0.1/udp/4101/quic-v1"),
        }];

        let first_start = manager.resolve_peer_from(&spec).await?;
        spec.new_state = false;
        let restart_before_peer_id = manager.resolve_peer_from(&spec).await?;
        std::fs::write(
            restart_dir.join(crate::node::PEER_ID_FILENAME),
            "restart-peer\n",
        )?;
        let restarted = manager.resolve_peer_from(&spec).await?;

        assert_eq!(
            first_start,
            vec!["/ip4/127.0.0.1/udp/4100/quic-v1/p2p/initial-peer"]
        );
        assert_eq!(
            restart_before_peer_id,
            vec!["/ip4/127.0.0.1/udp/4100/quic-v1/p2p/initial-peer"]
        );
        assert_eq!(
            restarted,
            vec![
                "/ip4/127.0.0.1/udp/4100/quic-v1/p2p/initial-peer",
                "/ip4/127.0.0.1/udp/4101/quic-v1/p2p/restart-peer",
            ]
        );

        std::fs::remove_dir_all(temp_dir).expect("cleanup temp dir");
        Ok(())
    }

    #[test]
    fn peer_id_wait_timeout_covers_slow_node_startup() {
        assert!(crate::node::PEER_ID_WAIT_TIMEOUT >= Duration::from_secs(120));
    }

    fn dummy_node_spec(id: &str) -> nockchain_testkit::NodeSpec {
        nockchain_testkit::NodeSpec {
            id: id.to_string(),
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
            new_state: true,
            no_default_peers: false,
            allowed_peers_path: None,
            fakenet_pow_len: None,
            fakenet_log_difficulty: None,
            fakenet_v1_phase: None,
            fakenet_bythos_phase: None,
            fakenet_update_candidate_interval_secs: None,
            fakenet_genesis_jam_path: None,
            extra_args: Vec::new(),
            env: Default::default(),
            binary: None,
        }
    }

    fn dummy_node_config() -> crate::node::NodeConfig {
        let spec = dummy_node_spec("node-a");
        let grpc_public_port = allocate_tcp_port();
        let mut grpc_private_port = allocate_tcp_port();
        while grpc_private_port == grpc_public_port {
            grpc_private_port = allocate_tcp_port();
        }
        let mut p2p_port = allocate_udp_port();
        while p2p_port == grpc_public_port || p2p_port == grpc_private_port {
            p2p_port = allocate_udp_port();
        }
        crate::node::NodeConfig {
            base_spec: spec.clone(),
            spec,
            grpc_public_addr: format!("127.0.0.1:{grpc_public_port}"),
            grpc_public_port,
            grpc_private_addr: format!("127.0.0.1:{grpc_private_port}"),
            grpc_private_port,
            grpc_private_host_port: None,
            p2p_port,
            work_dir: std::env::temp_dir().join("nockchain-e2e-node-args-test"),
            binary: PathBuf::from("nockchain"),
            supports_bythos_phase: true,
        }
    }

    fn flag_values<'a>(args: &'a [String], flag: &str) -> Vec<&'a str> {
        let mut values = Vec::new();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            if arg == flag {
                values.push(iter.next().expect("flag value").as_str());
            }
        }
        values
    }

    fn allocate_tcp_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .expect("allocate test tcp port")
            .local_addr()
            .expect("read test tcp addr")
            .port()
    }

    fn allocate_udp_port() -> u16 {
        UdpSocket::bind("127.0.0.1:0")
            .expect("allocate test udp port")
            .local_addr()
            .expect("read test udp addr")
            .port()
    }
}
