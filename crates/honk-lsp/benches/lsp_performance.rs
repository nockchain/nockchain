use std::fmt::Write as _;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, ensure, Context, Result};
use clap::{Parser, ValueEnum};
use honk::workspace::{WorkspaceCheckRequest, WorkspaceConfig};
use honk_lsp::{run_connection, LspConfig};
use honk_service::semantic::SemanticSession;
use honk_service::{
    CompilerHandle, CompilerService, CompilerServiceConfig, DocumentUpdate,
    DEFAULT_WORKER_STACK_BYTES,
};
use lsp_server::{Connection, Message, Notification, Request, RequestId, ResponseKind};
use lsp_types::notification::{
    DidChangeTextDocument, DidOpenTextDocument, Notification as LspNotification, PublishDiagnostics,
};
use lsp_types::{
    DidChangeTextDocumentParams, DidOpenTextDocumentParams, PublishDiagnosticsParams,
    TextDocumentContentChangeEvent, TextDocumentItem, Uri, VersionedTextDocumentIdentifier,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tempfile::TempDir;

const ENTRY_A: &str = "/+  helper, stable\n|=  [a=@ b=@]\n  (helper a b)\n";
const ENTRY_B: &str = "/+  helper, stable\n|=  [a=@ b=@]\n  (stable a b)\n";
const LEAF_A: &str = "|=  [a=@ b=@]\n  (add a b)\n";
const LEAF_B: &str = "|=  [a=@ b=@]\n  (mul a b)\n";
const HELPER: &str = "/+  leaf\n|=  [a=@ b=@]\n  (leaf a b)\n";
const STABLE: &str = "|=  [a=@ b=@]\n  (sub a b)\n";
const PROTOCOL_SOURCE_A: &str =
    "|%\n++  answer\n  42\n++  doubled\n  (add answer answer)\n+$  pair\n  $:  left=@  right=@  ==\n--\n";
const PROTOCOL_SOURCE_B: &str =
    "|%\n++  answer\n  43\n++  doubled\n  (add answer answer)\n+$  pair\n  $:  left=@  right=@  ==\n--\n";

#[derive(Debug, Parser)]
#[command(about = "Reproducible Honk editor/LSP latency and memory harness")]
struct Args {
    /// Measured samples per latency scenario.
    #[arg(long, default_value_t = 20)]
    samples: usize,

    /// Untimed samples used to warm caches before measurement.
    #[arg(long, default_value_t = 3)]
    warmups: usize,

    /// Invalidating root edits used by the sustained-memory scenario.
    #[arg(long, default_value_t = 256)]
    sustained_checks: usize,

    /// Arms in the generated semantic-index workload.
    #[arg(long, default_value_t = 1_000)]
    semantic_arms: usize,

    /// Output directory. Defaults to target/honk-lsp-performance/<run-id>.
    #[arg(long)]
    output: Option<PathBuf>,

    /// Use one warmup, three samples, sixteen sustained edits, and 100 arms.
    #[arg(long)]
    quick: bool,

    /// Skip the real Miner background-load LSP scenario.
    #[arg(long)]
    skip_contention: bool,

    /// Internal isolated worker group.
    #[arg(long, value_enum, hide = true)]
    worker: Option<WorkerGroup>,

    /// Compatibility flag appended by `cargo bench` for custom harnesses.
    #[arg(long, hide = true)]
    bench: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
enum WorkerGroup {
    Compiler,
    Semantic,
    Protocol,
    Sustained,
}

impl WorkerGroup {
    fn cli_name(self) -> &'static str {
        match self {
            Self::Compiler => "compiler",
            Self::Semantic => "semantic",
            Self::Protocol => "protocol",
            Self::Sustained => "sustained",
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct MetricReport {
    name: String,
    description: String,
    samples: usize,
    warmups: usize,
    operations_per_sample: usize,
    raw_ms_per_operation: Vec<f64>,
    mean_ms: f64,
    stddev_ms: f64,
    coefficient_of_variation: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
    operations_per_second: f64,
    notes: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct MemoryReport {
    current_rss_before_bytes: Option<u64>,
    current_rss_after_bytes: Option<u64>,
    current_rss_delta_bytes: Option<i64>,
    process_peak_rss_bytes: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GroupReport {
    group: WorkerGroup,
    metrics: Vec<MetricReport>,
    invariants: Vec<String>,
    memory: Option<MemoryReport>,
}

#[derive(Debug, Serialize)]
struct FullReport {
    schema_version: u32,
    run_id: String,
    fingerprint: Value,
    groups: Vec<GroupReport>,
}

struct CompilerFixture {
    _temp: TempDir,
    root: PathBuf,
    entry: PathBuf,
    leaf: PathBuf,
}

fn main() -> Result<()> {
    let mut args = Args::parse();
    if args.quick {
        args.samples = 3;
        args.warmups = 1;
        args.sustained_checks = 16;
        args.semantic_arms = 100;
    }
    ensure!(args.samples > 0, "--samples must be positive");
    ensure!(
        args.sustained_checks > 0,
        "--sustained-checks must be positive"
    );
    ensure!(args.semantic_arms > 0, "--semantic-arms must be positive");

    let repository = repository_root();
    if let Some(group) = args.worker {
        let report = run_worker(group, &args, &repository)?;
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }

    run_parent(&args, &repository)
}

fn run_parent(args: &Args, repository: &Path) -> Result<()> {
    let run_id = format!(
        "{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("clock is before Unix epoch")?
            .as_secs(),
        std::process::id()
    );
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| repository.join("target/honk-lsp-performance").join(&run_id));
    fs::create_dir_all(&output)
        .with_context(|| format!("create benchmark output {}", output.display()))?;

    let groups = vec![
        WorkerGroup::Compiler,
        WorkerGroup::Semantic,
        WorkerGroup::Sustained,
        WorkerGroup::Protocol,
    ];

    let executable = std::env::current_exe().context("resolve benchmark executable")?;
    let mut reports = Vec::new();
    for group in groups {
        eprintln!("running {} benchmark group", group.cli_name());
        let mut command = Command::new(&executable);
        command
            .arg("--worker")
            .arg(group.cli_name())
            .arg("--samples")
            .arg(args.samples.to_string())
            .arg("--warmups")
            .arg(args.warmups.to_string())
            .arg("--sustained-checks")
            .arg(args.sustained_checks.to_string())
            .arg("--semantic-arms")
            .arg(args.semantic_arms.to_string());
        if args.skip_contention {
            command.arg("--skip-contention");
        }
        let child = command
            .current_dir(repository)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("run {} worker", group.cli_name()))?;
        if !child.status.success() {
            bail!(
                "{} worker failed ({:?}):\n{}",
                group.cli_name(),
                child.status.code(),
                String::from_utf8_lossy(&child.stderr)
            );
        }
        let report: GroupReport = serde_json::from_slice(&child.stdout).with_context(|| {
            format!(
                "decode {} worker output: {}",
                group.cli_name(),
                String::from_utf8_lossy(&child.stdout)
            )
        })?;
        reports.push(report);
    }

    let fingerprint = fingerprint(repository, args, &run_id);
    let report = FullReport {
        schema_version: 1,
        run_id: run_id.clone(),
        fingerprint: fingerprint.clone(),
        groups: reports,
    };
    fs::write(
        output.join("results.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    fs::write(
        output.join("fingerprint.json"),
        serde_json::to_vec_pretty(&fingerprint)?,
    )?;
    fs::write(output.join("DEFINE.md"), render_define(args))?;
    fs::write(output.join("BASELINE.md"), render_baseline(&report))?;

    println!("Honk LSP performance artifacts: {}", output.display());
    print!("{}", render_baseline(&report));
    Ok(())
}

fn run_worker(group: WorkerGroup, args: &Args, repository: &Path) -> Result<GroupReport> {
    match group {
        WorkerGroup::Compiler => compiler_group(args, repository),
        WorkerGroup::Semantic => semantic_group(args),
        WorkerGroup::Protocol => protocol_group(args, repository),
        WorkerGroup::Sustained => sustained_group(args, repository),
    }
}

fn compiler_group(args: &Args, repository: &Path) -> Result<GroupReport> {
    let fixture = compiler_fixture()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .context("create compiler benchmark runtime")?;
    let mut startup = Vec::new();
    let mut first_check = Vec::new();
    let mut startup_to_check = Vec::new();
    for iteration in 0..(args.warmups + args.samples) {
        let total_started = Instant::now();
        let startup_started = Instant::now();
        let service = spawn_fixture_service(repository, &fixture)?;
        let startup_elapsed = startup_started.elapsed();
        let handle = service.handle();
        let check_started = Instant::now();
        let check = check_entry(&runtime, &handle, &fixture.entry)?;
        let check_elapsed = check_started.elapsed();
        ensure!(
            check.cache_stats.check_misses == 1,
            "fresh first check must miss the root-check cache"
        );
        let total_elapsed = total_started.elapsed();
        close_service(&runtime, service, handle);
        if iteration >= args.warmups {
            startup.push(startup_elapsed);
            first_check.push(check_elapsed);
            startup_to_check.push(total_elapsed);
        }
    }

    let service = spawn_fixture_service(repository, &fixture)?;
    let handle = service.handle();
    let baseline = check_entry(&runtime, &handle, &fixture.entry)?;
    ensure!(baseline.cache_stats.check_misses == 1, "baseline must miss");

    let unchanged = collect_metric(
        "unchanged_check",
        "Repeated artifact-free check returning detached semantic facts from the root cache.",
        args.warmups,
        args.samples,
        1,
        || {
            let check = check_entry(&runtime, &handle, &fixture.entry)?;
            ensure!(
                check.cache_stats.check_hits >= 1,
                "unchanged check must hit"
            );
            ensure!(
                check.cache_stats.check_misses == 0,
                "unchanged check must not remint"
            );
            Ok(())
        },
    )?;

    let mut leaf_version = 0_i64;
    let same_content = collect_metric(
        "same_content_update_check",
        "Overlay update with a new version but identical contents, followed by a check.",
        args.warmups,
        args.samples,
        1,
        || {
            leaf_version += 1;
            runtime.block_on(handle.update_document(DocumentUpdate {
                path: fixture.leaf.clone(),
                version: leaf_version,
                text: LEAF_A.to_string(),
            }))?;
            let check = check_entry(&runtime, &handle, &fixture.entry)?;
            ensure!(
                check.cache_stats.check_hits >= 1,
                "same-content check must hit"
            );
            ensure!(
                check.cache_stats.check_misses == 0,
                "same-content check reminted"
            );
            Ok(())
        },
    )?;

    let mut entry_version = 0_i64;
    let mut entry_variant = false;
    let root_edit = collect_metric(
        "root_edit_check",
        "Content-changing root overlay update plus artifact-free check with unchanged dependencies.",
        args.warmups,
        args.samples,
        1,
        || {
            entry_version += 1;
            entry_variant = !entry_variant;
            runtime.block_on(handle.update_document(DocumentUpdate {
                path: fixture.entry.clone(),
                version: entry_version,
                text: if entry_variant { ENTRY_B } else { ENTRY_A }.to_string(),
            }))?;
            let check = check_entry(&runtime, &handle, &fixture.entry)?;
            ensure!(check.cache_stats.check_misses == 1, "root edit must remint");
            ensure!(
                check.cache_stats.path_hits >= 1,
                "root edit must preserve a dependency cache hit"
            );
            Ok(())
        },
    )?;

    let mut leaf_variant = false;
    let dependency_edit = collect_metric(
        "dependency_edit_check",
        "Transitive dependency overlay update plus root check, preserving an unrelated dependency.",
        args.warmups,
        args.samples,
        1,
        || {
            leaf_version += 1;
            leaf_variant = !leaf_variant;
            runtime.block_on(handle.update_document(DocumentUpdate {
                path: fixture.leaf.clone(),
                version: leaf_version,
                text: if leaf_variant { LEAF_B } else { LEAF_A }.to_string(),
            }))?;
            let check = check_entry(&runtime, &handle, &fixture.entry)?;
            ensure!(
                check.cache_stats.check_misses == 1,
                "dependency edit must invalidate the root check"
            );
            ensure!(
                check.cache_stats.invalidated_checks >= 1,
                "dependency edit must report root-check invalidation"
            );
            ensure!(
                check.cache_stats.path_hits >= 1,
                "dependency edit must preserve the unrelated dependency"
            );
            Ok(())
        },
    )?;

    close_service(&runtime, service, handle);
    Ok(GroupReport {
        group: WorkerGroup::Compiler,
        metrics: vec![
            metric_from_durations(
                "compiler_epoch_startup",
                "Fresh compiler epoch construction with open editor overlays preinstalled.",
                args.warmups, 1, startup,
            ),
            metric_from_durations(
                "first_check", "First artifact-free root check after compiler epoch construction.",
                args.warmups, 1, first_check,
            ),
            metric_from_durations(
                "startup_to_first_check",
                "Compiler epoch construction through completion of its first check.", args.warmups,
                1, startup_to_check,
            ),
            unchanged,
            same_content,
            root_edit,
            dependency_edit,
        ],
        invariants: vec![
            "fresh first checks miss the detached root cache".to_string(),
            "unchanged and same-content checks hit without reminting".to_string(),
            "root edits remint while retaining dependency products".to_string(),
            "dependency edits invalidate transitive roots and preserve unrelated products"
                .to_string(),
        ],
        memory: Some(memory_report(None)),
    })
}

fn semantic_group(args: &Args) -> Result<GroupReport> {
    let path = PathBuf::from("/benchmark/generated-semantic.hoon");
    let source_a = generated_semantic_source(args.semantic_arms, 0);
    let source_b = generated_semantic_source(args.semantic_arms, 1);
    let hover_offset =
        u32::try_from(source_a.find("arm-a").context("generated hover target")? + 2)?;
    let completion_offset = u32::try_from(source_a.len().saturating_sub(3))?;
    let mut session = SemanticSession::default();
    let mut version = 0_i64;
    let mut variant = false;

    let changed = collect_metric(
        "semantic_changed_snapshot",
        "Whole-document parse and editor side-table rebuild for a changed generated source.",
        args.warmups,
        args.samples,
        1,
        || {
            version += 1;
            variant = !variant;
            let source = if variant { &source_b } else { &source_a };
            let count = session.snapshot(&path, version, source)?.symbols.len();
            ensure!(
                count == args.semantic_arms,
                "semantic snapshot lost declarations: {count} != {}",
                args.semantic_arms
            );
            Ok(())
        },
    )?;

    version += 1;
    let symbol_count = session.snapshot(&path, version, &source_a)?.symbols.len();
    ensure!(symbol_count == args.semantic_arms, "semantic seed mismatch");
    let cached = collect_metric(
        "semantic_cached_snapshot",
        "Path/version/content hit in the semantic snapshot cache.",
        args.warmups,
        args.samples,
        1,
        || {
            let count = session.snapshot(&path, version, &source_a)?.symbols.len();
            ensure!(
                count == args.semantic_arms,
                "cached semantic snapshot mismatch"
            );
            Ok(())
        },
    )?;

    let semantic_operations = 100;
    let hover = collect_metric(
        "semantic_cached_hover",
        "Cached structural hover lookup, reported per lookup.",
        args.warmups,
        args.samples,
        semantic_operations,
        || {
            let snapshot = session.snapshot(&path, version, &source_a)?;
            for _ in 0..semantic_operations {
                ensure!(
                    black_box(snapshot.hover(hover_offset)).is_some(),
                    "generated hover target disappeared"
                );
            }
            Ok(())
        },
    )?;
    let completion = collect_metric(
        "semantic_cached_completion",
        "Cached structural completion lookup over the generated document, reported per lookup.",
        args.warmups,
        args.samples,
        semantic_operations,
        || {
            let snapshot = session.snapshot(&path, version, &source_a)?;
            for _ in 0..semantic_operations {
                ensure!(
                    !black_box(snapshot.completions(completion_offset)).is_empty(),
                    "generated completion set disappeared"
                );
            }
            Ok(())
        },
    )?;

    Ok(GroupReport {
        group: WorkerGroup::Semantic,
        metrics: vec![changed, cached, hover, completion],
        invariants: vec![
            format!(
                "changed and cached snapshots retain all {} generated declarations",
                args.semantic_arms
            ),
            "cached hover and completion return nonempty results".to_string(),
        ],
        memory: Some(memory_report(None)),
    })
}

fn protocol_group(args: &Args, repository: &Path) -> Result<GroupReport> {
    let fixture = compiler_fixture()?;
    let mut startup_to_diagnostic = Vec::new();
    for iteration in 0..(args.warmups + args.samples) {
        let started = Instant::now();
        let (client, server_thread, _) =
            start_lsp_server(repository, &fixture.root, None, &fixture.root)?;
        let broken = fixture.root.join("broken.hoon");
        let broken_uri = file_uri(&broken)?;
        send_open(&client, &broken_uri, 1, "|=  [a=@\n")?;
        let published = receive_diagnostics(&client)?;
        ensure!(
            published.uri == broken_uri && !published.diagnostics.is_empty(),
            "startup diagnostic did not describe the opened malformed document"
        );
        let elapsed = started.elapsed();
        shutdown_lsp(client, server_thread, 10_000 + i32::try_from(iteration)?)?;
        if iteration >= args.warmups {
            startup_to_diagnostic.push(elapsed);
        }
    }

    let startup_metric = metric_from_durations(
        "lsp_startup_to_first_diagnostic",
        "In-memory LSP initialization through first unsaved parse diagnostic publication.",
        args.warmups, 1, startup_to_diagnostic,
    );
    if args.skip_contention {
        return Ok(GroupReport {
            group: WorkerGroup::Protocol,
            metrics: vec![startup_metric],
            invariants: vec![
                "malformed unsaved documents publish a versioned Honk diagnostic".to_string(),
                "startup timing uses the shipping protocol dispatch and worker queues".to_string(),
                "real Miner contention scenarios were explicitly skipped".to_string(),
            ],
            memory: Some(memory_report(None)),
        });
    }

    let contention_doc = fixture.root.join("protocol.hoon");
    let contention_uri = file_uri(&contention_doc)?;
    let miner = repository.join("hoon/apps/dumbnet/miner.hoon");
    ensure!(miner.is_file(), "Miner contention entry is missing");
    let (client, server_thread, initialize_elapsed) = start_lsp_server(
        repository,
        &repository.join("hoon"),
        Some(miner),
        repository,
    )?;
    send_open(&client, &contention_uri, 1, PROTOCOL_SOURCE_A)?;
    let mut request_id = 2_i32;

    let first_hover_started = Instant::now();
    let first_hover = request_hover(&client, request_id, &contention_uri)?;
    let first_hover_elapsed = first_hover_started.elapsed();
    ensure!(!first_hover.is_null(), "first protocol hover returned null");
    request_id += 1;

    let cached_hover = collect_metric(
        "lsp_cached_hover_with_background_miner_check",
        "LSP hover request/response while the configured Miner check is scheduled on its compiler worker.",
        args.warmups,
        args.samples,
        1,
        || {
            let result = request_hover(&client, request_id, &contention_uri)?;
            request_id += 1;
            ensure!(!result.is_null(), "cached protocol hover returned null");
            Ok(())
        },
    )?;

    let mut version = 1_i32;
    let mut variant = false;
    let changed_hover = collect_metric(
        "lsp_changed_hover_with_background_miner_check",
        "Full didChange plus hover response while the compiler worker handles a configured Miner check.",
        args.warmups,
        args.samples,
        1,
        || {
            version += 1;
            variant = !variant;
            send_change(
                &client,
                &contention_uri,
                version,
                if variant {
                    PROTOCOL_SOURCE_B
                } else {
                    PROTOCOL_SOURCE_A
                },
            )?;
            let result = request_hover(&client, request_id, &contention_uri)?;
            request_id += 1;
            ensure!(!result.is_null(), "changed protocol hover returned null");
            Ok(())
        },
    )?;

    let completion = collect_metric(
        "lsp_completion_with_background_miner_check",
        "LSP completion request/response while the configured Miner check is scheduled.",
        args.warmups,
        args.samples,
        1,
        || {
            let result = request_completion(&client, request_id, &contention_uri)?;
            request_id += 1;
            ensure!(!result.is_null(), "protocol completion returned null");
            Ok(())
        },
    )?;
    shutdown_lsp(client, server_thread, request_id)?;

    Ok(GroupReport {
        group: WorkerGroup::Protocol,
        metrics: vec![
            startup_metric,
            metric_from_durations(
                "lsp_initialize_workspace_scan",
                "One LSP initialize handshake including configured Hoon workspace discovery.",
                0,
                1,
                vec![initialize_elapsed],
            ),
            metric_from_durations(
                "lsp_first_hover_with_background_miner_check",
                "First structural hover request after scheduling the configured Miner check.",
                0,
                1,
                vec![first_hover_elapsed],
            ),
            cached_hover,
            changed_hover,
            completion,
        ],
        invariants: vec![
            "malformed unsaved documents publish a versioned Honk diagnostic".to_string(),
            "hover and completion remain responsive on the semantic worker while Miner is scheduled"
                .to_string(),
            "protocol timings use the shipping request dispatch and worker queues".to_string(),
        ],
        memory: Some(memory_report(None)),
    })
}

fn sustained_group(args: &Args, repository: &Path) -> Result<GroupReport> {
    let fixture = compiler_fixture()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .context("create sustained benchmark runtime")?;
    let service = spawn_fixture_service(repository, &fixture)?;
    let handle = service.handle();
    let baseline = check_entry(&runtime, &handle, &fixture.entry)?;
    ensure!(baseline.cache_stats.check_misses == 1, "baseline must miss");

    let mut version = 0_i64;
    let mut variant = false;
    for _ in 0..args.warmups {
        version += 1;
        variant = !variant;
        update_root_and_check(
            &runtime,
            &handle,
            &fixture.entry,
            version,
            if variant { ENTRY_B } else { ENTRY_A },
        )?;
    }
    let rss_before = current_rss_bytes();
    let sustained = collect_metric(
        "sustained_root_edit_check",
        "Invalidating root edit plus check across the configured sustained-check count.",
        0,
        args.sustained_checks,
        1,
        || {
            version += 1;
            variant = !variant;
            let check = update_root_and_check(
                &runtime,
                &handle,
                &fixture.entry,
                version,
                if variant { ENTRY_B } else { ENTRY_A },
            )?;
            ensure!(
                check.cache_stats.check_misses == 1,
                "sustained edit must remint"
            );
            ensure!(
                check.cache_stats.path_hits >= 1,
                "sustained root edit lost dependency reuse"
            );
            Ok(())
        },
    )?;
    let rss_after = current_rss_bytes();
    close_service(&runtime, service, handle);

    Ok(GroupReport {
        group: WorkerGroup::Sustained,
        metrics: vec![sustained],
        invariants: vec![
            format!(
                "all {} measured root edits reminted and retained dependency cache hits",
                args.sustained_checks
            ),
            "RSS is captured in an isolated worker before and after the measured edit sequence"
                .to_string(),
        ],
        memory: Some(memory_report(Some((rss_before, rss_after)))),
    })
}

fn collect_metric(
    name: &str,
    description: &str,
    warmups: usize,
    samples: usize,
    operations_per_sample: usize,
    mut operation: impl FnMut() -> Result<()>,
) -> Result<MetricReport> {
    let mut durations = Vec::with_capacity(samples);
    for iteration in 0..(warmups + samples) {
        let started = Instant::now();
        operation().with_context(|| format!("{name} iteration {iteration}"))?;
        let elapsed = started.elapsed();
        if iteration >= warmups {
            durations.push(elapsed);
        }
    }
    Ok(metric_from_durations(
        name, description, warmups, operations_per_sample, durations,
    ))
}

fn metric_from_durations(
    name: &str,
    description: &str,
    warmups: usize,
    operations_per_sample: usize,
    durations: Vec<Duration>,
) -> MetricReport {
    let samples = durations.len();
    let raw_ms_per_operation = durations
        .into_iter()
        .map(|duration| duration.as_secs_f64() * 1_000.0 / operations_per_sample as f64)
        .collect::<Vec<_>>();
    let mean_ms = raw_ms_per_operation.iter().sum::<f64>() / samples as f64;
    let variance = raw_ms_per_operation
        .iter()
        .map(|sample| (sample - mean_ms).powi(2))
        .sum::<f64>()
        / samples as f64;
    let stddev_ms = variance.sqrt();
    let mut sorted = raw_ms_per_operation.clone();
    sorted.sort_by(f64::total_cmp);
    let mut notes = Vec::new();
    if samples < 200 {
        notes.push("p95 is advisory: fewer than 200 measured samples".to_string());
    }
    if samples < 2_000 {
        notes.push("p99 is conservative: fewer than 2,000 measured samples".to_string());
    }
    let coefficient_of_variation = if mean_ms == 0.0 {
        0.0
    } else {
        stddev_ms / mean_ms
    };
    if coefficient_of_variation > 0.10 {
        notes.push(format!(
            "high variance: coefficient of variation is {:.1}%",
            coefficient_of_variation * 100.0
        ));
    }
    MetricReport {
        name: name.to_string(),
        description: description.to_string(),
        samples,
        warmups,
        operations_per_sample,
        raw_ms_per_operation,
        mean_ms,
        stddev_ms,
        coefficient_of_variation,
        p50_ms: percentile(&sorted, 0.50),
        p95_ms: percentile(&sorted, 0.95),
        p99_ms: percentile(&sorted, 0.99),
        max_ms: sorted.last().copied().unwrap_or_default(),
        operations_per_second: if mean_ms == 0.0 {
            f64::INFINITY
        } else {
            1_000.0 / mean_ms
        },
        notes,
    }
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (quantile * sorted.len() as f64).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

fn compiler_fixture() -> Result<CompilerFixture> {
    let temp = TempDir::new().context("create compiler fixture")?;
    let root = temp.path().to_path_buf();
    let lib = root.join("lib");
    fs::create_dir_all(&lib)?;
    let entry = root.join("entry.hoon");
    let leaf = lib.join("leaf.hoon");
    fs::write(&entry, ENTRY_A)?;
    fs::write(&leaf, LEAF_A)?;
    fs::write(lib.join("helper.hoon"), HELPER)?;
    fs::write(lib.join("stable.hoon"), STABLE)?;
    Ok(CompilerFixture {
        _temp: temp,
        root,
        entry,
        leaf,
    })
}

fn spawn_fixture_service(repository: &Path, fixture: &CompilerFixture) -> Result<CompilerService> {
    CompilerService::spawn_with_documents(
        CompilerServiceConfig {
            workspace: WorkspaceConfig {
                prelude: repository.join("hoon/common/hoon.hoon"),
                dependencies: fixture.root.clone(),
                subject_type_jam: None,
                dbug: true,
                vet: true,
            },
            max_compiles: 0,
            worker_stack_bytes: DEFAULT_WORKER_STACK_BYTES,
        },
        [
            DocumentUpdate {
                path: fixture.entry.clone(),
                version: 0,
                text: ENTRY_A.to_string(),
            },
            DocumentUpdate {
                path: fixture.leaf.clone(),
                version: 0,
                text: LEAF_A.to_string(),
            },
        ],
    )
}

fn check_entry(
    runtime: &tokio::runtime::Runtime,
    handle: &CompilerHandle,
    entry: &Path,
) -> Result<honk::workspace::WorkspaceCheckOutput> {
    runtime
        .block_on(handle.check(WorkspaceCheckRequest {
            entry: entry.to_path_buf(),
        }))?
        .result
        .map_err(|error| anyhow!(error.diagnostic.message))
}

fn update_root_and_check(
    runtime: &tokio::runtime::Runtime,
    handle: &CompilerHandle,
    entry: &Path,
    version: i64,
    source: &str,
) -> Result<honk::workspace::WorkspaceCheckOutput> {
    runtime.block_on(handle.update_document(DocumentUpdate {
        path: entry.to_path_buf(),
        version,
        text: source.to_string(),
    }))?;
    check_entry(runtime, handle, entry)
}

fn close_service(
    runtime: &tokio::runtime::Runtime,
    service: CompilerService,
    handle: CompilerHandle,
) {
    let (owned_handle, exhausted) = service.into_parts();
    drop(handle);
    drop(owned_handle);
    let _ = runtime.block_on(exhausted);
}

fn generated_semantic_source(arms: usize, delta: usize) -> String {
    let mut source = String::from("|%\n");
    for index in 0..arms {
        let _ = writeln!(source, "++  arm-{}", alpha_index(index));
        let _ = writeln!(
            source,
            "  {}",
            index + usize::from(index + 1 == arms) * delta
        );
    }
    source.push_str("--\n");
    source
}

fn alpha_index(mut index: usize) -> String {
    let mut reversed = Vec::new();
    loop {
        reversed.push((b'a' + (index % 26) as u8) as char);
        index /= 26;
        if index == 0 {
            break;
        }
        index -= 1;
    }
    reversed.into_iter().rev().collect()
}

fn start_lsp_server(
    repository: &Path,
    dependencies: &Path,
    entry: Option<PathBuf>,
    root_uri_path: &Path,
) -> Result<(
    Connection,
    std::thread::JoinHandle<anyhow::Result<()>>,
    Duration,
)> {
    let (server, client) = Connection::memory();
    let config = LspConfig {
        prelude: Some(repository.join("hoon/common/hoon.hoon")),
        dependencies: Some(dependencies.to_path_buf()),
        entry,
        subject_type_jam: None,
        dbug: true,
        vet: true,
        max_compiles: 0,
        worker_stack_bytes: DEFAULT_WORKER_STACK_BYTES,
        check_delay_ms: 0,
    };
    let server_thread = std::thread::spawn(move || run_connection(server, config));
    let root_url = url::Url::from_directory_path(root_uri_path)
        .map_err(|()| anyhow!("invalid benchmark root URI"))?;
    let started = Instant::now();
    client.sender.send(
        Request::new(
            RequestId::from(1),
            "initialize".to_string(),
            json!({
                "processId": null,
                "rootUri": root_url,
                "capabilities": {},
                "workspaceFolders": [{ "uri": root_url, "name": "honk-lsp-performance" }]
            }),
        )
        .into(),
    )?;
    let initialized = receive_response(&client, 1)?;
    ensure!(
        initialized["capabilities"]["hoverProvider"] == json!(true),
        "benchmark server did not advertise hover"
    );
    let elapsed = started.elapsed();
    client
        .sender
        .send(Notification::new("initialized".to_string(), json!({})).into())?;
    Ok((client, server_thread, elapsed))
}

fn send_open(client: &Connection, uri: &Uri, version: i32, source: &str) -> Result<()> {
    client.sender.send(
        Notification::new(
            DidOpenTextDocument::METHOD.to_string(),
            serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem::new(
                    uri.clone(),
                    "hoon".to_string(),
                    version,
                    source.to_string(),
                ),
            })?,
        )
        .into(),
    )?;
    Ok(())
}

fn send_change(client: &Connection, uri: &Uri, version: i32, source: &str) -> Result<()> {
    client.sender.send(
        Notification::new(
            DidChangeTextDocument::METHOD.to_string(),
            serde_json::to_value(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier::new(uri.clone(), version),
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: source.to_string(),
                }],
            })?,
        )
        .into(),
    )?;
    Ok(())
}

fn request_hover(client: &Connection, id: i32, uri: &Uri) -> Result<Value> {
    client.sender.send(
        Request::new(
            RequestId::from(id),
            "textDocument/hover".to_string(),
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 5 }
            }),
        )
        .into(),
    )?;
    receive_response(client, id)
}

