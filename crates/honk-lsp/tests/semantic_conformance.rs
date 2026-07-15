use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use honk_lsp::{run_connection, LspConfig};
use honk_service::DEFAULT_WORKER_STACK_BYTES;
use lsp_server::{Connection, Message, Notification, Request, RequestId, ResponseKind};
use lsp_types::notification::{DidOpenTextDocument, Notification as LspNotification};
use lsp_types::{DidOpenTextDocumentParams, DocumentSymbolResponse, Hover, TextDocumentItem, Uri};
use serde_json::{json, Value};
use tempfile::TempDir;

fn repository_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root")
        .to_path_buf()
}

fn receive_response(client: &Connection, expected: i32) -> Value {
    loop {
        let message = client
            .receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("LSP response");
        let Message::Response(response) = message else {
            continue;
        };
        if response.id != RequestId::from(expected) {
            continue;
        }
        let ResponseKind::Ok { result } = response.response_kind else {
            panic!(
                "LSP request {expected} failed: {:?}",
                response.response_kind
            );
        };
        return result;
    }
}

#[test]
fn document_symbols_and_hover_use_current_unsaved_snapshot() {
    let root = repository_root();
    let temp = TempDir::new().expect("temporary workspace");
    let entry = temp.path().join("symbols.hoon");
    std::fs::write(&entry, "42\n").expect("disk entry");
    let entry_url = url::Url::from_file_path(&entry).expect("entry file URI");
    let entry_uri = Uri::from_str(entry_url.as_str()).expect("entry LSP URI");
    let root_url = url::Url::from_directory_path(&root).expect("root file URI");
    let source = "|%\n++  answer\n  42\n+$  pair\n  $:  left=@  right=@  ==\n--\n";

    let (server, client) = Connection::memory();
    let server_thread = std::thread::spawn({
        let root = root.clone();
        move || {
            run_connection(
                server,
                LspConfig {
                    prelude: Some(root.join("hoon/common/hoon.hoon")),
                    dependencies: Some(root.join("hoon")),
                    entry: None,
                    subject_type_jam: None,
                    dbug: true,
                    vet: true,
                    max_compiles: 0,
                    worker_stack_bytes: DEFAULT_WORKER_STACK_BYTES,
                    check_delay_ms: 0,
                },
            )
        }
    });

    client
        .sender
        .send(
            Request::new(
                RequestId::from(1),
                "initialize".to_string(),
                json!({
                    "processId": null,
                    "rootUri": root_url,
                    "capabilities": {},
                    "workspaceFolders": [{ "uri": root_url, "name": "nockchain" }]
                }),
            )
            .into(),
        )
        .expect("send initialize");
    let initialize = receive_response(&client, 1);
    assert_eq!(initialize["capabilities"]["documentSymbolProvider"], true);
    assert_eq!(initialize["capabilities"]["hoverProvider"], true);

    client
        .sender
        .send(Notification::new("initialized".to_string(), json!({})).into())
        .expect("send initialized");
    client
        .sender
        .send(
            Notification::new(
                DidOpenTextDocument::METHOD.to_string(),
                DidOpenTextDocumentParams {
                    text_document: TextDocumentItem::new(
                        entry_uri.clone(),
                        "hoon".to_string(),
                        9,
                        source.to_string(),
                    ),
                },
            )
            .into(),
        )
        .expect("send didOpen");

    client
        .sender
        .send(
            Request::new(
                RequestId::from(2),
                "textDocument/documentSymbol".to_string(),
                json!({ "textDocument": { "uri": entry_uri } }),
            )
            .into(),
        )
        .expect("request document symbols");
    let symbols =
        serde_json::from_value::<Option<DocumentSymbolResponse>>(receive_response(&client, 2))
            .expect("document symbol response")
            .expect("document symbols");
    let DocumentSymbolResponse::Nested(symbols) = symbols else {
        panic!("server must return hierarchical document symbols");
    };
    assert_eq!(symbols.len(), 2);
    assert_eq!(symbols[0].name, "answer");
    assert_eq!(symbols[0].selection_range.start.line, 1);
    assert_eq!(symbols[0].selection_range.start.character, 4);
    assert_eq!(symbols[1].name, "pair");

    client
        .sender
        .send(
            Request::new(
                RequestId::from(3),
                "textDocument/hover".to_string(),
                json!({
                    "textDocument": { "uri": entry_uri },
                    "position": { "line": 1, "character": 5 }
                }),
            )
            .into(),
        )
        .expect("request hover");
    let hover = serde_json::from_value::<Option<Hover>>(receive_response(&client, 3))
        .expect("hover response")
        .expect("hover result");
    let hover_json = serde_json::to_value(hover.contents).expect("hover JSON");
    assert!(hover_json.to_string().contains("answer"));
    assert!(hover_json.to_string().contains("++"));

    client
        .sender
        .send(Request::new(RequestId::from(4), "shutdown".to_string(), json!(null)).into())
        .expect("send shutdown");
    client
        .sender
        .send(Notification::new("exit".to_string(), json!(null)).into())
        .expect("send exit");
    let _ = receive_response(&client, 4);
    server_thread
        .join()
        .expect("server thread")
        .expect("server result");
}
