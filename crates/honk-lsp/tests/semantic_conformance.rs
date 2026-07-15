use std::fmt::Write;
use std::path::Path;
use std::str::FromStr;
use std::time::{Duration, Instant};

use honk_lsp::{run_connection, LspConfig};
use honk_service::semantic::SemanticSession;
use honk_service::DEFAULT_WORKER_STACK_BYTES;
use lsp_server::{
    Connection, ErrorCode, Message, Notification, Request, RequestId, Response, ResponseKind,
};
use lsp_types::notification::{DidOpenTextDocument, Notification as LspNotification};
use lsp_types::{
    DidOpenTextDocumentParams, DocumentSymbolResponse, GotoDefinitionResponse, Hover,
    TextDocumentItem, Uri,
};
use serde_json::{json, Value};
use tempfile::TempDir;

fn repository_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root")
        .to_path_buf()
}

fn receive_response_message(client: &Connection, expected: i32) -> Response {
    loop {
        let message = client
            .receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("LSP response");
        let Message::Response(response) = message else {
            continue;
        };
        if response.id == RequestId::from(expected) {
            return response;
        }
    }
}

fn receive_response(client: &Connection, expected: i32) -> Value {
    let response = receive_response_message(client, expected);
    let ResponseKind::Ok { result } = response.response_kind else {
        panic!(
            "LSP request {expected} failed: {:?}",
            response.response_kind
        );
    };
    result
}

fn start_server(
    root: &Path,
    check_delay_ms: u64,
) -> (Connection, std::thread::JoinHandle<anyhow::Result<()>>) {
    start_server_with_dependencies(root, &root.join("hoon"), check_delay_ms)
}

fn start_server_with_dependencies(
    root: &Path,
    dependencies: &Path,
    check_delay_ms: u64,
) -> (Connection, std::thread::JoinHandle<anyhow::Result<()>>) {
    let (server, client) = Connection::memory();
    let server_thread = std::thread::spawn({
        let root = root.to_path_buf();
        let dependencies = dependencies.to_path_buf();
        move || {
            run_connection(
                server,
                LspConfig {
                    prelude: Some(root.join("hoon/common/hoon.hoon")),
                    dependencies: Some(dependencies),
                    entry: None,
                    subject_type_jam: None,
                    dbug: true,
                    vet: true,
                    max_compiles: 0,
                    worker_stack_bytes: DEFAULT_WORKER_STACK_BYTES,
                    check_delay_ms,
                },
            )
        }
    });
    let root_url = url::Url::from_directory_path(root).expect("root file URI");
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
    assert_eq!(initialize["capabilities"]["definitionProvider"], true);
    assert_eq!(
        initialize["capabilities"]["experimental"]["honk"]["semanticWorker"],
        true
    );
    client
        .sender
        .send(Notification::new("initialized".to_string(), json!({})).into())
        .expect("send initialized");
    (client, server_thread)
}

fn shutdown_server(
    client: &Connection,
    server_thread: std::thread::JoinHandle<anyhow::Result<()>>,
    request_id: i32,
) {
    client
        .sender
        .send(
            Request::new(
                RequestId::from(request_id),
                "shutdown".to_string(),
                json!(null),
            )
            .into(),
        )
        .expect("send shutdown");
    client
        .sender
        .send(Notification::new("exit".to_string(), json!(null)).into())
        .expect("send exit");
    let _ = receive_response(client, request_id);
    server_thread
        .join()
        .expect("server thread")
        .expect("server result");
}