fn request_completion(client: &Connection, id: i32, uri: &Uri) -> Result<Value> {
    client.sender.send(
        Request::new(
            RequestId::from(id),
            "textDocument/completion".to_string(),
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 4, "character": 8 }
            }),
        )
        .into(),
    )?;
    receive_response(client, id)
}

fn receive_response(client: &Connection, expected: i32) -> Result<Value> {
    loop {
        let message = client
            .receiver
            .recv_timeout(Duration::from_secs(120))
            .context("wait for LSP response")?;
        let Message::Response(response) = message else {
            continue;
        };
        if response.id != RequestId::from(expected) {
            continue;
        }
        return match response.response_kind {
            ResponseKind::Ok { result } => Ok(result),
            ResponseKind::Err { error } => {
                bail!("LSP request {expected} failed: {error:?}")
            }
        };
    }
}

fn receive_diagnostics(client: &Connection) -> Result<PublishDiagnosticsParams> {
    loop {
        let message = client
            .receiver
            .recv_timeout(Duration::from_secs(120))
            .context("wait for LSP diagnostics")?;
        let Message::Notification(notification) = message else {
            continue;
        };
        if notification.method == PublishDiagnostics::METHOD {
            return serde_json::from_value(notification.params)
                .context("decode publishDiagnostics parameters");
        }
    }
}

