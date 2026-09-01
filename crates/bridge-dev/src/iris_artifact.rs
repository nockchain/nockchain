use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tar::Archive;
use thiserror::Error;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const EXPECTED_PACKAGE_NAME: &str = "@nockbox/iris-sdk";
const EXPECTED_DRIVER_PATH: &str = "dist/e2e/encode-withdrawal-e2e.js";
const EXPECTED_WASM_PATH: &str = "dist/e2e/iris_wasm_bg.wasm";
const MAX_TARBALL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_UNPACKED_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PACKAGE_FILES: usize = 10_000;
const MAX_DRIVER_BYTES: u64 = 4 * 1024 * 1024;
const MAX_WASM_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone)]
pub enum IrisArtifactInput {
    Checkout {
        path: PathBuf,
        expected_revision: Option<String>,
        expected_version: Option<String>,
    },
    Tarball {
        path: PathBuf,
        metadata_path: PathBuf,
        expected_revision: Option<String>,
        expected_version: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrisPackedFileFacts {
    pub path: String,
    pub size: u64,
    pub mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrisPackageMetadata {
    pub schema_version: u64,
    pub package_name: String,
    pub package_version: String,
    pub git_revision: String,
    pub tarball_path: PathBuf,
    pub tarball_sha256: String,
    pub npm_integrity: String,
    pub npm_shasum: String,
    pub driver_path: String,
    pub files: Vec<IrisPackedFileFacts>,
    pub node_version: String,
    pub npm_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrisArtifactFacts {
    pub package_name: String,
    pub package_version: String,
    pub git_revision: String,
    pub tarball_path: PathBuf,
    pub tarball_sha256: String,
    pub npm_integrity: String,
    pub npm_shasum: String,
    pub files: Vec<IrisPackedFileFacts>,
    pub packed_node_version: String,
    pub packed_npm_version: String,
    pub runtime_node_version: String,
    pub driver_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct IrisDriverCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

impl IrisDriverCommand {
    pub async fn invoke_json(&self, request: &Value) -> Result<Value, IrisArtifactError> {
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| IrisArtifactError::CommandSpawn {
                program: self.program.clone(),
                source,
            })?;
        let mut stdin = child.stdin.take().ok_or(IrisArtifactError::MissingStdin)?;
        let mut input = serde_json::to_vec(request)?;
        input.push(b'\n');
        stdin
            .write_all(&input)
            .await
            .map_err(IrisArtifactError::DriverStdin)?;
        drop(stdin);
        let output = child
            .wait_with_output()
            .await
            .map_err(IrisArtifactError::DriverWait)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(IrisArtifactError::DriverFailed {
                status: output.status.code(),
                stderr: stderr.trim().to_owned(),
            });
        }
        let stdout = std::str::from_utf8(&output.stdout)
            .map_err(|_| IrisArtifactError::DriverOutput("stdout is not UTF-8".to_owned()))?;
        let mut lines = stdout.lines().filter(|line| !line.trim().is_empty());
        let line = lines.next().ok_or_else(|| {
            IrisArtifactError::DriverOutput("stdout did not contain a JSON response".to_owned())
        })?;
        if lines.next().is_some() {
            return Err(IrisArtifactError::DriverOutput(
                "stdout contained more than one response line".to_owned(),
            ));
        }
        serde_json::from_str(line).map_err(IrisArtifactError::from)
    }
}

#[derive(Debug, Clone)]
pub struct IrisArtifact {
    pub facts: IrisArtifactFacts,
    pub driver: IrisDriverCommand,
}

#[derive(Debug, Clone)]
pub struct IrisArtifactResolver {
    node_binary: PathBuf,
    npm_binary: PathBuf,
}

impl Default for IrisArtifactResolver {
    fn default() -> Self {
        Self {
            node_binary: PathBuf::from("node"),
            npm_binary: PathBuf::from("npm"),
        }
    }
}

impl IrisArtifactResolver {
    pub fn with_binaries(node_binary: PathBuf, npm_binary: PathBuf) -> Self {
        Self {
            node_binary,
            npm_binary,
        }
    }

    pub async fn resolve(
        &self,
        input: IrisArtifactInput,
        run_root: &Path,
    ) -> Result<IrisArtifact, IrisArtifactError> {
        let (tarball_path, metadata_path, expected_revision, expected_version) = match input {
            IrisArtifactInput::Checkout {
                path,
                expected_revision,
                expected_version,
            } => {
                let path = fs::canonicalize(&path)
                    .await
                    .map_err(IrisArtifactError::Filesystem)?;
                if !path.join("package.json").is_file() {
                    return Err(IrisArtifactError::InvalidCheckout(path));
                }
                let output_dir = run_root.join("iris-package");
                fs::create_dir_all(&output_dir)
                    .await
                    .map_err(IrisArtifactError::Filesystem)?;
                let output_dir = fs::canonicalize(output_dir)
                    .await
                    .map_err(IrisArtifactError::Filesystem)?;
                let mut args = vec![
                    OsString::from("run"),
                    OsString::from("--silent"),
                    OsString::from("package:withdrawal-e2e"),
                    OsString::from("--"),
                    OsString::from("--checkout"),
                    path.as_os_str().to_owned(),
                    OsString::from("--output-dir"),
                    output_dir.as_os_str().to_owned(),
                ];
                if let Some(revision) = &expected_revision {
                    args.push(OsString::from("--expected-revision"));
                    args.push(OsString::from(revision));
                }
                if let Some(version) = &expected_version {
                    args.push(OsString::from("--expected-version"));
                    args.push(OsString::from(version));
                }
                let output = Command::new(&self.npm_binary)
                    .args(&args)
                    .current_dir(&path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true)
                    .output()
                    .await
                    .map_err(|source| IrisArtifactError::CommandSpawn {
                        program: self.npm_binary.clone(),
                        source,
                    })?;
                if !output.status.success() {
                    return Err(IrisArtifactError::PackFailed {
                        status: output.status.code(),
                        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                    });
                }
                let metadata = parse_metadata_stdout(&output.stdout)?;
                if metadata.tarball_path.parent() != Some(output_dir.as_path()) {
                    return Err(IrisArtifactError::InvalidMetadata(
                        "checkout packer returned a tarball outside the run-owned output directory"
                            .to_owned(),
                    ));
                }
                let metadata_path = metadata_path_for(&metadata.tarball_path);
                (
                    metadata.tarball_path, metadata_path, expected_revision, expected_version,
                )
            }
            IrisArtifactInput::Tarball {
                path,
                metadata_path,
                expected_revision,
                expected_version,
            } => (path, metadata_path, expected_revision, expected_version),
        };
        self.resolve_tarball(
            &tarball_path,
            &metadata_path,
            expected_revision.as_deref(),
            expected_version.as_deref(),
            run_root,
        )
        .await
    }

    async fn resolve_tarball(
        &self,
        tarball_path: &Path,
        metadata_path: &Path,
        expected_revision: Option<&str>,
        expected_version: Option<&str>,
        run_root: &Path,
    ) -> Result<IrisArtifact, IrisArtifactError> {
        let metadata_file = fs::symlink_metadata(metadata_path)
            .await
            .map_err(IrisArtifactError::Filesystem)?;
        if !metadata_file.file_type().is_file() {
            return Err(IrisArtifactError::InvalidMetadata(
                "metadata path must be a regular file".to_owned(),
            ));
        }
        let metadata_bytes = fs::read(metadata_path)
            .await
            .map_err(IrisArtifactError::Filesystem)?;
        let metadata: IrisPackageMetadata = serde_json::from_slice(&metadata_bytes)?;
        validate_metadata(&metadata, expected_revision, expected_version)?;
        let tarball_metadata = fs::symlink_metadata(tarball_path)
            .await
            .map_err(IrisArtifactError::Filesystem)?;
        if !tarball_metadata.file_type().is_file() {
            return Err(IrisArtifactError::InvalidTarball(
                "tarball path must be a regular file".to_owned(),
            ));
        }
        if tarball_metadata.len() == 0 || tarball_metadata.len() > MAX_TARBALL_BYTES {
            return Err(IrisArtifactError::InvalidTarball(
                "tarball size is outside the accepted range".to_owned(),
            ));
        }
        let tarball = fs::read(tarball_path)
            .await
            .map_err(IrisArtifactError::Filesystem)?;
        let observed_sha256 = hex::encode(Sha256::digest(&tarball));
        if observed_sha256 != metadata.tarball_sha256 {
            return Err(IrisArtifactError::HashMismatch {
                expected: metadata.tarball_sha256,
                observed: observed_sha256,
            });
        }
        let expected_files = metadata_file_map(&metadata.files)?;
        let inspection = tokio::task::spawn_blocking(move || inspect_tarball(&tarball))
            .await
            .map_err(|error| IrisArtifactError::InspectionTask(error.to_string()))??;
        if inspection.files != expected_files {
            return Err(IrisArtifactError::FileListMismatch);
        }

        let runtime_node_version = command_version(&self.node_binary).await?;
        let packed_node_major = parse_major_version(&metadata.node_version, "packed Node")?;
        let runtime_node_major = parse_major_version(&runtime_node_version, "runtime Node")?;
        if packed_node_major != runtime_node_major || runtime_node_major < 22 {
            return Err(IrisArtifactError::NodeVersionDrift {
                packed: metadata.node_version,
                runtime: runtime_node_version,
            });
        }

        let driver_dir = run_root.join("iris-driver").join(&metadata.git_revision);
        fs::create_dir_all(&driver_dir)
            .await
            .map_err(IrisArtifactError::Filesystem)?;
        let driver_path = driver_dir.join("encode-withdrawal-e2e.js");
        let mut driver_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&driver_path)
            .await
            .map_err(IrisArtifactError::Filesystem)?;
        driver_file
            .write_all(&inspection.driver)
            .await
            .map_err(IrisArtifactError::Filesystem)?;
        driver_file
            .flush()
            .await
            .map_err(IrisArtifactError::Filesystem)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&driver_path, std::fs::Permissions::from_mode(0o700))
                .await
                .map_err(IrisArtifactError::Filesystem)?;
        }
        let wasm_path = driver_dir.join("iris_wasm_bg.wasm");
        let mut wasm_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&wasm_path)
            .await
            .map_err(IrisArtifactError::Filesystem)?;
        wasm_file
            .write_all(&inspection.wasm)
            .await
            .map_err(IrisArtifactError::Filesystem)?;
        wasm_file
            .flush()
            .await
            .map_err(IrisArtifactError::Filesystem)?;

