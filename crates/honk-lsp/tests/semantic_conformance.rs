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
use lsp_types::notification::{
    DidChangeWatchedFiles, DidOpenTextDocument, Notification as LspNotification,
};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, DidOpenTextDocumentParams,
    DocumentSymbolResponse, GotoDefinitionResponse, Hover, Location, PrepareRenameResponse,
    TextDocumentItem, Uri, WorkspaceEdit,
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

fn file_uri(path: &Path) -> Uri {
    Uri::from_str(url::Url::from_file_path(path).expect("file URI").as_str()).expect("LSP file URI")
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

fn request_definition(
    client: &Connection,
    request_id: i32,
    uri: &Uri,
    line: usize,
    character: usize,
) -> Option<Location> {
    client
        .sender
        .send(
            Request::new(
                RequestId::from(request_id),
                "textDocument/definition".to_string(),
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character }
                }),
            )
            .into(),
        )
        .expect("request definition");
    let response = serde_json::from_value::<Option<GotoDefinitionResponse>>(receive_response(
        client, request_id,
    ))
    .expect("definition response")?;
    let GotoDefinitionResponse::Scalar(location) = response else {
        panic!("server must return a single definition location");
    };
    Some(location)
}

fn request_references(
    client: &Connection,
    request_id: i32,
    uri: &Uri,
    line: usize,
    character: usize,
    include_declaration: bool,
) -> Vec<Location> {
    client
        .sender
        .send(
            Request::new(
                RequestId::from(request_id),
                "textDocument/references".to_string(),
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                    "context": { "includeDeclaration": include_declaration }
                }),
            )
            .into(),
        )
        .expect("request references");
    serde_json::from_value::<Option<Vec<Location>>>(receive_response(client, request_id))
        .expect("references response")
        .unwrap_or_default()
}

fn request_completion(
    client: &Connection,
    request_id: i32,
    uri: &Uri,
    line: usize,
    character: usize,
) -> Vec<CompletionItem> {
    client
        .sender
        .send(
            Request::new(
                RequestId::from(request_id),
                "textDocument/completion".to_string(),
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character }
                }),
            )
            .into(),
        )
        .expect("request completion");
    match serde_json::from_value::<Option<CompletionResponse>>(receive_response(client, request_id))
        .expect("completion response")
        .expect("completion candidates")
    {
        CompletionResponse::Array(items) => items,
        CompletionResponse::List(list) => list.items,
    }
}