fn shutdown_lsp(
    client: Connection,
    server_thread: std::thread::JoinHandle<anyhow::Result<()>>,
    request_id: i32,
) -> Result<()> {
    client.sender.send(
        Request::new(
            RequestId::from(request_id),
            "shutdown".to_string(),
            json!(null),
        )
        .into(),
    )?;
    client
        .sender
        .send(Notification::new("exit".to_string(), json!(null)).into())?;
    let _ = receive_response(&client, request_id)?;
    server_thread
        .join()
        .map_err(|_| anyhow!("LSP benchmark server panicked"))??;
    Ok(())
}

fn file_uri(path: &Path) -> Result<Uri> {
    let url = url::Url::from_file_path(path).map_err(|()| anyhow!("invalid file URI path"))?;
    Uri::from_str(url.as_str()).context("parse benchmark LSP URI")
}

fn memory_report(rss: Option<(Option<u64>, Option<u64>)>) -> MemoryReport {
    let (before, after) = rss.unwrap_or((None, current_rss_bytes()));
    let delta = before.zip(after).map(|(before, after)| {
        i64::try_from(after).unwrap_or(i64::MAX) - i64::try_from(before).unwrap_or(i64::MAX)
    });
    MemoryReport {
        current_rss_before_bytes: before,
        current_rss_after_bytes: after,
        current_rss_delta_bytes: delta,
        process_peak_rss_bytes: peak_rss_bytes(),
    }
}