        let facts = IrisArtifactFacts {
            package_name: metadata.package_name,
            package_version: metadata.package_version,
            git_revision: metadata.git_revision,
            tarball_path: tarball_path.to_path_buf(),
            tarball_sha256: observed_sha256,
            npm_integrity: metadata.npm_integrity,
            npm_shasum: metadata.npm_shasum,
            files: metadata.files,
            packed_node_version: metadata.node_version,
            packed_npm_version: metadata.npm_version,
            runtime_node_version,
            driver_path: driver_path.clone(),
        };
        Ok(IrisArtifact {
            facts,
            driver: IrisDriverCommand {
                program: self.node_binary.clone(),
                args: vec![driver_path.into_os_string()],
            },
        })
    }
}

struct TarballInspection {
    files: BTreeMap<String, u64>,
    driver: Vec<u8>,
    wasm: Vec<u8>,
}

fn inspect_tarball(bytes: &[u8]) -> Result<TarballInspection, IrisArtifactError> {
    let decoder = GzDecoder::new(Cursor::new(bytes));
    let mut archive = Archive::new(decoder);
    let mut files = BTreeMap::new();
    let mut driver = None;
    let mut wasm = None;
    let mut total_size = 0_u64;
    for entry in archive
        .entries()
        .map_err(|error| IrisArtifactError::InvalidTarball(error.to_string()))?
    {
        let mut entry =
            entry.map_err(|error| IrisArtifactError::InvalidTarball(error.to_string()))?;
        let path = entry
            .path()
            .map_err(|error| IrisArtifactError::InvalidTarball(error.to_string()))?;
        let normalized = normalize_tar_path(&path)?;
        if entry.header().entry_type().is_dir() {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            return Err(IrisArtifactError::InvalidTarball(format!(
                "non-regular archive entry {normalized}"
            )));
        }
        validate_package_path(&normalized)?;
        let size = entry.size();
        total_size = total_size
            .checked_add(size)
            .ok_or_else(|| IrisArtifactError::InvalidTarball("size overflow".to_owned()))?;
        if total_size > MAX_UNPACKED_BYTES || files.len() >= MAX_PACKAGE_FILES {
            return Err(IrisArtifactError::InvalidTarball(
                "archive exceeds unpacked size or file-count limit".to_owned(),
            ));
        }
        if files.insert(normalized.clone(), size).is_some() {
            return Err(IrisArtifactError::InvalidTarball(format!(
                "duplicate archive entry {normalized}"
            )));
        }
        if normalized == EXPECTED_DRIVER_PATH {
            if size == 0 || size > MAX_DRIVER_BYTES {
                return Err(IrisArtifactError::InvalidTarball(
                    "bundled driver size is outside the accepted range".to_owned(),
                ));
            }
            let mut contents = Vec::with_capacity(size as usize);
            entry
                .read_to_end(&mut contents)
                .map_err(|error| IrisArtifactError::InvalidTarball(error.to_string()))?;
            driver = Some(contents);
        } else if normalized == EXPECTED_WASM_PATH {
            if size == 0 || size > MAX_WASM_BYTES {
                return Err(IrisArtifactError::InvalidTarball(
                    "bundled WASM size is outside the accepted range".to_owned(),
                ));
            }
            let mut contents = Vec::with_capacity(size as usize);
            entry
                .read_to_end(&mut contents)
                .map_err(|error| IrisArtifactError::InvalidTarball(error.to_string()))?;
            wasm = Some(contents);
        }
    }
    let driver = driver.ok_or_else(|| {
        IrisArtifactError::InvalidTarball("archive is missing the bundled driver".to_owned())
    })?;
    let wasm = wasm.ok_or_else(|| {
        IrisArtifactError::InvalidTarball("archive is missing the bundled WASM".to_owned())
    })?;
    Ok(TarballInspection {
        files,
        driver,
        wasm,
    })
}