fn request_workspace_symbols(client: &Connection, request_id: i32, query: &str) -> Vec<Value> {
    client
        .sender
        .send(
            Request::new(
                RequestId::from(request_id),
                "workspace/symbol".to_string(),
                json!({ "query": query }),
            )
            .into(),
        )
        .expect("request workspace symbols");
    serde_json::from_value::<Option<Vec<Value>>>(receive_response(client, request_id))
        .expect("workspace symbol response")
        .expect("workspace symbols")
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
    assert_eq!(initialize["capabilities"]["referencesProvider"], true);
    assert_eq!(
        initialize["capabilities"]["workspaceSymbolProvider"]["resolveProvider"],
        false
    );
    assert!(initialize["capabilities"]["completionProvider"].is_object());
    assert_eq!(
        initialize["capabilities"]["renameProvider"]["prepareProvider"],
        true
    );
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
    let math = lib.join("math.hoon");
    std::fs::write(&math, "|%\n++  add-two\n  |=  [a=@ b=@]\n  (add a b)\n--\n")
        .expect("math source");
    let other = lib.join("other.hoon");
    std::fs::write(
        &other, "|%\n++  add-two\n  |=  [a=@ b=@]\n  (sub a b)\n--\n",
    )
    .expect("other source");
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
    let math_uri = Uri::from_str(
        url::Url::from_file_path(&math)
            .expect("math file URI")
            .as_str(),
    )
    .expect("math LSP URI");
    let other_uri = Uri::from_str(
        url::Url::from_file_path(&other)
            .expect("other file URI")
            .as_str(),
    )
    .expect("other LSP URI");
    let source = "/+  helper, math, other\n|=  [a=@ b=@]\n  [(helper a b) (add-two:math a b) (add-two:other a b)]\n";
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

    request_id += 1;
    let completions = request_completion(&client, request_id, &entry_uri, 2, 6);
    let helper = completions
        .iter()
        .find(|item| item.label == "helper")
        .expect("imported gate completion");
    assert_eq!(helper.kind, Some(CompletionItemKind::FUNCTION));
    assert!(helper
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("imported Hoon gate")));

    request_id += 1;
    let reference_deadline = Instant::now() + Duration::from_secs(30);
    let add_two_character = source
        .lines()
        .nth(2)
        .expect("entry body")
        .find("add-two")
        .expect("add-two use");
    let references = loop {
        let references = request_references(
            &client,
            request_id,
            &entry_uri,
            2,
            add_two_character + 2,
            true,
        );
        if references.iter().any(|location| location.uri == entry_uri)
            && references.iter().any(|location| location.uri == math_uri)
        {
            break references;
        }
        assert!(
            Instant::now() < reference_deadline,
            "compiler-owned imported references did not become available"
        );
        request_id += 1;
        std::thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(references.len(), 2);
    assert!(references.iter().all(|location| location.uri != other_uri));
    assert!(references.iter().any(|location| {
        location.uri == entry_uri
            && location.range.start.line == 2
            && location.range.start.character
                == u32::try_from(add_two_character).expect("small character")
    }));

    request_id += 1;
    let uses_only = request_references(
        &client,
        request_id,
        &entry_uri,
        2,
        add_two_character + 2,
        false,
    );
    assert_eq!(uses_only.len(), 1);
    assert_eq!(uses_only[0].uri, entry_uri);

    let edited_source = "/+  helper, math, other\n|=  [a=@ b=@]\n  [(add-two:math a b) (add-two:math b a) (add-two:other a b)]\n";
    client
        .sender
        .send(
            Notification::new(
                "textDocument/didChange".to_string(),
                json!({
                    "textDocument": { "uri": entry_uri, "version": 2 },
                    "contentChanges": [{ "text": edited_source }]
                }),
            )
            .into(),
        )
        .expect("send unsaved imported-reference edit");
    request_id += 1;
    assert!(
        request_references(&client, request_id, &entry_uri, 2, 5, true).is_empty(),
        "reference facts from the previous editor generation must be invalidated"
    );

    request_id += 1;
    let updated_deadline = Instant::now() + Duration::from_secs(30);
    let updated_references = loop {
        let references = request_references(&client, request_id, &entry_uri, 2, 5, true);
        let entry_uses = references
            .iter()
            .filter(|location| location.uri == entry_uri)
            .count();
        if entry_uses == 2 && references.iter().any(|location| location.uri == math_uri) {
            break references;
        }
        assert!(
            Instant::now() < updated_deadline,
            "unsaved imported references did not refresh"
        );
        request_id += 1;
        std::thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(updated_references.len(), 3);
    assert!(updated_references
        .iter()
        .all(|location| location.uri != other_uri));

    shutdown_server(&client, server_thread, request_id + 1);
}

#[test]
fn structural_references_preserve_import_identity_across_open_roots() {
    let root = repository_root();
    let temp = TempDir::new().expect("temporary workspace");
    let lib = temp.path().join("lib");
    std::fs::create_dir(&lib).expect("library directory");
    let alpha = lib.join("alpha.hoon");
    let beta = lib.join("beta.hoon");
    let alpha_entry = temp.path().join("alpha-entry.hoon");
    let beta_entry = temp.path().join("beta-entry.hoon");
    let mold_source = "|%\n+$  widget  @\n--\n";
    let entry_source = |import: &str| format!("/+  {import}\n^-  widget\n42\n");
    std::fs::write(&alpha, mold_source).expect("alpha mold source");
    std::fs::write(&beta, mold_source).expect("beta mold source");
    std::fs::write(&alpha_entry, "42\n").expect("alpha disk entry");
    std::fs::write(&beta_entry, "42\n").expect("beta disk entry");

    let alpha_uri = file_uri(&alpha);
    let beta_uri = file_uri(&beta);
    let alpha_entry_uri = file_uri(&alpha_entry);
    let beta_entry_uri = file_uri(&beta_entry);
    let alpha_entry_source = entry_source("alpha");
    let beta_entry_source = entry_source("beta");
    let (client, server_thread) = start_server_with_dependencies(&root, temp.path(), 0);
    for (uri, source) in [
        (alpha_entry_uri.clone(), alpha_entry_source),
        (beta_entry_uri.clone(), beta_entry_source),
    ] {
        client
            .sender
            .send(
                Notification::new(
                    DidOpenTextDocument::METHOD.to_string(),
                    DidOpenTextDocumentParams {
                        text_document: TextDocumentItem::new(uri, "hoon".to_string(), 1, source),
                    },
                )
                .into(),
            )
            .expect("open structural-reference root");
    }

    let alpha_references = request_references(&client, 2, &alpha_entry_uri, 1, 5, true);
    assert_eq!(alpha_references.len(), 2);
    assert!(alpha_references
        .iter()
        .any(|location| location.uri == alpha_entry_uri));
    assert!(alpha_references
        .iter()
        .any(|location| location.uri == alpha_uri));
    assert!(alpha_references
        .iter()
        .all(|location| location.uri != beta_entry_uri && location.uri != beta_uri));

    let beta_references = request_references(&client, 3, &beta_entry_uri, 1, 5, true);
    assert_eq!(beta_references.len(), 2);
    assert!(beta_references
        .iter()
        .any(|location| location.uri == beta_entry_uri));
    assert!(beta_references
        .iter()
        .any(|location| location.uri == beta_uri));
    assert!(beta_references
        .iter()
        .all(|location| location.uri != alpha_entry_uri && location.uri != alpha_uri));

    client
        .sender
        .send(
            Notification::new(
                DidOpenTextDocument::METHOD.to_string(),
                DidOpenTextDocumentParams {
                    text_document: TextDocumentItem::new(
                        alpha_uri.clone(),
                        "hoon".to_string(),
                        1,
                        mold_source.to_string(),
                    ),
                },
            )
            .into(),
        )
        .expect("open structural declaration");
    let declaration_references = request_references(&client, 4, &alpha_uri, 1, 5, true);
    assert_eq!(declaration_references.len(), 2);
    assert!(declaration_references
        .iter()
        .any(|location| location.uri == alpha_entry_uri));
    assert!(declaration_references
        .iter()
        .any(|location| location.uri == alpha_uri));
    assert!(declaration_references
        .iter()
        .all(|location| location.uri != beta_entry_uri && location.uri != beta_uri));

    shutdown_server(&client, server_thread, 5);
}

#[test]
fn workspace_references_index_unopened_sources_and_follow_watcher_changes() {
    let root = repository_root();
    let temp = TempDir::new().expect("temporary workspace");
    let lib = temp.path().join("lib");
    std::fs::create_dir(&lib).expect("library directory");
    let declaration = lib.join("types.hoon");
    let first = temp.path().join("first.hoon");
    let second = temp.path().join("second.hoon");
    let third = temp.path().join("third.hoon");
    let declaration_source = "|%\n+$  widget  @\n--\n";
    let use_source = "/+  types\n^-  widget\n42\n";
    std::fs::write(&declaration, declaration_source).expect("mold declaration");
    std::fs::write(&first, use_source).expect("first source");
    std::fs::write(&second, use_source).expect("second source");

    let declaration_uri = file_uri(&declaration);
    let first_uri = file_uri(&first);
    let second_uri = file_uri(&second);
    let third_uri = file_uri(&third);
    let (client, server_thread) = start_server_with_dependencies(&root, temp.path(), 0);
    client
        .sender
        .send(
            Notification::new(
                DidOpenTextDocument::METHOD.to_string(),
                DidOpenTextDocumentParams {
                    text_document: TextDocumentItem::new(
                        first_uri.clone(),
                        "hoon".to_string(),
                        1,
                        use_source.to_string(),
                    ),
                },
            )
            .into(),
        )
        .expect("open first source");

    let initial = request_references(&client, 2, &first_uri, 1, 5, true);
    assert_eq!(initial.len(), 3);
    assert!(initial
        .iter()
        .any(|location| location.uri == declaration_uri));
    assert!(initial.iter().any(|location| location.uri == first_uri));
    assert!(initial.iter().any(|location| location.uri == second_uri));

    std::fs::write(&second, "42\n").expect("update second source");
    client
        .sender
        .send(
            Notification::new(
                DidChangeWatchedFiles::METHOD.to_string(),
                json!({ "changes": [{ "uri": second_uri, "type": 2 }] }),
            )
            .into(),
        )
        .expect("notify changed source");
    let after_change = request_references(&client, 3, &first_uri, 1, 5, true);
    assert_eq!(after_change.len(), 2);
    assert!(after_change
        .iter()
        .all(|location| location.uri != second_uri));

    std::fs::write(&third, use_source).expect("create third source");
    client
        .sender
        .send(
            Notification::new(
                DidChangeWatchedFiles::METHOD.to_string(),
                json!({ "changes": [{ "uri": third_uri, "type": 1 }] }),
            )
            .into(),
        )
        .expect("notify created source");
    let after_create = request_references(&client, 4, &first_uri, 1, 5, true);
    assert_eq!(after_create.len(), 3);
    assert!(after_create
        .iter()
        .any(|location| location.uri == third_uri));

    std::fs::remove_file(&third).expect("delete third source");
    client
        .sender
        .send(
            Notification::new(
                DidChangeWatchedFiles::METHOD.to_string(),
                json!({ "changes": [{ "uri": third_uri, "type": 3 }] }),
            )
            .into(),
        )
        .expect("notify deleted source");
    let after_delete = request_references(&client, 5, &first_uri, 1, 5, true);
    assert_eq!(after_delete.len(), 2);
    assert!(after_delete
        .iter()
        .all(|location| location.uri != third_uri));

    shutdown_server(&client, server_thread, 6);
}

#[test]
fn workspace_symbols_index_unopened_sources_and_refresh_changed_files() {
    let root = repository_root();
    let temp = TempDir::new().expect("temporary workspace");
    let types = temp.path().join("types.hoon");
    let use_path = temp.path().join("use.hoon");
    std::fs::write(&types, "|%\n+$  widget  @\n--\n").expect("initial mold declaration");
    std::fs::write(&use_path, "/+  types\n^-  widget\n42\n").expect("workspace use");
    let types_uri = file_uri(&types);
    let (client, server_thread) = start_server_with_dependencies(&root, temp.path(), 0);

    let initial = request_workspace_symbols(&client, 2, "widget");
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0]["name"], "widget");
    assert_eq!(initial[0]["location"]["uri"], types_uri.as_str());

    std::fs::write(&types, "|%\n+$  gadget  @\n--\n").expect("changed mold declaration");
    client
        .sender
        .send(
            Notification::new(
                DidChangeWatchedFiles::METHOD.to_string(),
                json!({ "changes": [{ "uri": types_uri, "type": 2 }] }),
            )
            .into(),
        )
        .expect("notify changed declaration");
    assert!(request_workspace_symbols(&client, 3, "widget").is_empty());
    let changed = request_workspace_symbols(&client, 4, "gadget");
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["name"], "gadget");

    shutdown_server(&client, server_thread, 5);
}

