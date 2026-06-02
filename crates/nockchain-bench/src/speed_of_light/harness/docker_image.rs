use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use super::HarnessError;

const SAMPLY_PROFILING_IMAGE_SUFFIX: &str = "samply-bytehound";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DockerImageSource {
    Provided {
        #[serde(rename = "ref")]
        reference: String,
    },
    AutoBuild {
        tag: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
pub enum DockerImageVariant {
    Standard,
    Profiling,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedDockerImage {
    pub source: DockerImageSource,
    pub variant: DockerImageVariant,
    pub requested_ref: String,
    pub resolved_ref: String,
    pub immutable_identity: String,
    pub image_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerInspectIdentity {
    pub requested_ref: String,
    pub resolved_ref: String,
    pub immutable_identity: String,
    pub image_id: String,
}

#[derive(Debug, Deserialize)]
struct DockerInspectEntry {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "RepoDigests", default)]
    repo_digests: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerAutoBuildCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub current_dir: PathBuf,
}

static RESOLUTION_CACHE: OnceLock<
    Mutex<BTreeMap<(DockerImageSource, DockerImageVariant), ResolvedDockerImage>>,
> = OnceLock::new();

fn resolution_cache(
) -> &'static Mutex<BTreeMap<(DockerImageSource, DockerImageVariant), ResolvedDockerImage>> {
    RESOLUTION_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sync_stamp_source_root() -> Option<PathBuf> {
    let stamp_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(".pma-bench-sync-stamp");
    let stamp = std::fs::read_to_string(stamp_path).ok()?;
    for line in stamp.lines() {
        if let Some(value) = line.strip_prefix("source_root=") {
            let path = PathBuf::from(value);
            if path.is_dir() {
                return Some(path);
            }
        }
    }
    None
}

fn docker_build_script_root() -> PathBuf {
    let workspace_root = workspace_root();
    if workspace_root
        .join("scripts/build_nockchain_bench_image.sh")
        .is_file()
    {
        return workspace_root;
    }

    sync_stamp_source_root().unwrap_or(workspace_root)
}

fn command_failure_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }

    format!("exit status {}", output.status)
}

fn run_checked_command(
    program: &Path,
    args: &[&str],
    current_dir: &Path,
    description: &str,
) -> Result<(), HarnessError> {
    let output = Command::new(program)
        .args(args)
        .current_dir(current_dir)
        .output()?;
    if output.status.success() {
        return Ok(());
    }

    Err(HarnessError::CommandFailure(format!(
        "{description} failed: {}",
        command_failure_detail(&output)
    )))
}

