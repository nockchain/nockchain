use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use bridge_dev::iris_artifact::{
    IrisArtifactError, IrisArtifactInput, IrisArtifactResolver, IrisPackageMetadata,
    IrisPackedFileFacts,
};
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::json;
use sha2::{Digest, Sha256};
use tar::{Builder, Header};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
const PACKAGE_NAME: &str = "@nockbox/iris-sdk";
const PACKAGE_VERSION: &str = "0.3.0";
const REVISION: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DRIVER_PATH: &str = "dist/e2e/encode-withdrawal-e2e.js";

#[tokio::test]
async fn resolves_prebuilt_tarball_and_executes_bundled_driver() -> Result<()> {
    let fixture = artifact_fixture("prebuilt", false)?;
    let run_root = preserved_root("prebuilt-run");
    let artifact = IrisArtifactResolver::default()
        .resolve(
            IrisArtifactInput::Tarball {
                path: fixture.tarball.clone(),
                metadata_path: fixture.metadata.clone(),
                expected_revision: Some(REVISION.to_owned()),
                expected_version: Some(PACKAGE_VERSION.to_owned()),
            },
            &run_root,
        )
        .await?;

    assert_eq!(artifact.facts.package_name, PACKAGE_NAME);
    assert_eq!(artifact.facts.package_version, PACKAGE_VERSION);
    assert_eq!(artifact.facts.git_revision, REVISION);
    assert_eq!(artifact.facts.tarball_sha256, fixture.sha256);
    assert!(artifact.facts.driver_path.starts_with(&run_root));
    assert!(artifact.facts.driver_path.is_file());
    let response = artifact.driver.invoke_json(&json!({"probe": true})).await?;
    assert_eq!(response, json!({"ok": true, "request": {"probe": true}}));
    Ok(())
}

#[tokio::test]
async fn resolves_checkout_through_run_owned_package_output() -> Result<()> {
    let fixture = artifact_fixture("checkout source", false)?;
    let checkout = preserved_root("checkout with spaces");
    fs::create_dir_all(&checkout)?;
    fs::write(
        checkout.join("package.json"),
        format!("{{\"name\":\"{PACKAGE_NAME}\",\"version\":\"{PACKAGE_VERSION}\"}}\n"),
    )?;
    let fake_npm = fake_npm_packager(&fixture)?;
    let run_root = preserved_root("checkout-run");
    fs::create_dir_all(&run_root)?;
    let resolver = IrisArtifactResolver::with_binaries(PathBuf::from("node"), fake_npm);
    let artifact = resolver
        .resolve(
            IrisArtifactInput::Checkout {
                path: checkout,
                expected_revision: Some(REVISION.to_owned()),
                expected_version: Some(PACKAGE_VERSION.to_owned()),
            },
            &run_root,
        )
        .await?;

    assert!(artifact
        .facts
        .tarball_path
        .starts_with(fs::canonicalize(run_root.join("iris-package"))?));
    assert_eq!(artifact.facts.tarball_sha256, fixture.sha256);
    assert_eq!(
        artifact
            .driver
            .invoke_json(&json!({"checkout": true}))
            .await?,
        json!({"ok": true, "request": {"checkout": true}})
    );
    Ok(())
}

