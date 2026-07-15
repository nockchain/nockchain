use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use honk::workspace::WorkspaceConfig;
use honk_grpc::{bind_loopback, DaemonConfig, HonkServer, PROTOCOL_NAME, PROTOCOL_VERSION};
use honk_grpc_proto::v1::honk_compiler_client::HonkCompilerClient;
use honk_grpc_proto::v1::{ArtifactMode, CompileRequest, DiagnosticKind, GetServerInfoRequest};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::sync::oneshot;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::HealthCheckRequest;

const AURAS_SHA256: &str = "8535630fa4fd1464ecc398ab4d8882ed122b8e8e12435d103518faf64096d378";

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("honk-grpc crate should be below the repository root")
        .to_path_buf()
}

async fn connect_compiler(address: SocketAddr) -> HonkCompilerClient<tonic::transport::Channel> {
    let endpoint = format!("http://{address}");
    for _ in 0..40 {
        if let Ok(client) = HonkCompilerClient::connect(endpoint.clone()).await {
            return client.max_decoding_message_size(256 * 1024 * 1024);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("failed to connect to honkd at {address}");
}

fn request(request_id: &str, entry: &Path) -> CompileRequest {
    CompileRequest {
        protocol_version: PROTOCOL_VERSION,
        request_id: request_id.to_string(),
        entry_path: entry.display().to_string(),
        artifact_mode: ArtifactMode::Arbitrary as i32,
        directory_files: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_compile_matches_golden_and_revalidates_dependencies() {
    let root = repository_root();
    let temp = TempDir::new().expect("temporary workspace");
    let lib = temp.path().join("lib");
    fs::create_dir_all(&lib).expect("library directory");
    let helper = lib.join("helper.hoon");
    let entry = temp.path().join("demo.hoon");
    let malformed = temp.path().join("malformed.hoon");
    fs::write(&helper, "|=  [a=@ b=@]\n  (add a b)\n").expect("helper source");
    fs::write(&entry, "/+  helper\n|=  [a=@ b=@]\n  (helper a b)\n").expect("entry source");
    fs::write(&malformed, "|=  [a=@\n").expect("malformed source");

    let listener = bind_loopback("127.0.0.1:0".parse().expect("loopback address"))
        .await
        .expect("bind honkd");
    let address = listener.local_addr().expect("bound address");
    let server = HonkServer::new(DaemonConfig {
        workspace: WorkspaceConfig {
            prelude: root.join("hoon/common/hoon.hoon"),
            dependencies: temp.path().to_path_buf(),
            subject_type_jam: None,
            dbug: true,
            vet: true,
        },
        max_compiles: 6,
        worker_stack_bytes: honk_grpc::DEFAULT_WORKER_STACK_BYTES,
    })
    .expect("initialize honkd");
    let (shutdown_sender, shutdown) = oneshot::channel::<()>();
    let server_task = tokio::spawn(server.serve(listener, async move {
        let _ = shutdown.await;
    }));

    let mut client = connect_compiler(address).await;
    let info = client
        .get_server_info(GetServerInfoRequest {})
        .await
        .expect("server info")
        .into_inner();
    assert_eq!(info.protocol_name, PROTOCOL_NAME);
    assert_eq!(info.protocol_version, PROTOCOL_VERSION);
    assert_eq!(info.max_compiles, 6);

    let health_channel = tonic::transport::Endpoint::from_shared(format!("http://{address}"))
        .expect("health endpoint")
        .connect()
        .await
        .expect("health channel");
    let mut health = HealthClient::new(health_channel);
    let health = health
        .check(HealthCheckRequest {
            service: "honk.compiler.v1.HonkCompiler".to_string(),
        })
        .await
        .expect("health check")
        .into_inner();
    assert_eq!(health.status, ServingStatus::Serving as i32);

    let mut incompatible = request("wrong-version", &entry);
    incompatible.protocol_version = PROTOCOL_VERSION + 1;
    let status = client
        .compile(incompatible)
        .await
        .expect_err("version mismatch should be rejected");
    assert_eq!(status.code(), tonic::Code::FailedPrecondition);

    let auras = root.join("crates/honk/test-assets/type-probes/auras.hoon");
    let golden = client
        .compile(request("golden", &auras))
        .await
        .expect("golden compile")
        .into_inner();
    assert!(golden.diagnostics.is_empty());
    assert!(!golden.cache_invalidated);
    assert_eq!(golden.compile_index, 1);
    assert_eq!(
        format!("{:x}", Sha256::digest(&golden.artifact)),
        AURAS_SHA256
    );

    let first = client
        .compile(request("first", &entry))
        .await
        .expect("first dependency compile")
        .into_inner();
    assert!(first.diagnostics.is_empty());
    assert!(!first.cache_invalidated);
    assert_eq!(first.compile_index, 2);

    let unchanged = client
        .compile(request("unchanged", &entry))
        .await
        .expect("unchanged dependency compile")
        .into_inner();
    assert_eq!(unchanged.artifact, first.artifact);
    assert!(!unchanged.cache_invalidated);
    assert_eq!(unchanged.compile_index, 3);

    fs::write(temp.path().join("new-unrelated.hoon"), "42\n").expect("new dependency-layout entry");
    let layout_changed = client
        .compile(request("layout-changed", &entry))
        .await
        .expect("dependency layout compile")
        .into_inner();
    assert!(layout_changed.diagnostics.is_empty());
    assert!(layout_changed.cache_invalidated);
    assert_eq!(layout_changed.artifact, first.artifact);
    assert_eq!(layout_changed.compile_index, 4);

    fs::write(&helper, "|=  [a=@ b=@]\n  (mul a b)\n").expect("changed helper source");
    let changed = client
        .compile(request("changed", &entry))
        .await
        .expect("changed dependency compile")
        .into_inner();
    assert!(changed.diagnostics.is_empty());
    assert!(changed.cache_invalidated);
    assert_ne!(changed.artifact, first.artifact);
    assert_eq!(changed.compile_index, 5);

    let diagnostic = client
        .compile(request("diagnostic", &malformed))
        .await
        .expect("malformed compile response")
        .into_inner();
    assert!(diagnostic.artifact.is_empty());
    assert_eq!(diagnostic.diagnostics.len(), 1);
    assert_eq!(diagnostic.diagnostics[0].kind, DiagnosticKind::Parse as i32);
    assert!(diagnostic.diagnostics[0].location.is_some());
    assert_eq!(diagnostic.compile_index, 6);
    assert!(diagnostic.restart_required);

    let _ = shutdown_sender.send(());
    tokio::time::timeout(Duration::from_secs(30), server_task)
        .await
        .expect("server should shut down")
        .expect("server task should join")
        .expect("server should exit cleanly");
}
