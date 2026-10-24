//! The bridge-dev node executable must reject private API policy before native
//! consensus preparation, sequencer configuration, or external startup.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const OPERATOR: &str = "0123456789abcdef0123456789abcdef";
const MINER: &str = "abcdef0123456789abcdef0123456789";

fn rejected_startup(env: &[(&str, &str)], args: &[&str], expected: &str) {
    let dir = tempfile::tempdir().expect("temporary sequencer directory");
    let data = dir.path().join("kernel-data");
    let identity = dir.path().join("peer-identity");
    let mut command = Command::new(env!("CARGO_BIN_EXE_nockchain-bridge-sequencer"));
    command
        .current_dir(dir.path())
        .env("NOCKAPP_HOME", dir.path())
        .env_remove("NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN")
        .env_remove("NOCKCHAIN_PRIVATE_GRPC_MINER_TOKEN")
        .env_remove("NOCKCHAIN_ALLOW_UNAUTHENTICATED_PRIVATE_GRPC")
        .arg("--data-dir")
        .arg(&data)
        .arg("--identity-path")
        .arg(&identity)
        .args([
            "--bind-public-grpc-addr", "127.0.0.1:0", "--base-ws-url", "ws://127.0.0.1:1",
            "--sequencer-config-path", "intentionally-missing-sequencer.toml",
        ])
        .args(args)
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("start sequencer node");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child
            .try_wait()
            .expect("poll sequencer node exit")
            .is_some()
        {
            break;
        }
        if Instant::now() >= deadline {
            child
                .kill()
                .expect("kill node that failed to reject startup");
            let _ = child.wait();
            panic!("invalid private gRPC policy reached expensive or external sequencer startup");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().expect("collect sequencer exit");
    assert!(!output.status.success(), "invalid policy must fail startup");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(expected),
        "unexpected startup error: {stderr}"
    );
    for (name, token) in env {
        if name.ends_with("_TOKEN") {
            assert!(!stderr.contains(token), "startup error leaked a credential");
            assert!(
                !String::from_utf8_lossy(&output.stdout).contains(token),
                "startup output leaked a credential",
            );
        }
    }
    assert!(!data.exists(), "policy failure created kernel state");
    assert!(!identity.exists(), "policy failure created peer identity");
    assert!(
        !dir.path().join("nockchain").exists(),
        "policy failure created sequencer storage",
    );
}

#[test]
fn private_auth_failure_precedes_sequencer_dependencies() {
    rejected_startup(&[], &[], "private gRPC requires");
    rejected_startup(
        &[("NOCKCHAIN_ALLOW_UNAUTHENTICATED_PRIVATE_GRPC", "0")],
        &[],
        "private gRPC requires",
    );
    for args in [&[][..], &["--allow-unauthenticated-private-grpc"][..]] {
        rejected_startup(
            &[("NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN", "short")],
            args,
            "must be at least 32 characters",
        );
        for (operator, miner) in
            [(Some(OPERATOR), OPERATOR), (Some(OPERATOR), "short"), (None, MINER)]
        {
            let mut env = vec![("NOCKCHAIN_PRIVATE_GRPC_MINER_TOKEN", miner)];
            if let Some(operator) = operator {
                env.push(("NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN", operator));
            }
            rejected_startup(&env, args, "requires distinct operator and miner tokens");
        }
    }
    for addr in ["0.0.0.0:5555", "[::]:5555"] {
        rejected_startup(
            &[("NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN", OPERATOR)],
            &["--bind-private-grpc-addr", addr],
            "private gRPC must bind loopback",
        );
        rejected_startup(
            &[],
            &["--allow-unauthenticated-private-grpc", "--bind-private-grpc-addr", addr],
            "private gRPC must bind loopback",
        );
    }
}
