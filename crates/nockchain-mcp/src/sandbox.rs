use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::backend::Backend;
use crate::catalog::{catalog, OutputFormat};
use crate::process::{rust_script_command, terminate_process_group};

pub const DEFAULT_MAX_CODE_BYTES: usize = 32 * 1024;
pub const DEFAULT_MAX_RESULT_BYTES: usize = 1024 * 1024;
pub const DEFAULT_MAX_CALLS: usize = 32;
const PROTOCOL: &str = "nockchain-mcp-rust-v1";
const RUST_RUNTIME: &str = include_str!("rust_runtime.rs.template");

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum SandboxKind {
    Search,
    Execute,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestOptions {
    operation: String,
    #[serde(default = "empty_object")]
    input: Value,
    #[serde(default)]
    format: OutputFormat,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExplainOptions {
    operation: String,
    #[serde(default = "empty_object")]
    input: Value,
}

#[derive(Debug, Deserialize)]
struct ScriptFrame {
    protocol: String,
    kind: String,
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    value: Value,
    #[serde(default)]
    error: Option<String>,
}

fn empty_object() -> Value {
    json!({})
}

#[allow(clippy::too_many_arguments)]
pub async fn run_code(
    kind: SandboxKind,
    backend: Backend,
    code: &str,
    rust_script: &Path,
    max_calls: usize,
    max_result_bytes: usize,
    process_timeout: Duration,
    memory_limit_mib: u64,
) -> Result<Value> {
    if code.len() > DEFAULT_MAX_CODE_BYTES {
        bail!(
            "code is {} bytes; maximum is {} bytes",
            code.len(),
            DEFAULT_MAX_CODE_BYTES
        );
    }

    let directory = tempfile::Builder::new()
        .prefix("nockchain-mcp-rust-")
        .tempdir()
        .context("create Rust code-mode temporary directory")?;
    let source_path = directory.path().join("query.rs");
    std::fs::write(&source_path, render_script(code))
        .context("write generated rust-script source")?;
    let stderr_path = directory.path().join("stderr.log");
    let stderr_file = std::fs::File::create(&stderr_path).context("create rust-script stderr")?;

    let mut command = rust_script_command(rust_script, memory_limit_mib, process_timeout)?;
    command
        .arg("--debug")
        .arg(&source_path)
        .current_dir(directory.path())
        .stderr(stderr_file);
    let mut child = command
        .spawn()
        .with_context(|| format!("start Rust code-mode runner {}", rust_script.display()))?;
    let stdin = child.stdin.take().context("open rust-script stdin")?;
    let stdout = child.stdout.take().context("open rust-script stdout")?;

    let lifecycle = drive_script(
        &mut child,
        BufReader::new(stdout),
        stdin,
        kind,
        backend,
        max_calls,
        max_result_bytes,
    );
    let outcome = tokio::time::timeout(process_timeout, lifecycle).await;
    let result = match outcome {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            terminate_process_group(&mut child).await;
            Err(error)
        }
        Err(_) => {
            terminate_process_group(&mut child).await;
            bail!("Rust code-mode execution exceeded the {process_timeout:?} wall-time limit")
        }
    };

    let stderr = read_stderr(&stderr_path);
    match result {
        Ok(value) => Ok(value),
        Err(error) if stderr.is_empty() => Err(error),
        Err(error) => Err(error.context(format!("rust-script stderr:\n{stderr}"))),
    }
}

#[allow(clippy::too_many_arguments)]
async fn drive_script(
    child: &mut tokio::process::Child,
    mut stdout: BufReader<tokio::process::ChildStdout>,
    mut stdin: tokio::process::ChildStdin,
    kind: SandboxKind,
    backend: Backend,
    max_calls: usize,
    max_result_bytes: usize,
) -> Result<Value> {
    let mut calls = 0_usize;
    let mut result = None;
    while let Some(line) = read_protocol_line(&mut stdout, max_result_bytes + 64 * 1024).await? {
        let frame: ScriptFrame = serde_json::from_slice(&line)
            .context("Rust code wrote non-protocol data to stdout; return a JSON value and do not print to stdout")?;
        if frame.protocol != PROTOCOL {
            bail!("Rust code emitted an unsupported protocol frame")
        }
        match frame.kind.as_str() {
            "call" => {
                let id = frame.id.context("Rust code-mode call is missing an id")?;
                let method = frame
                    .method
                    .as_deref()
                    .context("Rust code-mode call is missing a method")?;
                let response =
                    match handle_call(kind, &backend, &mut calls, max_calls, method, frame.params)
                        .await
                    {
                        Ok(value) => json!({"id": id, "ok": true, "value": value}),
                        Err(error) => json!({"id": id, "ok": false, "error": format!("{error:#}")}),
                    };
                let mut encoded = serde_json::to_vec(&response)?;
                if encoded.len() > max_result_bytes + 64 * 1024 {
                    bail!("Rust code-mode broker response exceeded its configured limit")
                }
                encoded.push(b'\n');
                stdin.write_all(&encoded).await?;
                stdin.flush().await?;
            }
            "result" => {
                if result.is_some() {
                    bail!("Rust code emitted more than one result")
                }
                let encoded = serde_json::to_vec(&frame.value)?;
                if encoded.len() > max_result_bytes {
                    bail!(
                        "Rust result is {} bytes; maximum is {} bytes. Filter or project the result in code.",
                        encoded.len(),
                        max_result_bytes
                    )
                }
                result = Some(frame.value);
            }
            "error" => bail!(
                "Rust code returned an error: {}",
                frame.error.as_deref().unwrap_or("unspecified error")
            ),
            other => bail!("Rust code emitted unknown protocol frame kind {other:?}"),
        }
    }
    drop(stdin);
    let status = child.wait().await.context("wait for rust-script")?;
    if !status.success() {
        bail!("rust-script failed with {status}")
    }
    result.context("Rust code exited without returning a result")
}

async fn handle_call(
    kind: SandboxKind,
    backend: &Backend,
    calls: &mut usize,
    max_calls: usize,
    method: &str,
    params: Value,
) -> Result<Value> {
    match method {
        "spec" => catalog(backend.mode),
        "request" if kind == SandboxKind::Search => {
            bail!("search Rust code can only call codemode.spec()")
        }
        "explain" if kind == SandboxKind::Search => {
            bail!("search Rust code can only call codemode.spec()")
        }
        "request" => {
            *calls += 1;
            if *calls > max_calls {
                bail!("code exceeded the maximum of {max_calls} backend calls")
            }
            let options: RequestOptions = serde_json::from_value(params)
                .context("invalid codemode.request arguments from Rust code")?;
            backend
                .request(&options.operation, options.input, options.format)
                .await
        }
        "explain" => {
            let options: ExplainOptions = serde_json::from_value(params)
                .context("invalid codemode.explain arguments from Rust code")?;
            backend.explain(&options.operation, options.input)
        }
        other => bail!("unknown Rust code-mode method {other:?}"),
    }
}

async fn read_protocol_line(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    limit: usize,
) -> Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some(line))
            };
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if line.len().saturating_add(consumed) > limit {
            bail!("Rust code-mode stdout frame exceeded its configured limit")
        }
        line.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if line.last() == Some(&b'\n') {
            line.pop();
            return Ok(Some(line));
        }
    }
}