fn current_rss_bytes() -> Option<u64> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(|kibibytes| kibibytes * 1_024)
}

#[cfg(unix)]
fn peak_rss_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the pointed-to rusage on a successful call.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: the successful getrusage call above initialized usage.
    let usage = unsafe { usage.assume_init() };
    let raw = u64::try_from(usage.ru_maxrss).ok()?;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        Some(raw)
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        Some(raw * 1_024)
    }
}

#[cfg(not(unix))]
fn peak_rss_bytes() -> Option<u64> {
    None
}

fn fingerprint(repository: &Path, args: &Args, run_id: &str) -> Value {
    let git_sha = command_output(repository, "git", &["rev-parse", "HEAD"]);
    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repository)
        .output()
        .ok()
        .is_some_and(|output| !output.stdout.is_empty());
    let cpu_model = if cfg!(target_os = "macos") {
        command_output(repository, "sysctl", &["-n", "machdep.cpu.brand_string"])
    } else {
        fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|contents| {
                contents.lines().find_map(|line| {
                    line.strip_prefix("model name\t:")
                        .map(|value| value.trim().to_string())
                })
            })
    };
    let memory = if cfg!(target_os = "macos") {
        command_output(repository, "sysctl", &["-n", "hw.memsize"])
    } else {
        fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|contents| contents.lines().next().map(str::to_string))
    };
    json!({
        "run_id": run_id,
        "captured_at_unix_seconds": SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs()),
        "git_sha": git_sha,
        "git_dirty": dirty,
        "crate_version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "cpu_model": cpu_model,
        "logical_cpus": std::thread::available_parallelism().ok().map(|count| count.get()),
        "memory": memory,
        "kernel": command_output(repository, "uname", &["-a"]),
        "filesystem": command_output(repository, "df", &["-P", repository.to_string_lossy().as_ref()]),
        "rustc": command_output(repository, "rustc", &["--version", "--verbose"]),
        "cargo": command_output(repository, "cargo", &["--version"]),
        "build_profile": "cargo bench (optimized bench profile; repository release debug info retained)",
        "transport": "lsp-server Connection::memory for protocol scenarios",
        "cache_state": "warm operating-system cache; fresh compiler epochs where named",
        "samples": args.samples,
        "warmups": args.warmups,
        "sustained_checks": args.sustained_checks,
        "semantic_arms": args.semantic_arms,
        "contention_included": !args.skip_contention,
        "os_tuning_applied": false
    })
}