#[test]
fn workspace_rename_edits_unopened_sources_and_rejects_import_collisions() {
    let root = repository_root();
    let temp = TempDir::new().expect("temporary workspace");
    let lib = temp.path().join("lib");
    std::fs::create_dir(&lib).expect("library directory");
    let declaration = lib.join("types.hoon");
    let competing = lib.join("other.hoon");
    let first = temp.path().join("first.hoon");
    let second = temp.path().join("second.hoon");
    let captured = temp.path().join("captured.hoon");
    let declaration_source = "|%\n+$  widget  @\n--\n";
    let competing_source = "|%\n+$  gizmo  @\n--\n";
    let use_source = "/+  types, other\n^-  widget\n42\n";
    let captured_source = "/+  types, other\n=/  captured  1\n^-  widget\n42\n";
    std::fs::write(&declaration, declaration_source).expect("mold declaration");
    std::fs::write(&competing, competing_source).expect("competing mold declaration");
    std::fs::write(&first, use_source).expect("first source");
    std::fs::write(&second, use_source).expect("second source");
    std::fs::write(&captured, captured_source).expect("capturing source");

    let declaration_uri = file_uri(&declaration);
    let first_uri = file_uri(&first);
    let second_uri = file_uri(&second);
    let captured_uri = file_uri(&captured);
    let (client, server_thread) = start_server_with_dependencies(&root, temp.path(), 0);
    client
        .sender
        .send(
            Notification::new(
                DidOpenTextDocument::METHOD.to_string(),
                DidOpenTextDocumentParams {
                    text_document: TextDocumentItem::new(
                        first_uri.clone(),
                        "hoon".to_string(),
                        7,
                        use_source.to_string(),
                    ),
                },
            )
            .into(),
        )
        .expect("open first source");

    client
        .sender
        .send(
            Request::new(
                RequestId::from(2),
                "textDocument/prepareRename".to_string(),
                json!({
                    "textDocument": { "uri": first_uri },
                    "position": { "line": 1, "character": 5 }
                }),
            )
            .into(),
        )
        .expect("prepare workspace rename");
    let prepared =
        serde_json::from_value::<Option<PrepareRenameResponse>>(receive_response(&client, 2))
            .expect("prepare rename response")
            .expect("imported mold can be renamed");
    assert_eq!(
        prepared,
        PrepareRenameResponse::RangeWithPlaceholder {
            range: lsp_types::Range::new(
                lsp_types::Position::new(1, 4),
                lsp_types::Position::new(1, 10),
            ),
            placeholder: "widget".to_string(),
        }
    );

    client
        .sender
        .send(
            Request::new(
                RequestId::from(3),
                "textDocument/rename".to_string(),
                json!({
                    "textDocument": { "uri": first_uri },
                    "position": { "line": 1, "character": 5 },
                    "newName": "renamed-widget"
                }),
            )
            .into(),
        )
        .expect("request workspace rename");
    let edit = serde_json::from_value::<Option<WorkspaceEdit>>(receive_response(&client, 3))
        .expect("workspace rename response")
        .expect("workspace rename edit");
    let edit_json = serde_json::to_value(edit).expect("workspace edit JSON");
    let document_changes = edit_json["documentChanges"]
        .as_array()
        .expect("document changes");
    assert_eq!(document_changes.len(), 4);
    for uri in [&captured_uri, &declaration_uri, &first_uri, &second_uri] {
        let document = document_changes
            .iter()
            .find(|document| document["textDocument"]["uri"] == uri.as_str())
            .unwrap_or_else(|| panic!("missing rename edits for {uri:?}"));
        assert_eq!(document["edits"].as_array().expect("text edits").len(), 1);
        assert_eq!(document["edits"][0]["newText"], "renamed-widget");
    }
    let first_edits = document_changes
        .iter()
        .find(|document| document["textDocument"]["uri"] == first_uri.as_str())
        .expect("open document edits");
    assert_eq!(first_edits["textDocument"]["version"], 7);
    for uri in [&captured_uri, &declaration_uri, &second_uri] {
        let unopened = document_changes
            .iter()
            .find(|document| document["textDocument"]["uri"] == uri.as_str())
            .expect("unopened document edits");
        assert!(unopened["textDocument"]["version"].is_null());
    }

    client
        .sender
        .send(
            Request::new(
                RequestId::from(4),
                "textDocument/rename".to_string(),
                json!({
                    "textDocument": { "uri": first_uri },
                    "position": { "line": 1, "character": 5 },
                    "newName": "gizmo"
                }),
            )
            .into(),
        )
        .expect("request colliding workspace rename");
    let collision = receive_response_message(&client, 4);
    let ResponseKind::Err { error } = collision.response_kind else {
        panic!("workspace rename collision must be rejected");
    };
    assert_eq!(error.code, ErrorCode::InvalidParams as i32);
    assert!(error.message.contains("gizmo"));
    assert!(error.message.contains("capture"));

    client
        .sender
        .send(
            Request::new(
                RequestId::from(5),
                "textDocument/rename".to_string(),
                json!({
                    "textDocument": { "uri": first_uri },
                    "position": { "line": 1, "character": 5 },
                    "newName": "captured"
                }),
            )
            .into(),
        )
        .expect("request capturing workspace rename");
    let capture = receive_response_message(&client, 5);
    let ResponseKind::Err { error } = capture.response_kind else {
        panic!("workspace rename lexical capture must be rejected");
    };
    assert_eq!(error.code, ErrorCode::InvalidParams as i32);
    assert!(error.message.contains("captured"));
    assert!(error.message.contains("capture"));

    shutdown_server(&client, server_thread, 6);
}