fn normalize_tar_path(path: &Path) -> Result<String, IrisArtifactError> {
    let mut components = path.components();
    if components.next() != Some(Component::Normal("package".as_ref())) {
        return Err(IrisArtifactError::InvalidTarball(
            "archive entry is outside the package root".to_owned(),
        ));
    }
    let mut parts = Vec::new();
    for component in components {
        match component {
            Component::Normal(part) => parts.push(part.to_str().ok_or_else(|| {
                IrisArtifactError::InvalidTarball("archive path is not UTF-8".to_owned())
            })?),
            _ => {
                return Err(IrisArtifactError::InvalidTarball(
                    "archive path contains an unsafe component".to_owned(),
                ))
            }
        }
    }
    if parts.is_empty() {
        return Err(IrisArtifactError::InvalidTarball(
            "archive entry has an empty package path".to_owned(),
        ));
    }
    Ok(parts.join("/"))
}

fn metadata_file_map(
    files: &[IrisPackedFileFacts],
) -> Result<BTreeMap<String, u64>, IrisArtifactError> {
    if files.is_empty() || files.len() > MAX_PACKAGE_FILES {
        return Err(IrisArtifactError::InvalidMetadata(
            "file list is empty or too large".to_owned(),
        ));
    }
    let mut map = BTreeMap::new();
    for file in files {
        validate_package_path(&file.path)?;
        if map.insert(file.path.clone(), file.size).is_some() {
            return Err(IrisArtifactError::InvalidMetadata(format!(
                "duplicate file metadata for {}",
                file.path
            )));
        }
    }
    if !map.contains_key(EXPECTED_DRIVER_PATH) || !map.contains_key("package.json") {
        return Err(IrisArtifactError::InvalidMetadata(
            "file list is missing the driver or package.json".to_owned(),
        ));
    }
    Ok(map)
}

