use std::fs;
use std::path::{Path, PathBuf};

use honk::workspace::{
    ArtifactMode, WorkspaceCheckRequest, WorkspaceCompileRequest, WorkspaceConfig,
    WorkspaceDiagnosticKind,
};
use honk_service::{
    CompilerService, CompilerServiceConfig, CompilerServiceError, DocumentUpdate,
    DEFAULT_WORKER_STACK_BYTES,
};
use tempfile::TempDir;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("honk-service crate should be below the repository root")
        .to_path_buf()
}

fn request(entry: &Path) -> WorkspaceCompileRequest {
    WorkspaceCompileRequest {
        entry: entry.to_path_buf(),
        mode: ArtifactMode::Arbitrary,
        directory_files: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_documents_shadow_disk_and_close_restores_it() {
    let root = repository_root();
    let temp = TempDir::new().expect("temporary workspace");
    let lib = temp.path().join("lib");
    fs::create_dir_all(&lib).expect("library directory");
    let leaf = lib.join("leaf.hoon");
    let helper = lib.join("helper.hoon");
    let stable = lib.join("stable.hoon");
    let entry = temp.path().join("demo.hoon");
    let disk_leaf = "|=  [a=@ b=@]\n  (add a b)\n";
    let disk_helper = "/+  leaf\n|=  [a=@ b=@]\n  (leaf a b)\n";
    let disk_entry = "/+  helper, stable\n|=  [a=@ b=@]\n  (helper a b)\n";
    fs::write(&leaf, disk_leaf).expect("leaf source");
    fs::write(&helper, disk_helper).expect("helper source");
    fs::write(&stable, "|=  [a=@ b=@]\n  (sub a b)\n").expect("stable source");
    fs::write(&entry, disk_entry).expect("entry source");

    let service = CompilerService::spawn(CompilerServiceConfig {
        workspace: WorkspaceConfig {
            prelude: root.join("hoon/common/hoon.hoon"),
            dependencies: temp.path().to_path_buf(),
            subject_type_jam: None,
            dbug: true,
            vet: true,
        },
        max_compiles: 0,
        worker_stack_bytes: DEFAULT_WORKER_STACK_BYTES,
    })
    .expect("initialize compiler service");
    let compiler = service.handle();

    let baseline = compiler
        .compile(request(&entry))
        .await
        .expect("baseline response")
        .result
        .expect("baseline compile");
    let baseline_check = compiler
        .check(WorkspaceCheckRequest {
            entry: entry.clone(),
        })
        .await
        .expect("baseline check response")
        .result
        .expect("baseline check");
    assert!(
        baseline_check.cache_stats.path_hits >= 2,
        "artifact-free check should reuse compiled dependencies"
    );

    let update = compiler
        .update_document(DocumentUpdate {
            path: leaf.clone(),
            version: 1,
            text: "|=  [a=@ b=@]\n  (mul a b)\n".to_string(),
        })
        .await
        .expect("open leaf document");
    assert_eq!(update.revision, 1);
    let changed = compiler
        .compile(request(&entry))
        .await
        .expect("overlay response");
    assert_eq!(changed.document_revision, 1);
    let changed = changed.result.expect("overlay compile");
    assert!(changed.cache_invalidated);
    assert_ne!(changed.artifact, baseline.artifact);
    assert_eq!(fs::read_to_string(&leaf).expect("disk leaf"), disk_leaf);

    compiler
        .update_document(DocumentUpdate {
            path: leaf.clone(),
            version: 2,
            text: disk_leaf.to_string(),
        })
        .await
        .expect("update leaf to disk-equivalent text");
    let equivalent = compiler
        .compile(request(&entry))
        .await
        .expect("equivalent overlay response");
    assert_eq!(equivalent.document_revision, 2);
    let equivalent = equivalent.result.expect("equivalent overlay compile");
    assert!(equivalent.cache_invalidated);
    assert!(
        equivalent.cache_stats.invalidated_paths >= 2,
        "changed leaf should invalidate itself and its cached dependent"
    );
    assert!(
        equivalent.cache_stats.path_hits >= 1,
        "unchanged stable dependency should be reused"
    );
    assert!(
        equivalent.cache_stats.path_misses >= 1,
        "changed dependency chain should be recompiled"
    );
    assert_eq!(equivalent.artifact, baseline.artifact);

    let stale = compiler
        .update_document(DocumentUpdate {
            path: leaf.clone(),
            version: 2,
            text: "|=  [a=@ b=@]\n  (mul a b)\n".to_string(),
        })
        .await
        .expect_err("duplicate version should be rejected");
    assert!(matches!(
        stale,
        CompilerServiceError::StaleDocumentVersion { .. }
    ));

    compiler
        .update_document(DocumentUpdate {
            path: entry.clone(),
            version: 7,
            text: "|=  [a=@\n".to_string(),
        })
        .await
        .expect("open malformed entry document");
    let malformed = compiler
        .compile(request(&entry))
        .await
        .expect("malformed response");
    assert_eq!(malformed.document_revision, 3);
    let diagnostic = malformed.result.expect_err("malformed source should fail");
    assert_eq!(diagnostic.diagnostic.kind, WorkspaceDiagnosticKind::Parse);
    assert_eq!(fs::read_to_string(&entry).expect("disk entry"), disk_entry);

    compiler
        .close_document(entry.clone())
        .await
        .expect("close entry document");
    compiler
        .close_document(leaf.clone())
        .await
        .expect("close leaf document");
    let restored = compiler
        .compile(request(&entry))
        .await
        .expect("restored response");
    assert_eq!(restored.document_revision, 5);
    let restored = restored.result.expect("restored compile");
    assert!(restored.cache_invalidated);
    assert_eq!(restored.artifact, baseline.artifact);
}