fn docker_stdout<const N: usize>(args: [&str; N]) -> Result<String, HarnessError> {
    let output = Command::new("docker").args(args).output()?;
    if !output.status.success() {
        return Err(HarnessError::CommandFailure(format!(
            "docker {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn derive_auto_build_variant_tag(tag: &str, variant: DockerImageVariant) -> String {
    if variant == DockerImageVariant::Standard {
        return tag.to_string();
    }

    if tag.ends_with(&format!("-{SAMPLY_PROFILING_IMAGE_SUFFIX}"))
        || tag.ends_with(&format!(":{SAMPLY_PROFILING_IMAGE_SUFFIX}"))
    {
        return tag.to_string();
    }

    match tag.rsplit_once(':') {
        Some((repository, image_tag)) if !image_tag.contains('/') => {
            format!("{repository}:{image_tag}-{SAMPLY_PROFILING_IMAGE_SUFFIX}")
        }
        _ => format!("{tag}:{SAMPLY_PROFILING_IMAGE_SUFFIX}"),
    }
}

pub(crate) fn parse_inspect_identity(
    requested_ref: &str,
    inspect_json: &str,
) -> Result<DockerInspectIdentity, HarnessError> {
    let entries: Vec<DockerInspectEntry> = serde_json::from_str(inspect_json)?;
    let entry = entries.into_iter().next().ok_or_else(|| {
        HarnessError::CommandFailure(format!(
            "docker image inspect returned no entries for `{requested_ref}`"
        ))
    })?;
    if entry.id.trim().is_empty() {
        return Err(HarnessError::CommandFailure(format!(
            "docker image inspect returned an empty image id for `{requested_ref}`"
        )));
    }

    let repo_digest = entry
        .repo_digests
        .iter()
        .find(|value| value.contains("@sha256:"))
        .or_else(|| {
            entry
                .repo_digests
                .iter()
                .find(|value| !value.trim().is_empty())
        })
        .cloned();
    let resolved_ref = repo_digest.unwrap_or_else(|| entry.id.clone());

    Ok(DockerInspectIdentity {
        requested_ref: requested_ref.to_string(),
        resolved_ref: resolved_ref.clone(),
        immutable_identity: resolved_ref,
        image_id: entry.id,
    })
}

pub(crate) fn resolve_requested_image_ref(
    source: &DockerImageSource,
    variant: DockerImageVariant,
) -> Result<String, HarnessError> {
    let requested_ref = match source {
        DockerImageSource::Provided { reference } => reference.clone(),
        DockerImageSource::AutoBuild { tag } => derive_auto_build_variant_tag(tag, variant),
    };

    if requested_ref.trim().is_empty() {
        return Err(HarnessError::InvalidRequestedCase(
            "Docker image source requires a non-empty ref or tag".to_string(),
        ));
    }

    Ok(requested_ref)
}

fn build_auto_build_image(
    requested_ref: &str,
    variant: DockerImageVariant,
) -> Result<(), HarnessError> {
    let current_exe = std::env::current_exe()?;
    let command = docker_auto_build_command(requested_ref, variant, &current_exe);
    let arg_refs: Vec<&str> = command.args.iter().map(String::as_str).collect();

    run_checked_command(
        &command.program, &arg_refs, &command.current_dir, "auto-building Docker benchmark image",
    )
}

pub fn docker_auto_build_command(
    requested_ref: &str,
    variant: DockerImageVariant,
    current_exe: &Path,
) -> DockerAutoBuildCommand {
    let build_root = docker_build_script_root();
    let script = build_root.join("scripts/build_nockchain_bench_image.sh");
    let variant_arg = match variant {
        DockerImageVariant::Standard => "standard",
        DockerImageVariant::Profiling => "profiling",
    };
    let mut args = vec![
        "--variant".to_string(),
        variant_arg.to_string(),
        "--tag".to_string(),
        requested_ref.to_string(),
    ];

    if variant == DockerImageVariant::Standard {
        args.push("--binary".to_string());
        args.push(current_exe.display().to_string());
        args.push("--skip-cargo-build".to_string());
    }

    DockerAutoBuildCommand {
        program: script,
        args,
        current_dir: build_root,
    }
}

pub(crate) fn resolve_docker_image(
    source: &DockerImageSource,
    variant: DockerImageVariant,
) -> Result<ResolvedDockerImage, HarnessError> {
    if let Some(cached) = resolution_cache()
        .lock()
        .expect("docker image resolution cache")
        .get(&(source.clone(), variant))
        .cloned()
    {
        return Ok(cached);
    }

    let requested_ref = resolve_requested_image_ref(source, variant)?;
    if matches!(source, DockerImageSource::AutoBuild { .. }) {
        build_auto_build_image(&requested_ref, variant)?;
    }

    let inspect_json = docker_stdout(["image", "inspect", requested_ref.as_str()])?;
    let identity = parse_inspect_identity(&requested_ref, &inspect_json)?;
    let resolved = ResolvedDockerImage {
        source: source.clone(),
        variant,
        requested_ref: identity.requested_ref,
        resolved_ref: identity.resolved_ref,
        immutable_identity: identity.immutable_identity,
        image_id: identity.image_id,
    };
    resolution_cache()
        .lock()
        .expect("docker image resolution cache")
        .insert((source.clone(), variant), resolved.clone());
    Ok(resolved)
}

pub(crate) fn prefetch_docker_image(
    source: &DockerImageSource,
    variant: DockerImageVariant,
) -> Result<(), HarnessError> {
    let _ = resolve_docker_image(source, variant)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        parse_inspect_identity, resolve_requested_image_ref, DockerImageSource, DockerImageVariant,
    };

    #[test]
    fn docker_image_repo_digest_is_preferred_when_available() {
        let inspect_json = r#"[
          {
            "Id": "sha256:image-id",
            "RepoDigests": [
              "ghcr.io/org/nockchain-bench@sha256:repo-digest"
            ]
          }
        ]"#;

        let identity = parse_inspect_identity("ghcr.io/org/nockchain-bench:latest", inspect_json)
            .expect("inspect identity");

        assert_eq!(
            identity.immutable_identity,
            "ghcr.io/org/nockchain-bench@sha256:repo-digest"
        );
        assert_eq!(
            identity.resolved_ref,
            "ghcr.io/org/nockchain-bench@sha256:repo-digest"
        );
        assert_eq!(identity.image_id, "sha256:image-id");
    }

    #[test]
    fn docker_image_image_id_is_used_when_repo_digests_are_empty() {
        let inspect_json = r#"[
          {
            "Id": "sha256:image-id",
            "RepoDigests": []
          }
        ]"#;

        let identity = parse_inspect_identity("nockchain-bench:local", inspect_json)
            .expect("inspect identity");

        assert_eq!(identity.immutable_identity, "sha256:image-id");
        assert_eq!(identity.resolved_ref, "sha256:image-id");
        assert_eq!(identity.image_id, "sha256:image-id");
    }

    #[test]
    fn docker_image_digest_pinned_requested_ref_is_preserved_verbatim() {
        let inspect_json = r#"[
          {
            "Id": "sha256:image-id",
            "RepoDigests": [
              "ghcr.io/org/nockchain-bench@sha256:repo-digest"
            ]
          }
        ]"#;

        let identity = parse_inspect_identity(
            "ghcr.io/org/nockchain-bench@sha256:repo-digest", inspect_json,
        )
        .expect("inspect identity");

        assert_eq!(
            identity.requested_ref,
            "ghcr.io/org/nockchain-bench@sha256:repo-digest"
        );
    }

    #[test]
    fn docker_image_provided_source_with_profiling_requests_profiling_variant_without_rewriting_ref(
    ) {
        let requested_ref = resolve_requested_image_ref(
            &DockerImageSource::Provided {
                reference: "ghcr.io/org/nockchain-bench@sha256:repo-digest".to_string(),
            },
            DockerImageVariant::Profiling,
        )
        .expect("requested ref");

        assert_eq!(
            requested_ref,
            "ghcr.io/org/nockchain-bench@sha256:repo-digest"
        );
    }

    #[test]
    fn docker_image_auto_build_source_uses_standard_variant_for_normal_runs() {
        let requested_ref = resolve_requested_image_ref(
            &DockerImageSource::AutoBuild {
                tag: "nockchain-bench:local".to_string(),
            },
            DockerImageVariant::Standard,
        )
        .expect("requested ref");

        assert_eq!(requested_ref, "nockchain-bench:local");
    }

    #[test]
    fn docker_image_auto_build_source_uses_profiling_variant_for_samply_runs() {
        let requested_ref = resolve_requested_image_ref(
            &DockerImageSource::AutoBuild {
                tag: "nockchain-bench:local".to_string(),
            },
            DockerImageVariant::Profiling,
        )
        .expect("requested ref");

        assert_eq!(requested_ref, "nockchain-bench:local-samply-bytehound");
    }

    #[test]
    fn docker_image_local_only_launch_ref_uses_image_id_identity() {
        let inspect_json = r#"[
          {
            "Id": "sha256:local-only-id",
            "RepoDigests": []
          }
        ]"#;

        let identity = parse_inspect_identity("nockchain-bench:local", inspect_json)
            .expect("inspect identity");

        assert_eq!(identity.immutable_identity, "sha256:local-only-id");
        assert_eq!(identity.resolved_ref, "sha256:local-only-id");
    }
}