fn validate_package_path(path: &str) -> Result<(), IrisArtifactError> {
    let lower = path.to_ascii_lowercase();
    let safe_components = !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path
            .split('/')
            .any(|component| component.is_empty() || component == "..");
    let allowed = matches!(path, "LICENSE" | "README.md" | "package.json")
        || path.starts_with("dist/")
        || path.starts_with("test-fixtures/");
    let sensitive = lower.contains(".env")
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".p12")
        || lower.ends_with(".npmrc")
        || lower.contains("credential")
        || lower.contains("secret");
    if !safe_components || !allowed || sensitive {
        return Err(IrisArtifactError::UnexpectedPackagePath(path.to_owned()));
    }
    Ok(())
}

fn validate_metadata(
    metadata: &IrisPackageMetadata,
    expected_revision: Option<&str>,
    expected_version: Option<&str>,
) -> Result<(), IrisArtifactError> {
    if metadata.schema_version != 1
        || metadata.package_name != EXPECTED_PACKAGE_NAME
        || metadata.package_version.trim().is_empty()
        || !is_lower_hex(&metadata.git_revision, 40)
        || !is_lower_hex(&metadata.tarball_sha256, 64)
        || !is_lower_hex(&metadata.npm_shasum, 40)
        || !metadata.npm_integrity.starts_with("sha512-")
        || metadata.driver_path != EXPECTED_DRIVER_PATH
    {
        return Err(IrisArtifactError::InvalidMetadata(
            "package identity, hashes, or driver path are invalid".to_owned(),
        ));
    }
    if expected_revision.is_some_and(|expected| expected != metadata.git_revision) {
        return Err(IrisArtifactError::RevisionMismatch {
            expected: expected_revision.unwrap_or_default().to_owned(),
            observed: metadata.git_revision.clone(),
        });
    }
    if expected_version.is_some_and(|expected| expected != metadata.package_version) {
        return Err(IrisArtifactError::VersionMismatch {
            expected: expected_version.unwrap_or_default().to_owned(),
            observed: metadata.package_version.clone(),
        });
    }
    Ok(())
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

async fn command_version(program: &Path) -> Result<String, IrisArtifactError> {
    let output = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|source| IrisArtifactError::CommandSpawn {
            program: program.to_path_buf(),
            source,
        })?;
    if !output.status.success() {
        return Err(IrisArtifactError::VersionCommandFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|_| IrisArtifactError::VersionCommandFailed("stdout is not UTF-8".to_owned()))
}

fn parse_major_version(version: &str, label: &'static str) -> Result<u64, IrisArtifactError> {
    version
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| IrisArtifactError::InvalidMetadata(format!("invalid {label} version")))
}