#[test]
fn document_symbols_hover_and_definition_use_current_unsaved_snapshot() {
    let root = repository_root();
    let temp = TempDir::new().expect("temporary workspace");
    let entry = temp.path().join("symbols.hoon");
    std::fs::write(&entry, "42\n").expect("disk entry");
    let entry_url = url::Url::from_file_path(&entry).expect("entry file URI");
    let entry_uri = Uri::from_str(entry_url.as_str()).expect("entry LSP URI");
    let source =
        "|%\n++  answer\n  42\n++  doubled\n  (add answer answer)\n+$  pair\n  $:  left=@  right=@  ==\n--\n";
    let (client, server_thread) = start_server(&root, 0);

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
    assert_eq!(symbols.len(), 3);
    assert_eq!(symbols[0].name, "answer");
    assert_eq!(symbols[0].selection_range.start.line, 1);
    assert_eq!(symbols[0].selection_range.start.character, 4);
    assert_eq!(symbols[1].name, "doubled");
    assert_eq!(symbols[2].name, "pair");

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

    let inferred_deadline = Instant::now() + Duration::from_secs(30);
    let mut request_id = 4;
    let inferred_hover = loop {
        client
            .sender
            .send(
                Request::new(
                    RequestId::from(request_id),
                    "textDocument/hover".to_string(),
                    json!({
                        "textDocument": { "uri": entry_uri },
                        "position": { "line": 2, "character": 2 }
                    }),
                )
                .into(),
            )
            .expect("request inferred-type hover");
        let hover = serde_json::from_value::<Option<Hover>>(receive_response(&client, request_id))
            .expect("inferred-type hover response");
        if hover.as_ref().is_some_and(|hover| {
            serde_json::to_string(&hover.contents)
                .expect("hover JSON")
                .contains("Inferred type")
        }) {
            break hover.expect("checked above");
        }
        assert!(
            Instant::now() < inferred_deadline,
            "compiler-owned inferred type did not become available"
        );
        request_id += 1;
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(
        serde_json::to_string(&inferred_hover.contents)
            .expect("inferred hover JSON")
            .contains("@"),
        "constant expression should have an atom-shaped inferred type"
    );

    let definition_request_id = request_id + 1;
    client
        .sender
        .send(
            Request::new(
                RequestId::from(definition_request_id),
                "textDocument/definition".to_string(),
                json!({
                    "textDocument": { "uri": entry_uri },
                    "position": { "line": 4, "character": 8 }
                }),
            )
            .into(),
        )
        .expect("request definition");
    let definition = serde_json::from_value::<Option<GotoDefinitionResponse>>(receive_response(
        &client, definition_request_id,
    ))
    .expect("definition response")
    .expect("compiler-resolved definition");
    let GotoDefinitionResponse::Scalar(definition) = definition else {
        panic!("server must return a single definition location");
    };
    assert_eq!(definition.uri, entry_uri);
    assert_eq!(definition.range.start.line, 2);
    assert_eq!(definition.range.start.character, 2);

    shutdown_server(&client, server_thread, definition_request_id + 1);
}

#[test]
fn definition_navigates_to_an_imported_gate() {
    let root = repository_root();
    let temp = TempDir::new().expect("temporary workspace");
    let lib = temp.path().join("lib");
    std::fs::create_dir(&lib).expect("library directory");
    let helper = lib.join("helper.hoon");
    std::fs::write(&helper, "|=  [a=@ b=@]\n  (add a b)\n").expect("helper source");
    let entry = temp.path().join("entry.hoon");
    std::fs::write(&entry, "42\n").expect("disk entry");
    let entry_uri = Uri::from_str(
        url::Url::from_file_path(&entry)
            .expect("entry file URI")
            .as_str(),
    )
    .expect("entry LSP URI");
    let helper_uri = Uri::from_str(
        url::Url::from_file_path(&helper)
            .expect("helper file URI")
            .as_str(),
    )
    .expect("helper LSP URI");
    let source = "/+  helper\n|=  [a=@ b=@]\n  (helper a b)\n";
    let (client, server_thread) = start_server_with_dependencies(&root, temp.path(), 0);
    client
        .sender
        .send(
            Notification::new(
                DidOpenTextDocument::METHOD.to_string(),
                DidOpenTextDocumentParams {
                    text_document: TextDocumentItem::new(
                        entry_uri.clone(),
                        "hoon".to_string(),
                        1,
                        source.to_string(),
                    ),
                },
            )
            .into(),
        )
        .expect("send didOpen");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut request_id = 2;
    let definition = loop {
        client
            .sender
            .send(
                Request::new(
                    RequestId::from(request_id),
                    "textDocument/definition".to_string(),
                    json!({
                        "textDocument": { "uri": entry_uri },
                        "position": { "line": 2, "character": 4 }
                    }),
                )
                .into(),
            )
            .expect("request imported definition");
        let definition = serde_json::from_value::<Option<GotoDefinitionResponse>>(
            receive_response(&client, request_id),
        )
        .expect("definition response");
        if let Some(definition) = definition {
            break definition;
        }
        assert!(
            Instant::now() < deadline,
            "compiler-resolved imported definition did not become available"
        );
        request_id += 1;
        std::thread::sleep(Duration::from_millis(25));
    };
    let GotoDefinitionResponse::Scalar(definition) = definition else {
        panic!("server must return a single imported definition location");
    };
    assert_eq!(definition.uri, helper_uri);
    assert_eq!(definition.range.start.line, 1);
    assert_eq!(definition.range.start.character, 2);

    shutdown_server(&client, server_thread, request_id + 1);
}

#[test]
fn cancellation_and_other_requests_remain_responsive_during_semantic_indexing() {
    let root = repository_root();
    let temp = TempDir::new().expect("temporary workspace");
    let entry = temp.path().join("large.hoon");
    std::fs::write(&entry, "42\n").expect("disk entry");
    let entry_url = url::Url::from_file_path(&entry).expect("entry file URI");
    let entry_uri = Uri::from_str(entry_url.as_str()).expect("entry LSP URI");
    let mut source = String::with_capacity(1_500_000);
    source.push_str("|%\n");
    for arm in 0..50_000 {
        writeln!(&mut source, "++  arm-{arm}\n  42").expect("write source");
    }
    source.push_str("--\n");
    SemanticSession::default()
        .snapshot(&entry, 1, &source)
        .expect("large semantic fixture must parse");

    // Keep the independent compiler checker in its debounce window so this
    // contract measures semantic-worker responsiveness in isolation.
    let (client, server_thread) = start_server(&root, 60_000);
    client
        .sender
        .send(
            Notification::new(
                DidOpenTextDocument::METHOD.to_string(),
                DidOpenTextDocumentParams {
                    text_document: TextDocumentItem::new(
                        entry_uri.clone(),
                        "hoon".to_string(),
                        1,
                        source,
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
    std::thread::sleep(Duration::from_millis(50));
    let cancellation_started = Instant::now();
    client
        .sender
        .send(Notification::new("$/cancelRequest".to_string(), json!({ "id": 2 })).into())
        .expect("cancel document symbols");
    client
        .sender
        .send(Request::new(RequestId::from(3), "honk/testPing".to_string(), json!(null)).into())
        .expect("send unrelated request");

    let cancelled = receive_response_message(&client, 2);
    let ResponseKind::Err { error } = cancelled.response_kind else {
        panic!("cancelled semantic request returned a result");
    };
    assert_eq!(error.code, ErrorCode::RequestCanceled as i32);
    let unrelated = receive_response_message(&client, 3);
    let ResponseKind::Err { error } = unrelated.response_kind else {
        panic!("unsupported request unexpectedly succeeded");
    };
    assert_eq!(error.code, ErrorCode::MethodNotFound as i32);
    assert!(
        cancellation_started.elapsed() < Duration::from_secs(1),
        "semantic indexing blocked the protocol thread"
    );

    shutdown_server(&client, server_thread, 4);
    let duplicate_responses = client
        .receiver
        .try_iter()
        .filter(|message| {
            matches!(message, Message::Response(response) if response.id == RequestId::from(2))
        })
        .count();
    assert_eq!(
        duplicate_responses, 0,
        "cancellation must respond exactly once"
    );
}
