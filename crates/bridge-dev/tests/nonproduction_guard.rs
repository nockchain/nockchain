use std::net::TcpListener as StdTcpListener;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use bridge::shared::e2e_environment::{
    BaseSepoliaE2eManifest, BASE_SEPOLIA_E2E_CHAIN_ID, BASE_SEPOLIA_E2E_ENVIRONMENT_ID,
};
use bridge_dev::base_backend::BaseBackend;
use bridge_dev::nonproduction_guard::{
    LoopbackBaseRpcUrl, NonproductionGuard, NonproductionGuardError,
};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tokio::time::{sleep, Instant};

const MANIFEST_JSON: &str = include_str!("../../bridge/e2e/environments/base-sepolia.json");
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

fn manifest() -> BaseSepoliaE2eManifest {
    BaseSepoliaE2eManifest::from_json(MANIFEST_JSON)
        .expect("checked-in Base Sepolia E2E manifest must validate")
}

#[test]
fn endpoint_parser_accepts_only_literal_loopback_http() {
    for endpoint in [
        "http://127.0.0.1:8545", "http://127.9.8.7:1", "http://localhost:8545", "http://[::1]:8545",
    ] {
        let result = LoopbackBaseRpcUrl::parse(endpoint);
        assert!(result.is_ok(), "{endpoint} should pass: {result:?}");
    }

    for endpoint in [
        "", "not a URL", "https://127.0.0.1:8545", "ws://127.0.0.1:8545", "http://localhost",
        "http://u:p@127.0.0.1:8545", "http://127.0.0.1:8545?mode=test",
        "http://127.0.0.1:8545/#fragment", "http://127.0.0.1:8545/rpc",
        "http://localhost.example.com:8545", "http://192.0.2.10:8545",
        "http://sepolia.base.org:8545", "http://base-sepolia.gateway.tenderly.co:8545",
    ] {
        assert!(
            LoopbackBaseRpcUrl::parse(endpoint).is_err(),
            "{endpoint} must be rejected"
        );
    }
}

#[tokio::test]
async fn unsafe_endpoint_and_profile_fail_before_any_rpc_request() -> Result<()> {
    let server = FakeRpcServer::start(FakeMode::Chain(BASE_SEPOLIA_E2E_CHAIN_ID)).await?;
    let manifest = manifest();

    for endpoint in [
        format!("{}?mode=test", server.endpoint),
        format!("{}/rpc", server.endpoint),
        server.endpoint.replacen("http://", "http://u:p@", 1),
    ] {
        assert!(
            NonproductionGuard::acquire(&endpoint, BASE_SEPOLIA_E2E_ENVIRONMENT_ID, &manifest)
                .await
                .is_err()
        );
    }
    assert_eq!(server.requests(), 0, "static rejection must not send RPC");

    let error = NonproductionGuard::acquire(&server.endpoint, "another-environment", &manifest)
        .await
        .expect_err("profile mismatch must fail");
    assert!(matches!(
        error,
        NonproductionGuardError::EnvironmentMismatch { .. }
    ));
    assert_eq!(server.requests(), 0, "profile rejection must not send RPC");
    Ok(())
}

#[tokio::test]
async fn live_and_wrong_chain_ids_fail_closed() -> Result<()> {
    for (chain_id, live) in [(8_453, true), (84_532, true), (31_337, false)] {
        let server = FakeRpcServer::start(FakeMode::Chain(chain_id)).await?;
        let error = NonproductionGuard::acquire(
            &server.endpoint,
            BASE_SEPOLIA_E2E_ENVIRONMENT_ID,
            &manifest(),
        )
        .await
        .expect_err("wrong chain id must fail");
        if live {
            assert!(matches!(error, NonproductionGuardError::LiveBaseChain(id) if id == chain_id));
        } else {
            assert!(matches!(
                error,
                NonproductionGuardError::ChainIdMismatch {
                    expected: BASE_SEPOLIA_E2E_CHAIN_ID,
                    observed: 31_337
                }
            ));
        }
    }
    Ok(())
}

#[tokio::test]
async fn missing_anvil_capability_is_rejected() -> Result<()> {
    let server = FakeRpcServer::start(FakeMode::MissingNodeInfo).await?;
    let error = NonproductionGuard::acquire(
        &server.endpoint,
        BASE_SEPOLIA_E2E_ENVIRONMENT_ID,
        &manifest(),
    )
    .await
    .expect_err("missing anvil_nodeInfo must fail");
    assert!(matches!(
        error,
        NonproductionGuardError::JsonRpc {
            method: "anvil_nodeInfo",
            code: -32601
        }
    ));
    Ok(())
}

#[tokio::test]
async fn hard_coded_anvil_responses_fail_snapshot_proof() -> Result<()> {
    let server = FakeRpcServer::start(FakeMode::HardCodedAnvil).await?;
    let error = NonproductionGuard::acquire(
        &server.endpoint,
        BASE_SEPOLIA_E2E_ENVIRONMENT_ID,
        &manifest(),
    )
    .await
    .expect_err("hard-coded snapshot id must fail");
    assert!(matches!(error, NonproductionGuardError::Capability(_)));
    Ok(())
}