#[test]
fn definition_navigates_to_a_hyphenated_mold_arm() {
    let root = repository_root();
    let temp = TempDir::new().expect("temporary workspace");
    let entry = temp.path().join("hyphenated-mold.hoon");
    std::fs::write(&entry, "42\n").expect("disk entry");
    let entry_uri = Uri::from_str(
        url::Url::from_file_path(&entry)
            .expect("entry file URI")
            .as_str(),
    )
    .expect("entry LSP URI");
    let source = "|%\n+$  kernel-state  [%state version=%1]\n++  moat  (keep kernel-state)\n--\n";
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
                        "position": { "line": 2, "character": 20 }
                    }),
                )
                .into(),
            )
            .expect("request hyphenated mold definition");
        let definition = serde_json::from_value::<Option<GotoDefinitionResponse>>(
            receive_response(&client, request_id),
        )
        .expect("definition response");
        if let Some(definition) = definition {
            break definition;
        }
        assert!(
            Instant::now() < deadline,
            "compiler-resolved hyphenated mold definition did not become available"
        );
        request_id += 1;
        std::thread::sleep(Duration::from_millis(25));
    };
    let GotoDefinitionResponse::Scalar(definition) = definition else {
        panic!("server must return a single definition location");
    };
    assert_eq!(definition.uri, entry_uri);
    assert_eq!(definition.range.start.line, 1);

    shutdown_server(&client, server_thread, request_id + 1);
}

#[test]
fn local_binding_definition_respects_value_scope_and_shadowing() {
    let root = repository_root();
    let temp = TempDir::new().expect("temporary workspace");
    let entry = temp.path().join("local-bindings.hoon");
    std::fs::write(&entry, "42\n").expect("disk entry");
    let entry_uri = Uri::from_str(
        url::Url::from_file_path(&entry)
            .expect("entry file URI")
            .as_str(),
    )
    .expect("entry LSP URI");
    let source = concat!(
        "=/  value  1\n", "=/  before  value\n", "=/  result\n", "  =/  value  2\n", "  value\n",
        "[before result value]\n",
    );
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
                        1,
                        source.to_string(),
                    ),
                },
            )
            .into(),
        )
        .expect("send didOpen");

    let cases = [
        // A =/ face is not in scope in its value, so this resolves outward.
        (1, 13, 0, 4),
        // The nested face shadows the outer face only inside its body.
        (4, 3, 3, 6),
        // Leaving that value expression restores the outer binding.
        (5, 15, 0, 4),
    ];
    let mut request_id = 2;
    for (use_line, use_character, definition_line, definition_character) in cases {
        let definition =
            request_definition(&client, request_id, &entry_uri, use_line, use_character)
                .unwrap_or_else(|| panic!("no definition at {use_line}:{use_character}"));
        assert_eq!(definition.uri, entry_uri);
        assert_eq!(
            usize::try_from(definition.range.start.line).expect("small line"),
            definition_line
        );
        assert_eq!(
            usize::try_from(definition.range.start.character).expect("small character"),
            definition_character
        );
        request_id += 1;
    }

    shutdown_server(&client, server_thread, request_id);
}

#[test]
fn local_references_and_rename_preserve_binding_identity() {
    let root = repository_root();
    let temp = TempDir::new().expect("temporary workspace");
    let entry = temp.path().join("local-rename.hoon");
    std::fs::write(&entry, "42\n").expect("disk entry");
    let entry_uri = Uri::from_str(
        url::Url::from_file_path(&entry)
            .expect("entry file URI")
            .as_str(),
    )
    .expect("entry LSP URI");
    let source = concat!(
        "=/  value  1\n", "=/  before  value\n", "=/  result\n", "  =/  value  2\n", "  value\n",
        "[before result value]\n",
    );
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
                        7,
                        source.to_string(),
                    ),
                },
            )
            .into(),
        )
        .expect("send didOpen");

    let references = request_references(&client, 2, &entry_uri, 1, 13, true);
    assert_eq!(
        references
            .iter()
            .map(|location| (location.range.start.line, location.range.start.character))
            .collect::<Vec<_>>(),
        vec![(0, 4), (1, 12), (5, 15)]
    );
    let uses = request_references(&client, 3, &entry_uri, 1, 13, false);
    assert_eq!(uses.len(), 2);
    assert!(uses.iter().all(|location| location.range.start.line != 0));

    client
        .sender
        .send(
            Request::new(
                RequestId::from(4),
                "textDocument/prepareRename".to_string(),
                json!({
                    "textDocument": { "uri": entry_uri },
                    "position": { "line": 1, "character": 13 }
                }),
            )
            .into(),
        )
        .expect("prepare rename");
    let prepared =
        serde_json::from_value::<Option<PrepareRenameResponse>>(receive_response(&client, 4))
            .expect("prepare rename response")
            .expect("local face can be renamed");
    assert_eq!(
        prepared,
        PrepareRenameResponse::RangeWithPlaceholder {
            range: lsp_types::Range::new(
                lsp_types::Position::new(1, 12),
                lsp_types::Position::new(1, 17),
            ),
            placeholder: "value".to_string(),
        }
    );

    client
        .sender
        .send(
            Request::new(
                RequestId::from(5),
                "textDocument/rename".to_string(),
                json!({
                    "textDocument": { "uri": entry_uri },
                    "position": { "line": 1, "character": 13 },
                    "newName": "renamed"
                }),
            )
            .into(),
        )
        .expect("request rename");
    let edit = serde_json::from_value::<Option<WorkspaceEdit>>(receive_response(&client, 5))
        .expect("rename response")
        .expect("rename edit");
    let edit_json = serde_json::to_value(edit).expect("workspace edit JSON");
    assert_eq!(
        edit_json["documentChanges"][0]["textDocument"]["uri"],
        entry_uri.as_str()
    );
    assert_eq!(
        edit_json["documentChanges"][0]["textDocument"]["version"],
        7
    );
    let edits = edit_json["documentChanges"][0]["edits"]
        .as_array()
        .expect("rename text edits");
    assert_eq!(edits.len(), 3);
    assert!(edits.iter().all(|edit| edit["newText"] == "renamed"));
    assert_eq!(
        edits
            .iter()
            .map(|edit| edit["range"]["start"]["line"].as_u64().expect("line"))
            .collect::<Vec<_>>(),
        vec![0, 1, 5]
    );

    client
        .sender
        .send(
            Request::new(
                RequestId::from(6),
                "textDocument/rename".to_string(),
                json!({
                    "textDocument": { "uri": entry_uri },
                    "position": { "line": 1, "character": 13 },
                    "newName": "result"
                }),
            )
            .into(),
        )
        .expect("request colliding rename");
    let collision = receive_response_message(&client, 6);
    let ResponseKind::Err { error } = collision.response_kind else {
        panic!("colliding rename must be rejected");
    };
    assert_eq!(error.code, ErrorCode::InvalidParams as i32);
    assert!(error.message.contains("result"));
    assert!(error.message.contains("capture"));

    shutdown_server(&client, server_thread, 7);
}