fn render_script(code: &str) -> String {
    let mut source = String::with_capacity(RUST_RUNTIME.len() + code.len() + 256);
    source.push_str(RUST_RUNTIME);
    source.push_str("\nfn user_code(codemode: &mut Codemode) -> Result<Value, String> {\n");
    source.push_str(code);
    source.push_str("\n}\n\n");
    source.push_str(
        r#"fn main() {
    let mut codemode = Codemode::new();
    let frame = match user_code(&mut codemode) {
        Ok(value) => json!({"protocol": PROTOCOL, "kind": "result", "value": value}),
        Err(error) => json!({"protocol": PROTOCOL, "kind": "error", "error": error}),
    };
    emit(&frame);
}
"#,
    );
    source
}

fn read_stderr(path: &Path) -> String {
    const LIMIT: usize = 64 * 1024;
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    let start = bytes.len().saturating_sub(LIMIT);
    String::from_utf8_lossy(&bytes[start..]).trim().to_string()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::catalog::ApiMode;

    fn backend(mode: ApiMode) -> Backend {
        Backend {
            mode,
            endpoint: mode.default_backend().to_string(),
            timeout: Duration::from_secs(1),
        }
    }

    #[test]
    fn generated_program_is_rust() {
        let source = render_script("Ok(json!({\"answer\": 42}))");
        assert!(source.contains("fn user_code(codemode: &mut Codemode)"));
        assert!(source.contains("serde_json"));
        assert!(source.contains("rust-script"));
    }

    #[tokio::test]
    async fn broker_explains_without_contacting_a_node() {
        let mut calls = 0;
        let result = handle_call(
            SandboxKind::Execute,
            &backend(ApiMode::Public),
            &mut calls,
            DEFAULT_MAX_CALLS,
            "explain",
            json!({"operation": "get_blocks", "input": {"page": {"clientPageItemsLimit": 1}}}),
        )
        .await
        .expect("explain");
        assert_eq!(
            result["grpc"]["fullMethod"],
            "/nockchain.public.v2.NockchainBlockService/GetBlocks"
        );
    }

    #[tokio::test]
    async fn broker_rejects_mutations() {
        let mut calls = 0;
        let error = handle_call(
            SandboxKind::Execute,
            &backend(ApiMode::Public),
            &mut calls,
            DEFAULT_MAX_CALLS,
            "explain",
            json!({"operation": "wallet_send_transaction", "input": {}}),
        )
        .await
        .expect_err("mutation must fail");
        assert!(error.to_string().contains("unknown or unavailable"));
    }

    #[tokio::test]
    #[ignore = "requires the rust-script executable"]
    async fn rust_script_executes_agent_code() {
        let result = run_code(
            SandboxKind::Search,
            backend(ApiMode::Public),
            r#"
let spec = codemode.spec()?;
let names = spec["operations"]
    .as_array()
    .ok_or_else(|| "operations is not an array".to_string())?
    .iter()
    .filter_map(|operation| operation["name"].as_str())
    .filter(|name| name.contains("block"))
    .collect::<Vec<_>>();
Ok(json!(names))
"#,
            &PathBuf::from("rust-script"),
            DEFAULT_MAX_CALLS,
            DEFAULT_MAX_RESULT_BYTES,
            Duration::from_secs(30),
            512,
        )
        .await
        .expect("run Rust code");
        assert_eq!(
            result,
            json!(["get_blocks", "get_block_details", "get_transaction_block"])
        );
    }
}