fn parse_metadata_stdout(stdout: &[u8]) -> Result<IrisPackageMetadata, IrisArtifactError> {
    let stdout = std::str::from_utf8(stdout)
        .map_err(|_| IrisArtifactError::InvalidMetadata("pack stdout is not UTF-8".to_owned()))?;
    let line = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| IrisArtifactError::InvalidMetadata("pack stdout is empty".to_owned()))?;
    serde_json::from_str(line).map_err(IrisArtifactError::from)
}

fn metadata_path_for(tarball_path: &Path) -> PathBuf {
    let mut path = tarball_path.as_os_str().to_owned();
    path.push(".metadata.json");
    PathBuf::from(path)
}

#[derive(Debug, Error)]
pub enum IrisArtifactError {
    #[error("invalid Iris checkout: {0}")]
    InvalidCheckout(PathBuf),
    #[error("Iris artifact filesystem operation failed")]
    Filesystem(#[source] std::io::Error),
    #[error("failed to spawn {program}")]
    CommandSpawn {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Iris package preparation failed with status {status:?}: {stderr}")]
    PackFailed { status: Option<i32>, stderr: String },
    #[error("invalid Iris package metadata: {0}")]
    InvalidMetadata(String),
    #[error("invalid Iris package tarball: {0}")]
    InvalidTarball(String),
    #[error("unexpected Iris package path: {0}")]
    UnexpectedPackagePath(String),
    #[error("Iris tarball hash mismatch: expected {expected}, observed {observed}")]
    HashMismatch { expected: String, observed: String },
    #[error("Iris package file list does not match tarball entries")]
    FileListMismatch,
    #[error("Iris revision mismatch: expected {expected}, observed {observed}")]
    RevisionMismatch { expected: String, observed: String },
    #[error("Iris version mismatch: expected {expected}, observed {observed}")]
    VersionMismatch { expected: String, observed: String },
    #[error("Iris package Node version drift: packed {packed}, runtime {runtime}")]
    NodeVersionDrift { packed: String, runtime: String },
    #[error("failed to inspect Iris tarball: {0}")]
    InspectionTask(String),
    #[error("Node version command failed: {0}")]
    VersionCommandFailed(String),
    #[error("Iris driver stdin was unavailable")]
    MissingStdin,
    #[error("failed to write Iris driver stdin")]
    DriverStdin(#[source] std::io::Error),
    #[error("failed to wait for Iris driver")]
    DriverWait(#[source] std::io::Error),
    #[error("Iris driver failed with status {status:?}: {stderr}")]
    DriverFailed { status: Option<i32>, stderr: String },
    #[error("invalid Iris driver output: {0}")]
    DriverOutput(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