#[tokio::test]
async fn failed_guard_never_reaches_mock_mutation_sender() -> Result<()> {
    let server = FakeRpcServer::start(FakeMode::Chain(8_453)).await?;
    let sends = AtomicUsize::new(0);
    let guard = NonproductionGuard::acquire(
        &server.endpoint,
        BASE_SEPOLIA_E2E_ENVIRONMENT_ID,
        &manifest(),
    )
    .await;

    if let Ok(guarded_rpc) = guard {
        let _backend = BaseBackend::new(guarded_rpc)?;
        sends.fetch_add(1, Ordering::SeqCst);
    }
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn real_loopback_anvil_acquires_guard_and_backend() -> Result<()> {
    let mut anvil = TestAnvil::start(BASE_SEPOLIA_E2E_CHAIN_ID).await?;
    let guarded = NonproductionGuard::acquire(
        &anvil.endpoint,
        BASE_SEPOLIA_E2E_ENVIRONMENT_ID,
        &manifest(),
    )
    .await?;
    let backend = BaseBackend::new(guarded)?;

    assert_eq!(
        backend.guarded_rpc().capabilities().chain_id,
        BASE_SEPOLIA_E2E_CHAIN_ID
    );
    assert!(backend
        .guarded_rpc()
        .capabilities()
        .client_version
        .starts_with("anvil/"));
    assert!(backend.guarded_rpc().capabilities().snapshot_round_trip);
    assert_eq!(
        backend.guarded_rpc().manifest().environment_id,
        BASE_SEPOLIA_E2E_ENVIRONMENT_ID
    );

    anvil.shutdown().await
}

#[derive(Clone, Copy)]
enum FakeMode {
    Chain(u64),
    MissingNodeInfo,
    HardCodedAnvil,
}

struct FakeRpcServer {
    endpoint: String,
    request_count: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl FakeRpcServer {
    async fn start(mode: FakeMode) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let request_count = Arc::new(AtomicUsize::new(0));
        let task_count = request_count.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                task_count.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(handle_fake_connection(stream, mode));
            }
        });
        Ok(Self {
            endpoint,
            request_count,
            task,
        })
    }

    fn requests(&self) -> usize {
        self.request_count.load(Ordering::SeqCst)
    }
}

impl Drop for FakeRpcServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn handle_fake_connection(mut stream: TcpStream, mode: FakeMode) {
    let Ok(request) = read_http_json(&mut stream).await else {
        return;
    };
    let id = request.get("id").and_then(Value::as_u64).unwrap_or(0);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let response = fake_rpc_response(mode, id, method);
    let body = response.to_string();
    let headers = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(headers.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
}

fn fake_rpc_response(mode: FakeMode, id: u64, method: &str) -> Value {
    let chain_id = match mode {
        FakeMode::Chain(chain_id) => chain_id,
        FakeMode::MissingNodeInfo | FakeMode::HardCodedAnvil => BASE_SEPOLIA_E2E_CHAIN_ID,
    };
    if method == "anvil_nodeInfo" && matches!(mode, FakeMode::MissingNodeInfo) {
        return json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": "method not found" }
        });
    }
    let result = match method {
        "eth_chainId" => json!(format!("0x{chain_id:x}")),
        "web3_clientVersion" => json!("anvil/v1.5.0"),
        "anvil_nodeInfo" => json!({
            "currentBlockNumber": "0x0",
            "environment": { "chainId": chain_id }
        }),
        "evm_snapshot" => json!("0x0"),
        "evm_revert" => json!(true),
        _ => Value::Null,
    };
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

async fn read_http_json(stream: &mut TcpStream) -> Result<Value> {
    let mut buffer = Vec::with_capacity(2048);
    let header_end = loop {
        let mut chunk = [0u8; 1024];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(anyhow!("connection closed before headers"));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = find_bytes(&buffer, b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = std::str::from_utf8(&buffer[..header_end])?;
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .context("missing content-length")?;
    while buffer.len() < header_end + content_length {
        let mut chunk = [0u8; 1024];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(anyhow!("connection closed before body"));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    Ok(serde_json::from_slice(
        &buffer[header_end..header_end + content_length],
    )?)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

struct TestAnvil {
    endpoint: String,
    child: Child,
}

impl TestAnvil {
    async fn start(chain_id: u64) -> Result<Self> {
        let port = reserve_port()?;
        let anvil_bin = std::env::var_os("ANVIL_BIN").unwrap_or_else(|| "anvil".into());
        let mut child = Command::new(anvil_bin)
            .args(["--silent", "--port", &port.to_string(), "--chain-id", &chain_id.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("starting real Anvil process")?;
        let endpoint = format!("http://127.0.0.1:{port}");
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                break;
            }
            if let Some(status) = child.try_wait()? {
                return Err(anyhow!("Anvil exited before readiness: {status}"));
            }
            if Instant::now() >= deadline {
                return Err(anyhow!("Anvil did not listen before timeout"));
            }
            sleep(Duration::from_millis(50)).await;
        }
        Ok(Self { endpoint, child })
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.child.kill().await.context("stopping Anvil")?;
        let _ = self.child.wait().await?;
        Ok(())
    }
}

impl Drop for TestAnvil {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn reserve_port() -> Result<u16> {
    let listener = StdTcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}
