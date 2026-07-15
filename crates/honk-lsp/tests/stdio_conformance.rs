use std::io::{BufReader, Write};
use std::process::{Command, Stdio};

use lsp_server::{Message, Notification, Request, RequestId, ResponseKind};
use serde_json::json;

fn framed(messages: impl IntoIterator<Item = Message>) -> Vec<u8> {
    let mut bytes = Vec::new();
    for message in messages {
        message.write(&mut bytes).expect("frame LSP message");
    }
    bytes
}

#[test]
fn stdio_lifecycle_uses_lsp_framing_and_json_rpc_responses() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repository root");
    let root_uri = url::Url::from_directory_path(root).expect("repository file URI");
    let input = framed([
        Request::new(
            RequestId::from(1),
            "initialize".to_string(),
            json!({
                "processId": null,
                "rootUri": root_uri,
                "capabilities": {},
                "workspaceFolders": [{ "uri": root_uri, "name": "nockchain" }]
            }),
        )
        .into(),
        Notification::new("initialized".to_string(), json!({})).into(),
        Request::new(RequestId::from(2), "shutdown".to_string(), json!(null)).into(),
        Notification::new("exit".to_string(), json!(null)).into(),
    ]);

    let mut child = Command::new(env!("CARGO_BIN_EXE_honk-lsp"))
        .arg("--check-delay-ms=0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn honk-lsp");
    let mut stdin = child.stdin.take().expect("child stdin");
    stdin.write_all(&input).expect("send LSP lifecycle");
    drop(stdin);
    let output = child.wait_with_output().expect("wait for honk-lsp");
    assert!(
        output.status.success(),
        "honk-lsp failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut reader = BufReader::new(output.stdout.as_slice());
    let initialize = Message::read(&mut reader)
        .expect("read initialize response")
        .expect("initialize response");
    let shutdown = Message::read(&mut reader)
        .expect("read shutdown response")
        .expect("shutdown response");
    assert!(Message::read(&mut reader).expect("read EOF").is_none());

    let Message::Response(initialize) = initialize else {
        panic!("initialize did not produce a response");
    };
    assert_eq!(initialize.id, RequestId::from(1));
    let ResponseKind::Ok { result } = initialize.response_kind else {
        panic!("initialize failed");
    };
    assert_eq!(
        result["capabilities"]["textDocumentSync"]["change"],
        json!(1),
        "server must advertise full document synchronization"
    );
    assert_eq!(result["capabilities"]["positionEncoding"], json!("utf-16"));

    let Message::Response(shutdown) = shutdown else {
        panic!("shutdown did not produce a response");
    };
    assert_eq!(shutdown.id, RequestId::from(2));
    assert!(matches!(shutdown.response_kind, ResponseKind::Ok { .. }));
}