#[test]
fn completion_respects_lexical_scope_and_includes_the_standard_library() {
    let root = repository_root();
    let temp = TempDir::new().expect("temporary workspace");
    let entry = temp.path().join("local-completion.hoon");
    std::fs::write(&entry, "42\n").expect("disk entry");
    let entry_uri = Uri::from_str(
        url::Url::from_file_path(&entry)
            .expect("entry file URI")
            .as_str(),
    )
    .expect("entry LSP URI");
    let source = concat!(
        "|=  outer=@\n", "=/  before  outer\n", "=/  result\n", "  |=  nested=@\n",
        "  [outer nested]\n", "[before result outer]\n",
    );
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
                        3,
                        source.to_string(),
                    ),
                },
            )
            .into(),
        )
        .expect("send didOpen");

    let final_scope = request_completion(&client, 2, &entry_uri, 5, 1);
    for local in ["outer", "before", "result"] {
        let item = final_scope
            .iter()
            .find(|item| item.label == local)
            .unwrap_or_else(|| panic!("missing local completion: {local}"));
        assert_eq!(item.detail.as_deref(), Some("local face"));
    }
    assert!(final_scope.iter().all(|item| item.label != "nested"));
    assert!(final_scope.iter().any(|item| {
        item.label == "list"
            && item
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("standard library"))
    }));

    let nested_scope = request_completion(&client, 3, &entry_uri, 4, 3);
    assert!(nested_scope
        .iter()
        .any(|item| { item.label == "nested" && item.detail.as_deref() == Some("local face") }));

    client
        .sender
        .send(
            Notification::new(
                "textDocument/didChange".to_string(),
                json!({
                    "textDocument": { "uri": entry_uri, "version": 4 },
                    "contentChanges": [{ "text": "|=  outer=@\n  [" }]
                }),
            )
            .into(),
        )
        .expect("send malformed didChange");
    let malformed = request_completion(&client, 4, &entry_uri, 1, 3);
    assert!(malformed.iter().any(|item| {
        item.label == "list"
            && item
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("standard library"))
    }));

    shutdown_server(&client, server_thread, 5);
}