#[tokio::test]
async fn rejects_hash_identity_file_list_and_node_version_drift() -> Result<()> {
    let fixture = artifact_fixture("rejections", false)?;
    let resolver = IrisArtifactResolver::default();

    let mut wrong_hash = fixture.metadata_value.clone();
    wrong_hash.tarball_sha256 = "c".repeat(64);
    let wrong_hash_path = write_metadata_variant(&fixture, "wrong-hash", &wrong_hash)?;
    assert!(matches!(
        resolver
            .resolve(
                IrisArtifactInput::Tarball {
                    path: fixture.tarball.clone(),
                    metadata_path: wrong_hash_path,
                    expected_revision: None,
                    expected_version: None,
                },
                &preserved_root("wrong-hash-run"),
            )
            .await,
        Err(IrisArtifactError::HashMismatch { .. })
    ));

    let mut wrong_package = fixture.metadata_value.clone();
    wrong_package.package_name = "@example/not-iris".to_owned();
    let wrong_package_path = write_metadata_variant(&fixture, "wrong-package", &wrong_package)?;
    assert!(matches!(
        resolver
            .resolve(
                IrisArtifactInput::Tarball {
                    path: fixture.tarball.clone(),
                    metadata_path: wrong_package_path,
                    expected_revision: None,
                    expected_version: None,
                },
                &preserved_root("wrong-package-run"),
            )
            .await,
        Err(IrisArtifactError::InvalidMetadata(_))
    ));

    let mut wrong_files = fixture.metadata_value.clone();
    wrong_files.files.pop();
    let wrong_files_path = write_metadata_variant(&fixture, "wrong-files", &wrong_files)?;
    assert!(matches!(
        resolver
            .resolve(
                IrisArtifactInput::Tarball {
                    path: fixture.tarball.clone(),
                    metadata_path: wrong_files_path,
                    expected_revision: None,
                    expected_version: None,
                },
                &preserved_root("wrong-files-run"),
            )
            .await,
        Err(IrisArtifactError::FileListMismatch | IrisArtifactError::InvalidMetadata(_))
    ));

    let mut wrong_node = fixture.metadata_value.clone();
    wrong_node.node_version = "v22.0.0".to_owned();
    let wrong_node_path = write_metadata_variant(&fixture, "wrong-node", &wrong_node)?;
    assert!(matches!(
        resolver
            .resolve(
                IrisArtifactInput::Tarball {
                    path: fixture.tarball.clone(),
                    metadata_path: wrong_node_path,
                    expected_revision: None,
                    expected_version: None,
                },
                &preserved_root("wrong-node-run"),
            )
            .await,
        Err(IrisArtifactError::NodeVersionDrift { .. })
    ));

    assert!(matches!(
        resolver
            .resolve(
                IrisArtifactInput::Tarball {
                    path: fixture.tarball,
                    metadata_path: fixture.metadata,
                    expected_revision: Some("d".repeat(40)),
                    expected_version: None,
                },
                &preserved_root("wrong-revision-run"),
            )
            .await,
        Err(IrisArtifactError::RevisionMismatch { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn rejects_sensitive_package_entries_before_extraction() -> Result<()> {
    let fixture = artifact_fixture("sensitive", true)?;
    let error = IrisArtifactResolver::default()
        .resolve(
            IrisArtifactInput::Tarball {
                path: fixture.tarball,
                metadata_path: fixture.metadata,
                expected_revision: None,
                expected_version: None,
            },
            &preserved_root("sensitive-run"),
        )
        .await
        .expect_err("sensitive entry must fail");
    assert!(
        matches!(error, IrisArtifactError::UnexpectedPackagePath(path) if path.contains("secret"))
    );
    Ok(())
}

struct ArtifactFixture {
    root: PathBuf,
    tarball: PathBuf,
    metadata: PathBuf,
    metadata_value: IrisPackageMetadata,
    sha256: String,
}

fn artifact_fixture(label: &str, include_secret: bool) -> Result<ArtifactFixture> {
    let root = preserved_root(label);
    fs::create_dir_all(&root)?;
    let tarball = root.join("nockbox-iris-sdk-0.3.0.tgz");
    let driver = br##"#!/usr/bin/env node
let input = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', chunk => { input += chunk; });
process.stdin.on('end', () => {
  process.stdout.write(`${JSON.stringify({ ok: true, request: JSON.parse(input) })}\n`);
});
"##
    .to_vec();
    let mut files = BTreeMap::from([
        ("LICENSE".to_owned(), b"fixture license\n".to_vec()),
        ("README.md".to_owned(), b"fixture readme\n".to_vec()),
        (
            "package.json".to_owned(),
            format!("{{\"name\":\"{PACKAGE_NAME}\",\"version\":\"{PACKAGE_VERSION}\"}}\n")
                .into_bytes(),
        ),
        (DRIVER_PATH.to_owned(), driver),
        (
            "test-fixtures/vector.json".to_owned(),
            b"{\"fixture\":true}\n".to_vec(),
        ),
    ]);
    if include_secret {
        files.insert(
            "dist/secret.pem".to_owned(),
            b"not-a-real-secret\n".to_vec(),
        );
    }
    write_tarball(&tarball, &files)?;
    let bytes = fs::read(&tarball)?;
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let node_version = String::from_utf8(
        Command::new("node")
            .arg("--version")
            .output()
            .context("run node --version")?
            .stdout,
    )?
    .trim()
    .to_owned();
    let metadata_value = IrisPackageMetadata {
        schema_version: 1,
        package_name: PACKAGE_NAME.to_owned(),
        package_version: PACKAGE_VERSION.to_owned(),
        git_revision: REVISION.to_owned(),
        tarball_path: tarball.clone(),
        tarball_sha256: sha256.clone(),
        npm_integrity: "sha512-Zml4dHVyZQ==".to_owned(),
        npm_shasum: "a".repeat(40),
        driver_path: DRIVER_PATH.to_owned(),
        files: files
            .iter()
            .map(|(path, contents)| IrisPackedFileFacts {
                path: path.clone(),
                size: contents.len() as u64,
                mode: if path == DRIVER_PATH { 0o755 } else { 0o644 },
            })
            .collect(),
        node_version,
        npm_version: "11.0.0".to_owned(),
    };
    let metadata = root.join("nockbox-iris-sdk-0.3.0.tgz.metadata.json");
    fs::write(&metadata, serde_json::to_vec_pretty(&metadata_value)?)?;
    Ok(ArtifactFixture {
        root,
        tarball,
        metadata,
        metadata_value,
        sha256,
    })
}

fn write_tarball(path: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    let encoder = GzEncoder::new(File::create(path)?, Compression::default());
    let mut archive = Builder::new(encoder);
    for (name, contents) in files {
        let mut header = Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(if name == DRIVER_PATH { 0o755 } else { 0o644 });
        header.set_mtime(0);
        header.set_cksum();
        archive.append_data(
            &mut header,
            format!("package/{name}"),
            Cursor::new(contents),
        )?;
    }
    archive.into_inner()?.finish()?;
    Ok(())
}

fn write_metadata_variant(
    fixture: &ArtifactFixture,
    label: &str,
    metadata: &IrisPackageMetadata,
) -> Result<PathBuf> {
    let path = fixture.root.join(format!("{label}.metadata.json"));
    fs::write(&path, serde_json::to_vec_pretty(metadata)?)?;
    Ok(path)
}

fn fake_npm_packager(fixture: &ArtifactFixture) -> Result<PathBuf> {
    let path = fixture.root.join("fake npm with spaces.mjs");
    let source = format!(
        r##"#!/usr/bin/env node
import {{ copyFileSync, mkdirSync, readFileSync, writeFileSync }} from 'node:fs';
import {{ join }} from 'node:path';
const marker = process.argv.indexOf('--output-dir');
if (marker < 0 || !process.argv[marker + 1]) process.exit(9);
const output = process.argv[marker + 1];
mkdirSync(output, {{ recursive: true }});
const tarball = join(output, 'nockbox-iris-sdk-0.3.0.tgz');
copyFileSync({source_tar:?}, tarball);
const metadata = JSON.parse(readFileSync({source_metadata:?}, 'utf8'));
metadata.tarball_path = tarball;
writeFileSync(`${{tarball}}.metadata.json`, `${{JSON.stringify(metadata, null, 2)}}\n`);
process.stdout.write(`${{JSON.stringify(metadata)}}\n`);
"##,
        source_tar = fixture.tarball.display().to_string(),
        source_metadata = fixture.metadata.display().to_string(),
    );
    fs::write(&path, source)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(path)
}

fn preserved_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock follows Unix epoch")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "nockbridge-iris-artifact-{label}-{}-{nanos}-{sequence}",
        std::process::id()
    ))
}