fn command_output(repository: &Path, program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(arguments)
        .current_dir(repository)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn render_define(args: &Args) -> String {
    format!(
        "# DEFINE — Honk LSP editor baseline\n\n\
         ## Scenario\n\n\
         Measure fresh and warm artifact-free compiler checks, source-overlay invalidation, \
         semantic snapshots, LSP hover/completion responsiveness under a scheduled Miner check, \
         and RSS across {} invalidating root edits.\n\n\
         ## Metric\n\n\
         Wall-time p50/p95/p99/max, operations per second, coefficient of variation, and current/peak RSS.\n\n\
         ## Budget\n\n\
         This run establishes a host-specific baseline. Future same-host comparisons treat p95 drift \
         up to 10% as noise, above 10% as investigate, and above 20% (or three consecutive >10% runs) \
         as escalation. No cross-host absolute latency gate is claimed.\n\n\
         ## Golden output\n\n\
         Every timed operation asserts cache hit/miss/invalidation behavior and successful semantic or \
         protocol output. The harness aborts instead of publishing timings when an invariant fails.\n\n\
         ## Scope boundary\n\n\
         Protocol scenarios use in-memory LSP transport. OS process launch, stdio framing, cold filesystem \
         cache, CPU pinning, and kernel tuning are out of scope. Samples: {}; warmups: {}; semantic arms: {}.\n",
        args.sustained_checks, args.samples, args.warmups, args.semantic_arms
    )
}

fn render_baseline(report: &FullReport) -> String {
    let mut markdown = format!(
        "# Honk LSP baseline — {}\n\nAll latency values are milliseconds per operation.\n\n",
        report.run_id
    );
    for group in &report.groups {
        let _ = writeln!(markdown, "## {}\n", group.group.cli_name());
        markdown.push_str("| Scenario | N | p50 | p95 | p99 | max | CV |\n");
        markdown.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
        for metric in &group.metrics {
            let _ = writeln!(
                markdown,
                "| {} | {} | {:.3} | {:.3} | {:.3} | {:.3} | {:.1}% |",
                metric.name,
                metric.samples,
                metric.p50_ms,
                metric.p95_ms,
                metric.p99_ms,
                metric.max_ms,
                metric.coefficient_of_variation * 100.0
            );
        }
        if let Some(memory) = &group.memory {
            let _ = writeln!(
                markdown,
                "\nMemory: current before {}, current after {}, delta {}, process peak {}.\n",
                display_bytes(memory.current_rss_before_bytes),
                display_bytes(memory.current_rss_after_bytes),
                memory
                    .current_rss_delta_bytes
                    .map(|bytes| format!("{bytes} bytes"))
                    .unwrap_or_else(|| "unavailable".to_string()),
                display_bytes(memory.process_peak_rss_bytes)
            );
        }
    }
    markdown.push_str(
        "Tail warning: p95 is advisory below 200 samples and p99 is conservative below 2,000 samples.\n",
    );
    markdown
}

fn display_bytes(bytes: Option<u64>) -> String {
    bytes
        .map(|bytes| format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0)))
        .unwrap_or_else(|| "unavailable".to_string())
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("honk-lsp crate must be below the repository root")
        .to_path_buf()
}