#[test]
fn real_miner_definitions_resolve_local_transitive_prelude_and_rune_symbols() {
    let root = repository_root();
    let entry = root.join("hoon/apps/dumbnet/miner.hoon");
    let source = std::fs::read_to_string(&entry).expect("read real miner source");
    let entry_uri = Uri::from_str(
        url::Url::from_file_path(&entry)
            .expect("entry file URI")
            .as_str(),
    )
    .expect("entry LSP URI");
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
                        1,
                        source.clone(),
                    ),
                },
            )
            .into(),
        )
        .expect("send didOpen");

    let expected_tip5_path = root.join("hoon/common/ztd/four.hoon");
    let expected_tip5_uri = file_uri(&expected_tip5_path);
    let tip5_symbols = request_workspace_symbols(&client, 10_000, "tip5-hash-atom");
    assert!(tip5_symbols.iter().any(|symbol| {
        symbol["name"] == "tip5-hash-atom"
            && symbol["location"]["uri"] == expected_tip5_uri.as_str()
    }));

    let declaration_line = source
        .lines()
        .position(|line| line.trim_start().starts_with("+$  kernel-state"))
        .expect("kernel-state declaration");
    let declaration_character = source
        .lines()
        .nth(declaration_line)
        .expect("kernel-state declaration line")
        .find("kernel-state")
        .expect("kernel-state declaration character");
    let uses = [
        ("++  moat  (keep kernel-state)", true),
        ("|_  k=kernel-state", true),
        // The declaration-side occurrence remains a mold reference; the
        // same token in the gate body is covered by the lexical cases below.
        ("|=  =kernel-state  kernel-state", false),
    ];
    let mut request_id = 2;
    for (use_line, use_last) in uses {
        let line = source
            .lines()
            .position(|line| line.contains(use_line))
            .unwrap_or_else(|| panic!("missing real miner use: {use_line}"));
        let line_source = source.lines().nth(line).expect("located source line");
        let character = if use_last {
            line_source.rfind("kernel-state")
        } else {
            line_source.find("kernel-state")
        }
        .expect("kernel-state use character");
        client
            .sender
            .send(
                Request::new(
                    RequestId::from(request_id),
                    "textDocument/definition".to_string(),
                    json!({
                        "textDocument": { "uri": entry_uri },
                        "position": { "line": line, "character": character + 2 }
                    }),
                )
                .into(),
            )
            .expect("request real miner definition");
        let definition = serde_json::from_value::<Option<GotoDefinitionResponse>>(
            receive_response(&client, request_id),
        )
        .expect("definition response")
        .unwrap_or_else(|| panic!("no definition for real miner use: {use_line}"));
        let GotoDefinitionResponse::Scalar(definition) = definition else {
            panic!("server must return a single definition location");
        };
        assert_eq!(definition.uri, entry_uri);
        assert_eq!(
            usize::try_from(definition.range.start.line).expect("small line"),
            declaration_line
        );
        assert_eq!(
            usize::try_from(definition.range.start.character).expect("small character"),
            declaration_character
        );
        request_id += 1;
    }

    let local_bindings = [
        // Gate sample shorthand: the body use resolves to the sample face, not
        // the same-named mold used to declare it.
        (
            "|=  =kernel-state  kernel-state", "kernel-state", true,
            "|=  =kernel-state  kernel-state", "kernel-state",
        ),
        ("?~  pax", "pax", false, "=/  pax", "pax"),
        (
            "?~  cause", "cause", false, "=/  cause  ((soft cause)", "cause",
        ),
        (
            "=/  cause  u.cause", "cause", true, "=/  cause  ((soft cause)", "cause",
        ),
        ("?-  -.cause", "cause", false, "=/  cause  u.cause", "cause"),
        (
            "(prove-block-inner:mine input)", "input", false, "=/  input=prover-input:sp", "input",
        ),
        (
            "?:  (check-target:mine dig", "dig", false, "dig=tip5-hash-atom]", "dig",
        ),
        (":_  k", "k", false, "|_  k=kernel-state", "k"),
        ("((soft path) arg)", "arg", false, "|=  arg=*", "arg"),
    ];
    for (use_text, symbol, use_last, declaration_text, declaration_symbol) in local_bindings {
        let use_line = source
            .lines()
            .position(|line| line.contains(use_text))
            .unwrap_or_else(|| panic!("missing real miner local use: {use_text}"));
        let use_source = source.lines().nth(use_line).expect("located use line");
        let use_character = if use_last {
            use_source.rfind(symbol)
        } else {
            use_source.find(symbol)
        }
        .unwrap_or_else(|| panic!("missing {symbol} on local use line"));
        let declaration_line = source
            .lines()
            .position(|line| line.contains(declaration_text))
            .unwrap_or_else(|| panic!("missing local declaration: {declaration_text}"));
        let declaration_character = source
            .lines()
            .nth(declaration_line)
            .expect("local declaration line")
            .find(declaration_symbol)
            .expect("local declaration character");
        let definition =
            request_definition(&client, request_id, &entry_uri, use_line, use_character)
                .unwrap_or_else(|| panic!("no definition for real miner local: {symbol}"));
        assert_eq!(definition.uri, entry_uri);
        assert_eq!(
            usize::try_from(definition.range.start.line).expect("small line"),
            declaration_line,
            "wrong declaration line for {symbol} at {use_text}"
        );
        assert_eq!(
            usize::try_from(definition.range.start.character).expect("small character"),
            declaration_character,
            "wrong declaration character for {symbol} at {use_text}"
        );
        request_id += 1;
    }

    let external_definitions = [
        (
            "dig=tip5-hash-atom",
            "tip5-hash-atom",
            root.join("hoon/common/ztd/four.hoon"),
            "+$  tip5-hash-atom",
        ),
        (
            "^-  [(list effect)",
            "list",
            root.join("hoon/common/hoon.hoon"),
            "++  list",
        ),
    ];
    for (use_text, symbol, definition_path, declaration_text) in external_definitions {
        let line = source
            .lines()
            .position(|line| line.contains(use_text))
            .unwrap_or_else(|| panic!("missing real miner use: {use_text}"));
        let character = source
            .lines()
            .nth(line)
            .expect("located source line")
            .find(symbol)
            .unwrap_or_else(|| panic!("missing {symbol} on real miner use line"));
        client
            .sender
            .send(
                Request::new(
                    RequestId::from(request_id),
                    "textDocument/definition".to_string(),
                    json!({
                        "textDocument": { "uri": entry_uri },
                        "position": { "line": line, "character": character + 1 }
                    }),
                )
                .into(),
            )
            .expect("request external real miner definition");
        let definition = serde_json::from_value::<Option<GotoDefinitionResponse>>(
            receive_response(&client, request_id),
        )
        .expect("definition response")
        .unwrap_or_else(|| panic!("no definition for real miner symbol: {symbol}"));
        let GotoDefinitionResponse::Scalar(definition) = definition else {
            panic!("server must return a single definition location");
        };

        let definition_source =
            std::fs::read_to_string(&definition_path).expect("read definition source");
        let declaration_line = definition_source
            .lines()
            .position(|line| line.trim_start().starts_with(declaration_text))
            .unwrap_or_else(|| panic!("missing declaration: {declaration_text}"));
        let declaration_character = definition_source
            .lines()
            .nth(declaration_line)
            .expect("declaration line")
            .find(symbol)
            .expect("declaration character");
        let definition_uri = Uri::from_str(
            url::Url::from_file_path(&definition_path)
                .expect("definition file URI")
                .as_str(),
        )
        .expect("definition LSP URI");
        assert_eq!(definition.uri, definition_uri);
        assert_eq!(
            usize::try_from(definition.range.start.line).expect("small line"),
            declaration_line
        );
        assert_eq!(
            usize::try_from(definition.range.start.character).expect("small character"),
            declaration_character
        );
        request_id += 1;
    }

    let imported_arm_use_line = source
        .lines()
        .position(|line| line.contains("check-target:mine"))
        .expect("check-target use");
    let imported_arm_use_character = source
        .lines()
        .nth(imported_arm_use_line)
        .expect("check-target use line")
        .find("check-target")
        .expect("check-target use character");
    let imported_arm_definition_path = root.join("hoon/common/pow.hoon");
    let imported_arm_definition_uri = Uri::from_str(
        url::Url::from_file_path(&imported_arm_definition_path)
            .expect("check-target definition URI")
            .as_str(),
    )
    .expect("check-target LSP URI");
    let reference_deadline = Instant::now() + Duration::from_secs(60);
    let imported_arm_references = loop {
        let references = request_references(
            &client,
            request_id,
            &entry_uri,
            imported_arm_use_line,
            imported_arm_use_character + 1,
            true,
        );
        if references.iter().any(|location| location.uri == entry_uri)
            && references
                .iter()
                .any(|location| location.uri == imported_arm_definition_uri)
        {
            break references;
        }
        assert!(
            Instant::now() < reference_deadline,
            "compiler-owned check-target references did not become available"
        );
        request_id += 1;
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(imported_arm_references.len() >= 2);
    request_id += 1;

    let tip5_use_line = source
        .lines()
        .position(|line| line.contains("dig=tip5-hash-atom"))
        .expect("tip5-hash-atom use");
    let tip5_use_character = source
        .lines()
        .nth(tip5_use_line)
        .expect("tip5-hash-atom use line")
        .find("tip5-hash-atom")
        .expect("tip5-hash-atom use character");
    let tip5_definition_path = root.join("hoon/common/ztd/four.hoon");
    let tip5_definition_uri = Uri::from_str(
        url::Url::from_file_path(&tip5_definition_path)
            .expect("tip5-hash-atom definition URI")
            .as_str(),
    )
    .expect("tip5-hash-atom LSP URI");
    let tip5_references = request_references(
        &client,
        request_id,
        &entry_uri,
        tip5_use_line,
        tip5_use_character + 1,
        true,
    );
    assert!(tip5_references.iter().any(|location| {
        location.uri == entry_uri
            && usize::try_from(location.range.start.line).expect("small line") == tip5_use_line
            && usize::try_from(location.range.start.character).expect("small character")
                == tip5_use_character
    }));
    assert!(tip5_references
        .iter()
        .any(|location| location.uri == tip5_definition_uri));
    request_id += 1;

    let tip5_definition_source = std::fs::read_to_string(&tip5_definition_path)
        .expect("read tip5-hash-atom definition source");
    let tip5_declaration_line = tip5_definition_source
        .lines()
        .position(|line| line.trim_start().starts_with("+$  tip5-hash-atom"))
        .expect("tip5-hash-atom declaration");
    let tip5_declaration_character = tip5_definition_source
        .lines()
        .nth(tip5_declaration_line)
        .expect("tip5-hash-atom declaration line")
        .find("tip5-hash-atom")
        .expect("tip5-hash-atom declaration character");
    let tip5_uses_only = request_references(
        &client,
        request_id,
        &entry_uri,
        tip5_use_line,
        tip5_use_character + 1,
        false,
    );
    assert!(tip5_uses_only
        .iter()
        .any(|location| location.uri == entry_uri));
    assert!(!tip5_uses_only.iter().any(|location| {
        location.uri == tip5_definition_uri
            && usize::try_from(location.range.start.line).expect("small line")
                == tip5_declaration_line
            && usize::try_from(location.range.start.character).expect("small character")
                == tip5_declaration_character
    }));
    request_id += 1;

    client
        .sender
        .send(
            Request::new(
                RequestId::from(request_id),
                "textDocument/rename".to_string(),
                json!({
                    "textDocument": { "uri": entry_uri },
                    "position": {
                        "line": tip5_use_line,
                        "character": tip5_use_character + 1
                    },
                    "newName": "tip5-hash-atom-lsp-probe"
                }),
            )
            .into(),
        )
        .expect("rename real imported mold");
    let tip5_rename =
        serde_json::from_value::<Option<WorkspaceEdit>>(receive_response(&client, request_id))
            .expect("real imported mold rename response")
            .expect("real imported mold rename edit");
    let tip5_rename = serde_json::to_value(tip5_rename).expect("imported mold rename JSON");
    let tip5_documents = tip5_rename["documentChanges"]
        .as_array()
        .expect("imported mold document edits");
    let miner_edits = tip5_documents
        .iter()
        .find(|document| document["textDocument"]["uri"] == entry_uri.as_str())
        .expect("miner imported mold edits");
    assert_eq!(miner_edits["textDocument"]["version"], 1);
    assert!(miner_edits["edits"].as_array().is_some_and(|edits| {
        edits
            .iter()
            .any(|edit| edit["newText"] == "tip5-hash-atom-lsp-probe")
    }));
    let definition_edits = tip5_documents
        .iter()
        .find(|document| document["textDocument"]["uri"] == tip5_definition_uri.as_str())
        .expect("tip5 declaration edits");
    assert!(definition_edits["textDocument"]["version"].is_null());
    assert!(definition_edits["edits"].as_array().is_some_and(|edits| {
        edits.iter().any(|edit| {
            edit["range"]["start"]["line"]
                == u64::try_from(tip5_declaration_line).expect("small line")
                && edit["newText"] == "tip5-hash-atom-lsp-probe"
        })
    }));
    request_id += 1;

    client
        .sender
        .send(
            Notification::new(
                DidOpenTextDocument::METHOD.to_string(),
                DidOpenTextDocumentParams {
                    text_document: TextDocumentItem::new(
                        tip5_definition_uri.clone(),
                        "hoon".to_string(),
                        1,
                        tip5_definition_source,
                    ),
                },
            )
            .into(),
        )
        .expect("open tip5-hash-atom declaration");
    let declaration_references = request_references(
        &client,
        request_id,
        &tip5_definition_uri,
        tip5_declaration_line,
        tip5_declaration_character + 1,
        true,
    );
    assert!(declaration_references
        .iter()
        .any(|location| location.uri == entry_uri));
    assert!(declaration_references.iter().any(|location| {
        location.uri == tip5_definition_uri
            && usize::try_from(location.range.start.line).expect("small line")
                == tip5_declaration_line
            && usize::try_from(location.range.start.character).expect("small character")
                == tip5_declaration_character
    }));
    request_id += 1;

    let prelude_path = root.join("hoon/common/hoon.hoon");
    let prelude_source = std::fs::read_to_string(&prelude_path).expect("read prelude source");
    let prelude_uri = Uri::from_str(
        url::Url::from_file_path(&prelude_path)
            .expect("prelude file URI")
            .as_str(),
    )
    .expect("prelude LSP URI");
    let list_use_line = source
        .lines()
        .position(|line| line.contains("^-  [(list effect)"))
        .expect("standard-library list use");
    let list_use_character = source
        .lines()
        .nth(list_use_line)
        .expect("standard-library list use line")
        .find("list")
        .expect("standard-library list use character");
    let list_declaration_line = prelude_source
        .lines()
        .position(|line| line.trim_start().starts_with("++  list"))
        .expect("standard-library list declaration");
    let list_declaration_character = prelude_source
        .lines()
        .nth(list_declaration_line)
        .expect("standard-library list declaration line")
        .find("list")
        .expect("standard-library list declaration character");
    let list_references = request_references(
        &client,
        request_id,
        &entry_uri,
        list_use_line,
        list_use_character + 1,
        true,
    );
    assert!(list_references.iter().any(|location| {
        location.uri == entry_uri
            && usize::try_from(location.range.start.line).expect("small line") == list_use_line
            && usize::try_from(location.range.start.character).expect("small character")
                == list_use_character
    }));
    assert!(list_references.iter().any(|location| {
        location.uri == prelude_uri
            && usize::try_from(location.range.start.line).expect("small line")
                == list_declaration_line
            && usize::try_from(location.range.start.character).expect("small character")
                == list_declaration_character
    }));
    request_id += 1;

    let rune_cases = [
        ("^-  [(list effect)", "^-", "kthp", "[%kthp "),
        ("=/  cause", "=/", "tsfs", "[%tsfs "),
        ("|=  =kernel-state", "|=", "brts", "[%brts "),
        ("?:  (check-target", "?:", "wtcl", "[%wtcl "),
        ("++  moat", "++", "bola", "++  bola"),
        ("+$  effect", "+$", "boba", "++  boba"),
    ];
    for (use_text, rune, target, declaration_marker) in rune_cases {
        let rune_line = source
            .lines()
            .position(|line| line.contains(use_text))
            .unwrap_or_else(|| panic!("missing real miner rune use: {use_text}"));
        let rune_character = source
            .lines()
            .nth(rune_line)
            .expect("rune source line")
            .find(rune)
            .expect("rune character");
        let rune_definition_line = prelude_source
            .lines()
            .position(|line| line.contains(declaration_marker))
            .unwrap_or_else(|| panic!("missing {rune} rune declaration"));
        let rune_definition_character = prelude_source
            .lines()
            .nth(rune_definition_line)
            .expect("rune declaration line")
            .find(target)
            .expect("rune declaration character");
        for cursor_delta in 0..=1 {
            client
                .sender
                .send(
                    Request::new(
                        RequestId::from(request_id),
                        "textDocument/definition".to_string(),
                        json!({
                            "textDocument": { "uri": entry_uri },
                            "position": {
                                "line": rune_line,
                                "character": rune_character + cursor_delta
                            }
                        }),
                    )
                    .into(),
                )
                .unwrap_or_else(|_| panic!("request {rune} rune definition"));
            let definition = serde_json::from_value::<Option<GotoDefinitionResponse>>(
                receive_response(&client, request_id),
            )
            .expect("rune definition response")
            .unwrap_or_else(|| panic!("no definition for {rune} rune"));
            let GotoDefinitionResponse::Scalar(definition) = definition else {
                panic!("server must return a single rune definition location");
            };
            assert_eq!(definition.uri, prelude_uri);
            assert_eq!(
                usize::try_from(definition.range.start.line).expect("small line"),
                rune_definition_line
            );
            assert_eq!(
                usize::try_from(definition.range.start.character).expect("small character"),
                rune_definition_character
            );
            request_id += 1;
        }
    }

    let outer_cause_declaration_line = source
        .lines()
        .position(|line| line.contains("=/  cause  ((soft cause)"))
        .expect("outer cause declaration");
    let outer_cause_declaration_character = source
        .lines()
        .nth(outer_cause_declaration_line)
        .expect("outer cause declaration line")
        .find("cause")
        .expect("outer cause declaration character");
    let outer_cause_use_line = source
        .lines()
        .position(|line| line.contains("?~  cause"))
        .expect("outer cause use");
    let outer_cause_use_character = source
        .lines()
        .nth(outer_cause_use_line)
        .expect("outer cause use line")
        .find("cause")
        .expect("outer cause use character");
    let outer_cause_initializer_line = source
        .lines()
        .position(|line| line.contains("=/  cause  u.cause"))
        .expect("shadowing cause initializer");
    let outer_cause_initializer_character = source
        .lines()
        .nth(outer_cause_initializer_line)
        .expect("shadowing cause initializer line")
        .rfind("cause")
        .expect("shadowing cause initializer character");
    let cause_references = request_references(
        &client, request_id, &entry_uri, outer_cause_use_line, outer_cause_use_character, true,
    );
    assert_eq!(
        cause_references
            .iter()
            .map(|location| (location.range.start.line, location.range.start.character))
            .collect::<Vec<_>>(),
        vec![
            (
                u32::try_from(outer_cause_declaration_line).expect("small line"),
                u32::try_from(outer_cause_declaration_character).expect("small character"),
            ),
            (
                u32::try_from(outer_cause_use_line).expect("small line"),
                u32::try_from(outer_cause_use_character).expect("small character"),
            ),
            (
                u32::try_from(outer_cause_initializer_line).expect("small line"),
                u32::try_from(outer_cause_initializer_character).expect("small character"),
            ),
        ]
    );
    request_id += 1;

    let dig_use_line = source
        .lines()
        .position(|line| line.contains("check-target:mine dig"))
        .expect("dig use");
    let dig_use_character = source
        .lines()
        .nth(dig_use_line)
        .expect("dig use line")
        .find("dig")
        .expect("dig use character");
    client
        .sender
        .send(
            Request::new(
                RequestId::from(request_id),
                "textDocument/rename".to_string(),
                json!({
                    "textDocument": { "uri": entry_uri },
                    "position": { "line": dig_use_line, "character": dig_use_character },
                    "newName": "mining-digest"
                }),
            )
            .into(),
        )
        .expect("rename real miner dig");
    let dig_rename =
        serde_json::from_value::<Option<WorkspaceEdit>>(receive_response(&client, request_id))
            .expect("real miner rename response")
            .expect("real miner rename edit");
    let dig_rename = serde_json::to_value(dig_rename).expect("real miner rename JSON");
    assert_eq!(
        dig_rename["documentChanges"][0]["textDocument"]["version"],
        1
    );
    let dig_edits = dig_rename["documentChanges"][0]["edits"]
        .as_array()
        .expect("real miner rename edits");
    assert_eq!(dig_edits.len(), 5);
    assert!(dig_edits
        .iter()
        .all(|edit| edit["newText"] == "mining-digest"));
    assert_eq!(
        dig_edits
            .iter()
            .map(|edit| edit["range"]["start"]["line"].as_u64().expect("line"))
            .collect::<Vec<_>>(),
        vec![53, 56, 57, 57, 58]
    );
    request_id += 1;

    let shorthand_line = source
        .lines()
        .position(|line| line.contains("|=  =kernel-state  kernel-state"))
        .expect("kernel-state shorthand binding");
    let shorthand_use_character = source
        .lines()
        .nth(shorthand_line)
        .expect("shorthand binding line")
        .rfind("kernel-state")
        .expect("shorthand body use");
    client
        .sender
        .send(
            Request::new(
                RequestId::from(request_id),
                "textDocument/rename".to_string(),
                json!({
                    "textDocument": { "uri": entry_uri },
                    "position": {
                        "line": shorthand_line,
                        "character": shorthand_use_character
                    },
                    "newName": "state-value"
                }),
            )
            .into(),
        )
        .expect("rename real miner shorthand binding");
    let shorthand_rename =
        serde_json::from_value::<Option<WorkspaceEdit>>(receive_response(&client, request_id))
            .expect("real miner shorthand rename response")
            .expect("real miner shorthand rename edit");
    let shorthand_rename =
        serde_json::to_value(shorthand_rename).expect("real miner shorthand rename JSON");
    let shorthand_edits = shorthand_rename["documentChanges"][0]["edits"]
        .as_array()
        .expect("real miner shorthand rename edits");
    assert_eq!(shorthand_edits.len(), 2);
    assert_eq!(
        shorthand_edits
            .iter()
            .map(|edit| edit["newText"].as_str().expect("replacement text"))
            .collect::<Vec<_>>(),
        vec!["state-value=kernel-state", "state-value"]
    );
    assert!(shorthand_edits.iter().all(|edit| {
        edit["range"]["start"]["line"] == u64::try_from(shorthand_line).expect("small line")
    }));
    request_id += 1;

    let completion_cases = [
        (
            "dig=tip5-hash-atom",
            "tip5-hash-atom",
            4usize,
            CompletionItemKind::STRUCT,
            "common/ztd/four.hoon",
        ),
        (
            "^-  [(list effect)",
            "list",
            2usize,
            CompletionItemKind::FUNCTION,
            "standard library",
        ),
        (
            "check-target:mine dig",
            "dig",
            2usize,
            CompletionItemKind::VARIABLE,
            "local face",
        ),
    ];
    for (use_text, symbol, prefix_len, kind, detail_fragment) in completion_cases {
        let line = source
            .lines()
            .position(|line| line.contains(use_text))
            .unwrap_or_else(|| panic!("missing completion use: {use_text}"));
        let character = source
            .lines()
            .nth(line)
            .expect("completion source line")
            .find(symbol)
            .unwrap_or_else(|| panic!("missing completion symbol: {symbol}"));
        let completions = request_completion(
            &client,
            request_id,
            &entry_uri,
            line,
            character + prefix_len,
        );
        let item = completions
            .iter()
            .find(|item| item.label == symbol)
            .unwrap_or_else(|| panic!("missing real miner completion: {symbol}"));
        assert_eq!(item.kind, Some(kind), "wrong completion kind for {symbol}");
        assert!(
            item.detail
                .as_deref()
                .is_some_and(|detail| detail.contains(detail_fragment)),
            "wrong completion provenance for {symbol}: {:?}",
            item.detail
        );
        let item = serde_json::to_value(item).expect("completion item JSON");
        assert_eq!(item["textEdit"]["newText"], symbol);
        assert_eq!(
            item["textEdit"]["range"]["start"]["character"],
            u64::try_from(character).expect("small character")
        );
        assert_eq!(
            item["textEdit"]["range"]["end"]["character"],
            u64::try_from(character + symbol.len()).expect("small character")
        );
        request_id += 1;
    }

    shutdown_server(&client, server_thread, request_id);
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
