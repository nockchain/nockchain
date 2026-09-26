use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use honk_lsp::{run_connection, LspConfig};
use honk_service::DEFAULT_WORKER_STACK_BYTES;
use lsp_server::{Connection, Message, Notification, Request, RequestId, ResponseKind};
use lsp_types::notification::{
    DidCloseTextDocument, DidOpenTextDocument, Notification as LspNotification, PublishDiagnostics,
};
use lsp_types::{
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, PublishDiagnosticsParams,
    TextDocumentIdentifier, TextDocumentItem, Uri,
};
use serde_json::json;
use tempfile::TempDir;

fn repository_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root")
        .to_path_buf()
}

#[test]
fn unsaved_parse_error_is_published_for_the_current_document_version() {
    let root = repository_root();
    let temp = TempDir::new().expect("temporary workspace");
    let entry = temp.path().join("broken.hoon");
    std::fs::write(&entry, "42\n").expect("disk entry");
    let entry_url = url::Url::from_file_path(&entry).expect("entry file URI");
    let entry_uri = Uri::from_str(entry_url.as_str()).expect("entry LSP URI");
    let root_url = url::Url::from_directory_path(&root).expect("root file URI");

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
    let initialize = client
        .receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("initialize response");
    assert!(matches!(
        initialize,
        Message::Response(lsp_server::Response {
            response_kind: ResponseKind::Ok { .. },
            ..
        })
    ));
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
                        7,
                        "|=  [a=@\n".to_string(),
                    ),
                },
            )
            .into(),
        )
        .expect("send didOpen");

    let published = loop {
        let message = client
            .receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("publish diagnostics");
        if let Message::Notification(notification) = message {
            if notification.method == PublishDiagnostics::METHOD {
                break serde_json::from_value::<PublishDiagnosticsParams>(notification.params)
                    .expect("publish diagnostics parameters");
            }
        }
    };
    assert_eq!(published.uri, entry_uri);
    assert_eq!(published.version, Some(7));
    assert_eq!(published.diagnostics.len(), 1);
    assert_eq!(published.diagnostics[0].source.as_deref(), Some("honk"));

    client
        .sender
        .send(
            Notification::new(
                DidCloseTextDocument::METHOD.to_string(),
                DidCloseTextDocumentParams {
                    text_document: TextDocumentIdentifier::new(entry_uri.clone()),
                },
            )
            .into(),
        )
        .expect("send didClose");
    let cleared = loop {
        let message = client
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("cleared diagnostics");
        if let Message::Notification(notification) = message {
            if notification.method == PublishDiagnostics::METHOD {
                break serde_json::from_value::<PublishDiagnosticsParams>(notification.params)
                    .expect("cleared diagnostics parameters");
            }
        }
    };
    assert_eq!(cleared.uri, entry_uri);
    assert!(cleared.diagnostics.is_empty());

    client
        .sender
        .send(Request::new(RequestId::from(2), "shutdown".to_string(), json!(null)).into())
        .expect("send shutdown");
    client
        .sender
        .send(Notification::new("exit".to_string(), json!(null)).into())
        .expect("send exit");
    let shutdown = client
        .receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("shutdown response");
    assert!(matches!(
        shutdown,
        Message::Response(lsp_server::Response {
            response_kind: ResponseKind::Ok { .. },
            ..
        })
    ));
    server_thread
        .join()
        .expect("server thread")
        .expect("server result");
}
