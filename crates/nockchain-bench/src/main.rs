//! Nockchain Bench CLI
//!
//! Benchmarking and memory profiling tool for Nockchain.
//!
//! Usage:
//!   nockchain-bench sol extract [OPTIONS]       # Extract blocks to archive
//!   nockchain-bench sol inspect [OPTIONS]       # Inspect mempool snapshots

mod commands;

use std::path::PathBuf;
use std::process::Command as ProcessCommand;

use clap::{Parser, Subcommand, ValueEnum};
use nockchain_bench::speed_of_light::harness::profiler::ensure_samply_profiled_binary;
use nockchain_bench::speed_of_light::{ColdMode, CpuProfilerKind, PeekMode};

const SOL_AFTER_HELP: &str = "Command roles:\n  quick-bench: ad hoc single-run debugging only; not reproducible evidence\n  quick-read-bench: ad hoc checkpoint- or snapshot-backed read benchmarking only\n  bench: trusted measured runs with persisted artifacts and verdicts\n  validate: Docker preflight without replay\n  sweep: trusted matrix orchestration over bench\n\n`--blocks N` always means prefix replay of the fixture archive window, not an arbitrary slice.\nSee crates/nockchain-bench/README.md for the full trusted benchmark protocol.";

const QUICK_BENCH_AFTER_HELP: &str = "Use this for inner-loop investigation only.\nIt does not run the trusted orchestration and should not be used as published benchmark evidence.\n\n`--blocks N` replays the first N accepted blocks from the fixture archive window.\n`--cpu-profiler samply --cpu-profile-output <path>` relaunches the quick-bench session under a bytehound-built `nockchain-bench`, then writes one extra raw profiled replay pass to the requested path.\nOn Linux, CPU profiling requires `kernel.perf_event_paranoid <= 1`.\nFor trusted measurement, use `nockchain-bench sol bench`.\nSee crates/nockchain-bench/README.md.";

const QUICK_READ_BENCH_AFTER_HELP: &str = "Use this for inner-loop checkpoint- or snapshot-backed read investigation only.\nIt issues sequential `%heavy-n` peeks over a resolved height range and does not run the trusted orchestration.\n\n`--count N` peeks a positive number of heights starting at `--start-height`.\n`--end-height N` resolves an inclusive range ending at the requested height.\nFor trusted measurement, use the later read-harness flow rather than this quick command.\nSee crates/nockchain-bench/README.md.";
const QUICK_ORCHESTRATE_AFTER_HELP: &str = "Use this for quick shared-runtime orchestration only.\nIt boots one checkpoint- or snapshot-backed runtime, executes ordered poke/peek steps from a JSON plan, and is not trusted benchmark evidence.\n\nThe plan file owns boot inputs and step order.\nFor trusted measurement, use `nockchain-bench sol bench`.\nSee crates/nockchain-bench/README.md.";

const BENCH_AFTER_HELP: &str = "Trusted protocol:\n- use a release binary unless you intentionally pass --allow-debug-benchmark\n- point --output at an existing empty directory\n- `--blocks N` replays a prefix of the fixture archive window\n- Docker mode records host/container binary identity and rejects version or commit skew unless --allow-version-skew is set\n- use `sol validate` to inspect Docker resource realization without replay\n- direct `sol bench` stays on trusted warmup/measured runs only; CPU profiling is exposed via `sol quick-bench` and `sol sweep`\n\nSee crates/nockchain-bench/README.md for the full protocol and artifact model.";

const VALIDATE_AFTER_HELP: &str = "Preflight Docker trusted execution without replay.\nThis records the same resource-realization facts and environment evidence that trusted Docker `sol bench` uses when deciding whether a run is valid.\n\nSee crates/nockchain-bench/README.md for the version policy and artifact model.";

const SWEEP_AFTER_HELP: &str = "Each expanded case runs through the trusted `sol bench` orchestrator.\nAll non-axis fields must remain constant across a trusted comparison.\n\n`--blocks N` keeps prefix-replay semantics for every case in the sweep.\nTrusted CPU profiling is not supported for the first release.\nSee crates/nockchain-bench/README.md for the comparison protocol.";

#[derive(Parser)]
#[command(name = "nockchain-bench")]
#[command(about = "Benchmarking and memory profiling tool for Nockchain")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Debug, ValueEnum, PartialEq, Eq)]
enum BenchWorkDirMode {
    HostBind,
    DockerVolume,
    DockerTmpfs,
}

#[derive(Clone, Debug, ValueEnum, PartialEq, Eq)]
enum BenchFsyncMode {
    On,
    Off,
}

impl BenchFsyncMode {
    fn enabled(self) -> bool {
        matches!(self, Self::On)
    }
}

#[derive(Clone, Debug, ValueEnum, PartialEq, Eq)]
enum QuickOrchestrateColdMode {
    Strict,
    Soft,
}

impl From<QuickOrchestrateColdMode> for ColdMode {
    fn from(value: QuickOrchestrateColdMode) -> Self {
        match value {
            QuickOrchestrateColdMode::Strict => Self::Strict,
            QuickOrchestrateColdMode::Soft => Self::Soft,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
enum BenchPeekMode {
    Warm,
    ColdEach,
}

impl From<BenchPeekMode> for PeekMode {
    fn from(value: BenchPeekMode) -> Self {
        match value {
            BenchPeekMode::Warm => Self::Warm,
            BenchPeekMode::ColdEach => Self::ColdEach,
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Speed-of-light benchmark commands
    #[command(
        subcommand,
        after_help = SOL_AFTER_HELP
    )]
    Sol(SolCommands),
}

#[derive(Subcommand)]
enum SolCommands {
    /// Extract blocks from a checkpoint or snapshot to an archive file
    Extract {
        /// Number of blocks to extract
        #[arg(short = 'n', long, default_value = "1000")]
        blocks: u64,

        /// Start block height (inclusive)
        #[arg(long, default_value = "0")]
        start_height: u64,

        /// End block height (inclusive). If set, overrides --blocks.
        #[arg(long)]
        end_height: Option<u64>,

        /// Path to checkpoint file
        #[arg(
            short,
            long,
            required_unless_present = "snapshot_pma",
            conflicts_with_all = ["snapshot_pma", "snapshot_manifest"]
        )]
        checkpoint: Option<PathBuf>,

        /// Path to snapshot PMA file
        #[arg(long, requires = "snapshot_manifest", conflicts_with = "checkpoint")]
        snapshot_pma: Option<PathBuf>,

        /// Path to snapshot manifest file
        #[arg(long, requires = "snapshot_pma", conflicts_with = "checkpoint")]
        snapshot_manifest: Option<PathBuf>,

        /// Path to kernel jam file
        #[arg(short, long, default_value = "assets/dumb.jam")]
        kernel: PathBuf,

        /// Output archive path (defaults to blocks_<N>.solarch)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Include mempool snapshots in the archive
        #[arg(long)]
        include_mempool: bool,
    },

    /// Run a quick inner-loop benchmark from a unified fixture (`.soltest`); NOT reproducible data
    #[command(name = "quick-bench", after_help = QUICK_BENCH_AFTER_HELP)]
    QuickBench {
        /// Path to a unified `.soltest` fixture file (includes checkpoint + archive + kernel)
        #[arg(short, long)]
        fixture: PathBuf,

        /// Number of blocks to benchmark (0 = all in archive)
        #[arg(short = 'n', long, default_value = "0")]
        blocks: u64,

        /// Skip genesis block (block 0) - not recommended
        #[arg(long)]
        skip_genesis: bool,

        #[arg(long, value_enum, default_value = "on")]
        fsync: BenchFsyncMode,

        /// Enable process memory timeline profiling during benchmark replay
        #[arg(long)]
        profile_memory: bool,

        /// Memory profile sample interval in milliseconds
        #[arg(long, default_value = "500")]
        profile_interval_ms: u64,

        /// Write benchmark + memory profile JSON to this path
        #[arg(long)]
        profile_output: Option<PathBuf>,

        /// Optional CPU profiler for an extra profiling replay pass
        #[arg(long, value_enum, requires = "cpu_profile_output")]
        cpu_profiler: Option<CpuProfilerKind>,

        /// CPU profiling sample rate in Hz
        #[arg(
            long,
            default_value_t = 1000,
            requires = "cpu_profiler",
            value_parser = clap::value_parser!(u32).range(1..)
        )]
        cpu_profile_rate: u32,

        /// Write the raw CPU profile artifact to this path
        #[arg(long, requires = "cpu_profiler")]
        cpu_profile_output: Option<PathBuf>,

        /// Inferred GC threshold in MiB (RSS drop >= threshold)
        #[arg(long, default_value = "64")]
        gc_drop_threshold_mib: u64,

        /// Minor page-fault delta threshold for burst detection
        #[arg(long, default_value = "50000")]
        page_fault_minor_burst_threshold: u64,

        /// Major page-fault delta threshold for burst detection
        #[arg(long, default_value = "1")]
        page_fault_major_burst_threshold: u64,
    },

    /// Run a quick checkpoint- or snapshot-backed read benchmark; NOT reproducible data
    #[command(name = "quick-read-bench", after_help = QUICK_READ_BENCH_AFTER_HELP)]
    QuickReadBench {
        /// Path to checkpoint file
        #[arg(
            long,
            required_unless_present = "snapshot_pma",
            conflicts_with_all = ["snapshot_pma", "snapshot_manifest"]
        )]
        checkpoint: Option<PathBuf>,

        /// Path to snapshot PMA file
        #[arg(long, requires = "snapshot_manifest", conflicts_with = "checkpoint")]
        snapshot_pma: Option<PathBuf>,

        /// Path to snapshot manifest file
        #[arg(long, requires = "snapshot_pma", conflicts_with = "checkpoint")]
        snapshot_manifest: Option<PathBuf>,

        /// Path to kernel jam file
        #[arg(long, default_value = "assets/dumb.jam")]
        kernel: PathBuf,

        /// Start height for the read range
        #[arg(long, default_value = "0")]
        start_height: u64,

        /// End height for the read range (inclusive); mutually exclusive with --count.
        #[arg(long, conflicts_with = "count")]
        end_height: Option<u64>,

        /// Number of heights to peek starting at --start-height (1..)
        #[arg(
            long,
            conflicts_with = "end_height",
            value_parser = clap::value_parser!(u64).range(1..)
        )]
        count: Option<u64>,

        /// Enable or disable PMA replay fsync behavior
        #[arg(long, value_enum, default_value = "on")]
        fsync: BenchFsyncMode,

        /// Exit after setup without issuing peeks
        #[arg(long)]
        dry_run: bool,

        /// Enable process memory timeline profiling during the read benchmark
        #[arg(long)]
        profile_memory: bool,

        /// Memory profile sample interval in milliseconds
        #[arg(long, default_value = "500")]
        profile_interval_ms: u64,

        /// Write benchmark + memory profile JSON to this path
        #[arg(long)]
        profile_output: Option<PathBuf>,

        /// Optional CPU profiler for an extra profiling rerun
        #[arg(long, value_enum, requires = "cpu_profile_output")]
        cpu_profiler: Option<CpuProfilerKind>,

        /// CPU profiling sample rate in Hz
        #[arg(
            long,
            default_value_t = 1000,
            requires = "cpu_profiler",
            value_parser = clap::value_parser!(u32).range(1..)
        )]
        cpu_profile_rate: u32,

        /// Write the raw CPU profile artifact to this path
        #[arg(long, requires = "cpu_profiler")]
        cpu_profile_output: Option<PathBuf>,
    },

    /// Run a quick shared-runtime orchestration plan; NOT reproducible data
    #[command(name = "quick-orchestrate", after_help = QUICK_ORCHESTRATE_AFTER_HELP)]
    QuickOrchestrate {
        /// Path to the quick-orchestrate JSON plan file
        #[arg(long)]
        plan: PathBuf,

        /// Write compact orchestrate JSON to this path
        #[arg(long)]
        profile_output: Option<PathBuf>,

        /// Control how future cold-step verification failures are handled
        #[arg(long, value_enum, default_value = "strict")]
        cold_mode: QuickOrchestrateColdMode,

        /// Enable or disable PMA replay fsync behavior
        #[arg(long, value_enum, default_value = "on")]
        fsync: BenchFsyncMode,
    },

    /// Run a trusted SOL benchmark and emit machine-readable artifacts
    #[command(after_help = BENCH_AFTER_HELP)]
    Bench {
        /// Trusted benchmark kind. Only `sol-orchestrate` is accepted.
        #[arg(long, default_value = "sol-orchestrate")]
        benchmark: String,

        /// Path to a trusted orchestration plan JSON
        #[arg(long, conflicts_with_all = ["fixture", "checkpoint", "snapshot_pma", "snapshot_manifest"])]
        plan: Option<PathBuf>,

        /// Path to a unified `.soltest` fixture file (includes checkpoint + archive + kernel)
        #[arg(short, long, conflicts_with_all = ["plan", "checkpoint", "snapshot_pma", "snapshot_manifest"])]
        fixture: Option<PathBuf>,

        /// Path to checkpoint file for trusted read shorthand
        #[arg(long, conflicts_with_all = ["plan", "fixture", "snapshot_pma", "snapshot_manifest"])]
        checkpoint: Option<PathBuf>,

        /// Path to snapshot PMA file for trusted read shorthand
        #[arg(long, requires = "snapshot_manifest", conflicts_with_all = ["plan", "fixture", "checkpoint"])]
        snapshot_pma: Option<PathBuf>,

        /// Path to snapshot manifest file for trusted read shorthand
        #[arg(long, requires = "snapshot_pma", conflicts_with_all = ["plan", "fixture", "checkpoint"])]
        snapshot_manifest: Option<PathBuf>,

        /// Path to kernel jam file for trusted read shorthand
        #[arg(long, default_value = "assets/dumb.jam")]
        kernel: PathBuf,

        /// Start height for trusted read shorthand
        #[arg(long)]
        start_height: Option<u64>,

        /// End height for trusted read shorthand; mutually exclusive with --count.
        #[arg(long, conflicts_with = "count")]
        end_height: Option<u64>,

        /// Number of heights to peek for trusted read shorthand
        #[arg(
            long,
            conflicts_with = "end_height",
            value_parser = clap::value_parser!(u64).range(1..)
        )]
        count: Option<u64>,

        /// Read mode for trusted read shorthand
        #[arg(long, value_enum, default_value = "warm")]
        peek_mode: BenchPeekMode,

        /// Output root directory for trusted run artifacts
        #[arg(short, long)]
        output: PathBuf,

        /// Number of blocks to benchmark (0 = all in archive)
        #[arg(short = 'n', long, default_value = "0")]
        blocks: u64,

        /// Skip genesis block (block 0) - not recommended
        #[arg(long)]
        skip_genesis: bool,

        /// Enable process memory timeline profiling during benchmark replay
        #[arg(long)]
        profile_memory: bool,

        /// Memory profile sample interval in milliseconds
        #[arg(long, default_value = "500")]
        profile_interval_ms: u64,

        /// Logical thread count metadata for this requested case
        #[arg(long, default_value = "1")]
        threads: u32,

        /// Warmup repetitions to persist but exclude from summary statistics
        #[arg(long, default_value = "1")]
        warmup_runs: u32,

        /// Measured repetitions to include in summary statistics
        #[arg(long, default_value = "5")]
        measured_runs: u32,

        /// Cooldown between measured repetitions in seconds
        #[arg(long, default_value = "10")]
        cooldown_secs: u64,

        /// Maximum accepted primary throughput coefficient of variation before Partial verdict
        #[arg(long, value_parser = clap::value_parser!(f64))]
        cv_threshold: Option<f64>,

        /// Optional human label for the requested case
        #[arg(long)]
        label: Option<String>,

        /// Run the trusted benchmark inside this provided Docker image instead of natively
        #[arg(long, conflicts_with = "docker_build_tag")]
        docker_image: Option<String>,

        /// Auto-build and run the trusted benchmark from this local Docker tag
        #[arg(long, conflicts_with = "docker_image")]
        docker_build_tag: Option<String>,

        /// Docker memory limit for trusted container execution (for example `16g`)
        #[arg(long)]
        memory_limit: Option<String>,

        /// Explicit Docker work directory mode for trusted container execution
        #[arg(long, value_enum)]
        work_dir_mode: Option<BenchWorkDirMode>,

        /// Optional Docker CPU set (for example `0-3`)
        #[arg(long)]
        cpuset: Option<String>,

        /// Optional Docker CPU quota
        #[arg(long)]
        cpu_quota: Option<i64>,

        /// Optional Docker CPU period
        #[arg(long)]
        cpu_period: Option<i64>,

        /// Allow trusted Docker runs when host/container versions differ
        #[arg(long)]
        allow_version_skew: bool,

        /// Allow enumerated degraded cold evidence and mark the verdict Partial
        #[arg(long)]
        allow_degraded_cold: bool,

        /// Allow trusted artifacts from a non-release build
        #[arg(long)]
        allow_debug_benchmark: bool,
    },

    /// Validate a trusted Docker SOL benchmark environment without running replay
    #[command(after_help = VALIDATE_AFTER_HELP)]
    Validate {
        /// Path to a unified `.soltest` fixture file (includes checkpoint + archive + kernel)
        #[arg(short, long)]
        fixture: PathBuf,

        /// Output root directory for validation artifacts
        #[arg(short, long)]
        output: PathBuf,

        /// Docker image containing the trusted benchmark binary
        #[arg(long, conflicts_with = "docker_build_tag")]
        docker_image: Option<String>,

        /// Auto-build a local Docker image containing the trusted benchmark binary
        #[arg(long, conflicts_with = "docker_image")]
        docker_build_tag: Option<String>,

        /// Docker memory limit for trusted container execution (for example `16g`)
        #[arg(long)]
        memory_limit: String,

        /// Explicit Docker work directory mode for trusted container execution
        #[arg(long, value_enum)]
        work_dir_mode: BenchWorkDirMode,

        /// Optional Docker CPU set (for example `0-3`)
        #[arg(long)]
        cpuset: Option<String>,

        /// Optional Docker CPU quota
        #[arg(long)]
        cpu_quota: Option<i64>,

        /// Optional Docker CPU period
        #[arg(long)]
        cpu_period: Option<i64>,
    },

    /// Run a trusted SOL sweep over a matrix of benchmark cases
    #[command(after_help = SWEEP_AFTER_HELP)]
    Sweep {
        /// Path to the sweep matrix JSON file
        #[arg(long)]
        matrix: PathBuf,

        /// Output root directory for sweep artifacts
        #[arg(short, long)]
        output: PathBuf,

        /// Interleave case execution across axis values
        #[arg(long, conflicts_with = "randomize_order")]
        interleave: bool,

        /// Randomize trusted case execution order
        #[arg(long, conflicts_with = "interleave")]
        randomize_order: bool,

        /// Emit optional `comparison.md` alongside `comparison.json`
        #[arg(long)]
        comparison_markdown: bool,
    },

    /// Hidden machine-oriented wrapper for one shared once-run execution
    #[command(hide = true, name = "run-once")]
    RunOnce {
        /// Path to a resolved-case JSON payload
        #[arg(long)]
        resolved_case: PathBuf,

        /// Output directory for this run's artifacts
        #[arg(long)]
        run_dir: PathBuf,

        /// PMA/runtime work directory for this run
        #[arg(long)]
        work_dir: Option<PathBuf>,

        /// Optional explicit run id (defaults to the run_dir basename)
        #[arg(long)]
        run_id: Option<String>,
    },

    /// Hidden machine-oriented wrapper for one resolved read-benchmark rerun
    #[command(hide = true, name = "quick-read-once")]
    QuickReadOnce {
        /// Path to checkpoint file
        #[arg(
            long,
            required_unless_present = "snapshot_pma",
            conflicts_with_all = ["snapshot_pma", "snapshot_manifest"]
        )]
        checkpoint: Option<PathBuf>,

        /// Path to snapshot PMA file
        #[arg(long, requires = "snapshot_manifest", conflicts_with = "checkpoint")]
        snapshot_pma: Option<PathBuf>,

        /// Path to snapshot manifest file
        #[arg(long, requires = "snapshot_pma", conflicts_with = "checkpoint")]
        snapshot_manifest: Option<PathBuf>,

        /// Path to kernel jam file
        #[arg(long)]
        kernel: PathBuf,

        /// Start height for the resolved read range
        #[arg(long)]
        start_height: u64,

        /// End height for the resolved read range (inclusive)
        #[arg(long)]
        end_height: u64,

        /// Enable or disable PMA replay fsync behavior
        #[arg(long, value_enum, default_value = "on")]
        fsync: BenchFsyncMode,

        /// Exit after setup without issuing peeks
        #[arg(long)]
        dry_run: bool,
    },

    /// Hidden machine-oriented binary identity output
    #[command(hide = true, name = "binary-identity")]
    BinaryIdentity,

    /// Hidden machine-oriented Docker validation probe
    #[command(hide = true, name = "validate-probe")]
    ValidateProbe,

    /// Inspect mempool snapshots for stale transactions
    Inspect {
        /// Path to the archive file
        #[arg(short, long, default_value = "blocks_1000.solarch")]
        archive: PathBuf,

        /// Retention threshold in blocks (age >= retain is considered stale)
        #[arg(long, default_value = "20")]
        retain: u64,
    },

    /// Inspect unified SOL fixture bundles (`.soltest`)
    #[command(subcommand)]
    Fixture(FixtureCommands),
}

#[derive(Subcommand)]
enum FixtureCommands {
    /// Inspect fixture metadata and embedded payload sizes
    Inspect {
        /// Fixture path
        #[arg(short, long)]
        fixture: PathBuf,
    },
}

impl SolCommands {
    fn requests_samply_bytehound_session(&self) -> bool {
        matches!(
            self,
            Self::QuickBench {
                cpu_profiler: Some(CpuProfilerKind::Samply),
                ..
            }
        )
    }

    fn relaunch_under_bytehound_if_needed(&self) -> Result<(), Box<dyn std::error::Error>> {
        if !self.requests_samply_bytehound_session() {
            return Ok(());
        }

        let current_exe = std::env::current_exe()?;
        let bytehound_binary = ensure_samply_profiled_binary(&current_exe)?;
        if bytehound_binary == current_exe {
            return Ok(());
        }
        let args = std::env::args_os().skip(1).collect::<Vec<_>>();
        let status = ProcessCommand::new(&bytehound_binary).args(args).status()?;
        std::process::exit(status.code().unwrap_or(1));
    }

    async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        self.relaunch_under_bytehound_if_needed()?;
        match self {
            Self::Extract {
                blocks,
                start_height,
                end_height,
                checkpoint,
                snapshot_pma,
                snapshot_manifest,
                kernel,
                output,
                include_mempool,
            } => {
                commands::sol::cmd_sol_extract(
                    blocks, start_height, end_height, checkpoint, snapshot_pma, snapshot_manifest,
                    kernel, output, include_mempool,
                )
                .await
            }
            Self::QuickBench {
                fixture,
                blocks,
                skip_genesis,
                fsync,
                profile_memory,
                profile_interval_ms,
                profile_output,
                cpu_profiler,
                cpu_profile_rate,
                cpu_profile_output,
                gc_drop_threshold_mib,
                page_fault_minor_burst_threshold,
                page_fault_major_burst_threshold,
            } => {
                let fsync_enabled = fsync.enabled();

                commands::sol::cmd_sol_quick_bench(commands::sol::QuickBenchOptions {
                    fixture,
                    blocks,
                    skip_genesis,
                    fsync: fsync_enabled,
                    profile_memory,
                    profile_interval_ms,
                    profile_output,
                    cpu_profiler,
                    cpu_profile_rate,
                    cpu_profile_output,
                    gc_drop_threshold_mib,
                    page_fault_minor_burst_threshold,
                    page_fault_major_burst_threshold,
                })
                .await
            }
            Self::QuickReadBench {
                checkpoint,
                snapshot_pma,
                snapshot_manifest,
                kernel,
                start_height,
                end_height,
                count,
                fsync,
                dry_run,
                profile_memory,
                profile_interval_ms,
                profile_output,
                cpu_profiler,
                cpu_profile_rate,
                cpu_profile_output,
            } => {
                commands::sol::cmd_sol_quick_read_bench(commands::sol::QuickReadBenchOptions {
                    checkpoint,
                    snapshot_pma,
                    snapshot_manifest,
                    kernel,
                    start_height,
                    end_height,
                    count,
                    fsync: fsync.enabled(),
                    dry_run,
                    profile_memory,
                    profile_interval_ms,
                    profile_output,
                    cpu_profiler,
                    cpu_profile_rate,
                    cpu_profile_output,
                })
                .await
            }
            Self::QuickOrchestrate {
                plan,
                profile_output,
                cold_mode,
                fsync,
            } => {
                commands::sol::cmd_sol_quick_orchestrate(commands::sol::QuickOrchestrateOptions {
                    plan,
                    profile_output,
                    cold_mode: cold_mode.into(),
                    fsync: fsync.enabled(),
                })
                .await
            }
            Self::Bench {
                benchmark,
                plan,
                fixture,
                checkpoint,
                snapshot_pma,
                snapshot_manifest,
                kernel,
                start_height,
                end_height,
                count,
                peek_mode,
                output,
                blocks,
                skip_genesis,
                profile_memory,
                profile_interval_ms,
                threads,
                warmup_runs,
                measured_runs,
                cooldown_secs,
                cv_threshold,
                label,
                docker_image,
                docker_build_tag,
                memory_limit,
                work_dir_mode,
                cpuset,
                cpu_quota,
                cpu_period,
                allow_version_skew,
                allow_degraded_cold,
                allow_debug_benchmark,
            } => {
                commands::sol::cmd_sol_bench(
                    benchmark,
                    plan,
                    fixture,
                    checkpoint,
                    snapshot_pma,
                    snapshot_manifest,
                    kernel,
                    start_height,
                    end_height,
                    count,
                    peek_mode.into(),
                    output,
                    blocks,
                    skip_genesis,
                    profile_memory,
                    profile_interval_ms,
                    threads,
                    warmup_runs,
                    measured_runs,
                    cooldown_secs,
                    cv_threshold,
                    label,
                    docker_image,
                    docker_build_tag,
                    memory_limit,
                    work_dir_mode,
                    cpuset,
                    cpu_quota,
                    cpu_period,
                    allow_version_skew,
                    allow_degraded_cold,
                    allow_debug_benchmark,
                )
                .await
            }
            Self::RunOnce {
                resolved_case,
                run_dir,
                work_dir,
                run_id,
            } => commands::sol::cmd_sol_run_once(resolved_case, run_dir, work_dir, run_id).await,
            Self::QuickReadOnce {
                checkpoint,
                snapshot_pma,
                snapshot_manifest,
                kernel,
                start_height,
                end_height,
                fsync,
                dry_run,
            } => {
                commands::sol::cmd_sol_quick_read_once(commands::sol::QuickReadBenchOptions {
                    checkpoint,
                    snapshot_pma,
                    snapshot_manifest,
                    kernel,
                    start_height,
                    end_height: Some(end_height),
                    count: None,
                    fsync: fsync.enabled(),
                    dry_run,
                    profile_memory: false,
                    profile_interval_ms: 500,
                    profile_output: None,
                    cpu_profiler: None,
                    cpu_profile_rate: 1000,
                    cpu_profile_output: None,
                })
                .await
            }
            Self::BinaryIdentity => commands::sol::cmd_sol_binary_identity(),
            Self::Validate {
                fixture,
                output,
                docker_image,
                docker_build_tag,
                memory_limit,
                work_dir_mode,
                cpuset,
                cpu_quota,
                cpu_period,
            } => {
                commands::sol::cmd_sol_validate(
                    fixture, output, docker_image, docker_build_tag, memory_limit, work_dir_mode,
                    cpuset, cpu_quota, cpu_period,
                )
                .await
            }
            Self::Sweep {
                matrix,
                output,
                interleave,
                randomize_order,
                comparison_markdown,
            } => {
                commands::sol::cmd_sol_sweep(
                    matrix, output, interleave, randomize_order, comparison_markdown,
                )
                .await
            }
            Self::ValidateProbe => commands::sol::cmd_sol_validate_probe(),
            Self::Inspect { archive, retain } => commands::sol::cmd_sol_inspect(archive, retain),
            Self::Fixture(fixture) => fixture.run().await,
        }
    }
}

impl FixtureCommands {
    async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Inspect { fixture } => commands::sol::cmd_sol_fixture_inspect(fixture),
        }
    }
}

impl Cli {
    fn parse() -> Self {
        let cli = <Self as Parser>::parse();
        cli.validate_or_exit()
    }

    #[cfg(test)]
    fn try_parse_from<I, T>(itr: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let cli = <Self as Parser>::try_parse_from(itr)?;
        cli.validate()
    }

    fn validate(self) -> Result<Self, clap::Error> {
        match &self.command {
            Commands::Sol(sol_cmd) => sol_cmd.validate_parse_semantics()?,
        }
        Ok(self)
    }

    fn validate_or_exit(self) -> Self {
        match self.validate() {
            Ok(cli) => cli,
            Err(error) => error.exit(),
        }
    }

    fn value_validation_error(message: impl Into<String>) -> clap::Error {
        let mut error = clap::Error::raw(clap::error::ErrorKind::ValueValidation, message.into());
        error.insert(
            clap::error::ContextKind::InvalidArg,
            clap::error::ContextValue::String("--end-height".to_string()),
        );
        error.with_cmd(&<Self as clap::CommandFactory>::command())
    }
}

impl SolCommands {
    fn validate_parse_semantics(&self) -> Result<(), clap::Error> {
        match self {
            Self::QuickReadBench {
                start_height,
                end_height: Some(end_height),
                ..
            } if end_height < start_height => Err(Cli::value_validation_error(format!(
                "--end-height ({end_height}) must be greater than or equal to --start-height ({start_height})"
            ))),
            Self::Bench {
                start_height: Some(start_height),
                end_height: Some(end_height),
                ..
            } if end_height < start_height => Err(Cli::value_validation_error(format!(
                "--end-height ({end_height}) must be greater than or equal to --start-height ({start_height})"
            ))),
            Self::QuickReadOnce {
                start_height,
                end_height,
                ..
            } if end_height < start_height => Err(Cli::value_validation_error(format!(
                "--end-height ({end_height}) must be greater than or equal to --start-height ({start_height})"
            ))),
            _ => Ok(()),
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Sol(sol_cmd) => sol_cmd.run().await,
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    fn subcommand_names(command: &clap::Command) -> Vec<String> {
        command
            .get_subcommands()
            .filter(|subcommand| !subcommand.is_hide_set())
            .map(|subcommand| subcommand.get_name().to_string())
            .collect()
    }

    fn render_help(mut command: clap::Command) -> String {
        let mut buffer = Vec::new();
        command.write_long_help(&mut buffer).expect("render help");
        String::from_utf8(buffer).expect("utf8 help")
    }

    #[test]
    fn test_phase1_cli_surface() {
        let command = Cli::command();
        let top_level = subcommand_names(&command);

        assert_eq!(top_level, vec!["sol"]);

        let sol = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "sol")
            .expect("sol subcommand");

        assert_eq!(
            subcommand_names(sol),
            vec![
                "extract", "quick-bench", "quick-read-bench", "quick-orchestrate", "bench",
                "validate", "sweep", "inspect", "fixture",
            ]
        );

        let fixture = sol
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "fixture")
            .expect("fixture subcommand");

        assert_eq!(subcommand_names(fixture), vec!["inspect"]);
    }

    #[test]
    fn test_sol_bench_cli_surface() {
        let command = Cli::command();
        let sol = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "sol")
            .expect("sol subcommand");

        assert!(subcommand_names(sol).contains(&"bench".to_string()));
        assert!(subcommand_names(sol).contains(&"quick-bench".to_string()));
    }

    #[test]
    fn test_sol_quick_read_bench_cli_lists_subcommand() {
        let command = Cli::command();
        let sol = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "sol")
            .expect("sol subcommand");

        assert!(subcommand_names(sol).contains(&"quick-read-bench".to_string()));
    }

    #[test]
    fn test_sol_quick_orchestrate_cli_lists_subcommand() {
        let command = Cli::command();
        let sol = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "sol")
            .expect("sol subcommand");

        assert!(subcommand_names(sol).contains(&"quick-orchestrate".to_string()));
    }

    #[test]
    fn test_sol_quick_bench_requests_bytehound_session_for_samply() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-bench", "--fixture", "fixture.soltest",
            "--cpu-profiler", "samply", "--cpu-profile-output", "profile.json.gz",
        ])
        .expect("parse quick bench");

        let Commands::Sol(sol) = cli.command;
        assert!(sol.requests_samply_bytehound_session());
    }

    #[test]
    fn test_sol_sweep_without_samply_does_not_request_bytehound_session() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "sweep", "--matrix", "matrix.json", "--output", "out",
        ])
        .expect("parse sweep");

        let Commands::Sol(sol) = cli.command;
        assert!(!sol.requests_samply_bytehound_session());
    }

    #[test]
    fn test_sol_help_warns_about_quick_bench() {
        let command = Cli::command();
        let sol = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "sol")
            .expect("sol subcommand")
            .clone();
        let help = render_help(sol);

        assert!(help.contains("quick-bench"));
        assert!(help.contains("NOT reproducible data"));
    }

    #[test]
    fn test_sol_quick_read_bench_cli_parses_checkpoint_range() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-read-bench", "--checkpoint", "checkpoint.chkjam",
            "--start-height", "7", "--end-height", "42",
        ])
        .expect("parse quick-read-bench");

        match cli.command {
            Commands::Sol(SolCommands::QuickReadBench {
                checkpoint,
                kernel,
                start_height,
                end_height,
                count,
                dry_run,
                ..
            }) => {
                assert_eq!(checkpoint, Some(PathBuf::from("checkpoint.chkjam")));
                assert_eq!(kernel, PathBuf::from("assets/dumb.jam"));
                assert_eq!(start_height, 7);
                assert_eq!(end_height, Some(42));
                assert_eq!(count, None);
                assert!(!dry_run);
            }
            _ => panic!("expected sol quick-read-bench command"),
        }
    }

    #[test]
    fn test_sol_quick_read_bench_cli_parses_snapshot_pair() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-read-bench", "--snapshot-pma", "snapshot.pma",
            "--snapshot-manifest", "snapshot.manifest", "--kernel", "kernel.jam", "--start-height",
            "7", "--count", "3",
        ])
        .expect("parse snapshot quick-read-bench");

        match cli.command {
            Commands::Sol(SolCommands::QuickReadBench {
                checkpoint,
                snapshot_pma,
                snapshot_manifest,
                kernel,
                start_height,
                count,
                ..
            }) => {
                assert_eq!(checkpoint, None);
                assert_eq!(snapshot_pma, Some(PathBuf::from("snapshot.pma")));
                assert_eq!(snapshot_manifest, Some(PathBuf::from("snapshot.manifest")));
                assert_eq!(kernel, PathBuf::from("kernel.jam"));
                assert_eq!(start_height, 7);
                assert_eq!(count, Some(3));
            }
            _ => panic!("expected sol quick-read-bench command"),
        }
    }

    #[test]
    fn test_sol_quick_read_bench_cli_rejects_incomplete_or_conflicting_snapshot_pair() {
        let missing_manifest = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-read-bench", "--snapshot-pma", "snapshot.pma",
        ]);
        assert!(missing_manifest.is_err());

        let missing_pma = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-read-bench", "--snapshot-manifest",
            "snapshot.manifest",
        ]);
        assert!(missing_pma.is_err());

        let conflict = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-read-bench", "--checkpoint", "checkpoint.chkjam",
            "--snapshot-pma", "snapshot.pma", "--snapshot-manifest", "snapshot.manifest",
        ]);
        assert!(conflict.is_err());
    }

    #[test]
    fn test_sol_quick_read_bench_cli_rejects_count_and_end_height_together() {
        let result = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-read-bench", "--checkpoint", "checkpoint.chkjam",
            "--count", "3", "--end-height", "42",
        ]);

        assert!(result.is_err(), "count and end-height together should fail");
        let rendered = result.err().expect("clap parse error").to_string();
        assert!(rendered.contains("--count"));
        assert!(rendered.contains("--end-height"));
    }

    #[test]
    fn test_sol_quick_read_bench_cli_rejects_zero_count() {
        let result = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-read-bench", "--checkpoint", "checkpoint.chkjam",
            "--count", "0",
        ]);

        assert!(result.is_err(), "zero count should fail");
        let rendered = result.err().expect("clap parse error").to_string();
        assert!(rendered.contains("--count"));
    }

    #[test]
    fn test_sol_quick_read_bench_cli_rejects_end_height_before_start_at_parse_time() {
        let result = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-read-bench", "--checkpoint", "checkpoint.chkjam",
            "--start-height", "9", "--end-height", "8",
        ]);

        assert!(
            result.is_err(),
            "end-height before start-height should fail parse"
        );
        let rendered = result.err().expect("clap parse error").to_string();
        assert!(rendered.contains("--end-height"));
        assert!(rendered.contains("--start-height"));
    }

    #[test]
    fn test_sol_quick_read_bench_cli_hides_and_parses_quick_read_once() {
        let command = Cli::command();
        let sol = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "sol")
            .expect("sol subcommand")
            .clone();
        let help = render_help(sol);

        assert!(!help.contains("quick-read-once"));

        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-read-once", "--checkpoint", "checkpoint.chkjam",
            "--kernel", "kernel.jam", "--start-height", "11", "--end-height", "13", "--dry-run",
        ])
        .expect("parse quick-read-once");

        match cli.command {
            Commands::Sol(SolCommands::QuickReadOnce {
                checkpoint,
                kernel,
                start_height,
                end_height,
                dry_run,
                ..
            }) => {
                assert_eq!(checkpoint, Some(PathBuf::from("checkpoint.chkjam")));
                assert_eq!(kernel, PathBuf::from("kernel.jam"));
                assert_eq!(start_height, 11);
                assert_eq!(end_height, 13);
                assert!(dry_run);
            }
            _ => panic!("expected sol quick-read-once command"),
        }
    }

    #[test]
    fn test_sol_quick_read_once_cli_parses_snapshot_pair() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-read-once", "--snapshot-pma", "snapshot.pma",
            "--snapshot-manifest", "snapshot.manifest", "--kernel", "kernel.jam", "--start-height",
            "11", "--end-height", "13",
        ])
        .expect("parse quick-read-once snapshot source");

        match cli.command {
            Commands::Sol(SolCommands::QuickReadOnce {
                checkpoint,
                snapshot_pma,
                snapshot_manifest,
                kernel,
                ..
            }) => {
                assert_eq!(checkpoint, None);
                assert_eq!(snapshot_pma, Some(PathBuf::from("snapshot.pma")));
                assert_eq!(snapshot_manifest, Some(PathBuf::from("snapshot.manifest")));
                assert_eq!(kernel, PathBuf::from("kernel.jam"));
            }
            _ => panic!("expected sol quick-read-once command"),
        }
    }

    #[test]
    fn test_sol_quick_read_once_cli_rejects_end_height_before_start_at_parse_time() {
        let result = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-read-once", "--checkpoint", "checkpoint.chkjam",
            "--kernel", "kernel.jam", "--start-height", "13", "--end-height", "12",
        ]);

        assert!(
            result.is_err(),
            "quick-read-once end-height before start-height should fail parse"
        );
        let rendered = result.err().expect("clap parse error").to_string();
        assert!(rendered.contains("--end-height"));
        assert!(rendered.contains("--start-height"));
    }

    #[test]
    fn test_sol_quick_read_bench_cli_parses_fsync_modes() {
        let off_cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-read-bench", "--checkpoint", "checkpoint.chkjam",
            "--fsync", "off",
        ])
        .expect("parse quick-read-bench fsync off");
        let on_cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-read-bench", "--checkpoint", "checkpoint.chkjam",
            "--fsync", "on",
        ])
        .expect("parse quick-read-bench fsync on");

        match off_cli.command {
            Commands::Sol(SolCommands::QuickReadBench { fsync, .. }) => {
                assert_eq!(fsync, BenchFsyncMode::Off);
            }
            _ => panic!("expected sol quick-read-bench command"),
        }

        match on_cli.command {
            Commands::Sol(SolCommands::QuickReadBench { fsync, .. }) => {
                assert_eq!(fsync, BenchFsyncMode::On);
            }
            _ => panic!("expected sol quick-read-bench command"),
        }
    }

    #[test]
    fn test_sol_quick_read_once_cli_parses_fsync_modes() {
        let off_cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-read-once", "--checkpoint", "checkpoint.chkjam",
            "--kernel", "kernel.jam", "--start-height", "11", "--end-height", "13", "--fsync",
            "off",
        ])
        .expect("parse quick-read-once fsync off");
        let on_cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-read-once", "--checkpoint", "checkpoint.chkjam",
            "--kernel", "kernel.jam", "--start-height", "11", "--end-height", "13", "--fsync",
            "on",
        ])
        .expect("parse quick-read-once fsync on");

        match off_cli.command {
            Commands::Sol(SolCommands::QuickReadOnce { fsync, .. }) => {
                assert_eq!(fsync, BenchFsyncMode::Off);
            }
            _ => panic!("expected sol quick-read-once command"),
        }

        match on_cli.command {
            Commands::Sol(SolCommands::QuickReadOnce { fsync, .. }) => {
                assert_eq!(fsync, BenchFsyncMode::On);
            }
            _ => panic!("expected sol quick-read-once command"),
        }
    }

    #[test]
    fn test_sol_quick_orchestrate_cli_parses_plan_and_profile_output() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-orchestrate", "--plan", "plan.json",
            "--profile-output", "out.json",
        ])
        .expect("parse quick-orchestrate");

        match cli.command {
            Commands::Sol(SolCommands::QuickOrchestrate {
                plan,
                profile_output,
                cold_mode,
                ..
            }) => {
                assert_eq!(plan, PathBuf::from("plan.json"));
                assert_eq!(profile_output, Some(PathBuf::from("out.json")));
                assert_eq!(cold_mode, QuickOrchestrateColdMode::Strict);
            }
            _ => panic!("expected sol quick-orchestrate command"),
        }
    }

    #[test]
    fn test_sol_quick_orchestrate_cli_parses_cold_mode() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-orchestrate", "--plan", "plan.json", "--cold-mode",
            "soft",
        ])
        .expect("parse quick-orchestrate with cold mode");

        match cli.command {
            Commands::Sol(SolCommands::QuickOrchestrate { cold_mode, .. }) => {
                assert_eq!(cold_mode, QuickOrchestrateColdMode::Soft);
            }
            _ => panic!("expected sol quick-orchestrate command"),
        }
    }

    #[test]
    fn test_sol_quick_orchestrate_cli_parses_fsync_modes() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-orchestrate", "--plan", "plan.json", "--fsync", "off",
        ])
        .expect("parse quick-orchestrate with fsync");

        match cli.command {
            Commands::Sol(SolCommands::QuickOrchestrate { fsync, .. }) => {
                assert_eq!(fsync, BenchFsyncMode::Off);
            }
            _ => panic!("expected sol quick-orchestrate command"),
        }
    }

    #[test]
    fn test_sol_quick_orchestrate_cli_defaults_fsync_on() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-orchestrate", "--plan", "plan.json",
        ])
        .expect("parse quick-orchestrate with default fsync");

        match cli.command {
            Commands::Sol(SolCommands::QuickOrchestrate { fsync, .. }) => {
                assert_eq!(fsync, BenchFsyncMode::On);
            }
            _ => panic!("expected sol quick-orchestrate command"),
        }
    }

    #[test]
    fn test_sol_quick_bench_help_mentions_bytehound_session_for_samply() {
        let command = Cli::command();
        let quick_bench = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "sol")
            .expect("sol subcommand")
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "quick-bench")
            .expect("quick-bench subcommand")
            .clone();
        let help = render_help(quick_bench);

        assert!(help.contains("bytehound"));
        assert!(!help.contains("adds one extra profiled replay pass and writes the raw profile"));
    }

    #[test]
    fn test_sol_sweep_help_rejects_trusted_cpu_profiling() {
        let command = Cli::command();
        let sweep = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "sol")
            .expect("sol subcommand")
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "sweep")
            .expect("sweep subcommand")
            .clone();
        let help = render_help(sweep);

        assert!(help.contains("Trusted CPU profiling is not supported"));
        assert!(!help.contains("bytehound"));
        assert!(!help.contains("keeps that pass out of trusted measured-run statistics"));
    }

    #[test]
    fn test_sol_bench_help_hides_quick_only_flags() {
        let command = Cli::command();
        let bench = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "sol")
            .expect("sol subcommand")
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "bench")
            .expect("bench subcommand")
            .clone();
        let help = render_help(bench);

        assert!(!help.contains("--checkpoint-recovery-timeout-ms"));
        assert!(!help.contains("--gc-drop-threshold-mib"));
        assert!(!help.contains("--page-fault-minor-burst-threshold"));
    }

    #[test]
    fn test_sol_help_hides_internal_run_once() {
        let command = Cli::command();
        let sol = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "sol")
            .expect("sol subcommand")
            .clone();
        let help = render_help(sol);

        assert!(!help.contains("run-once"));
    }

    #[test]
    fn test_sol_run_once_cli_parses_hidden_command() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "run-once", "--resolved-case", "resolved_case.json",
            "--run-dir", "out/run-0", "--work-dir", "/bench/work/run-0",
        ])
        .expect("parse run-once");

        match cli.command {
            Commands::Sol(SolCommands::RunOnce {
                resolved_case,
                run_dir,
                work_dir,
                run_id,
            }) => {
                assert_eq!(resolved_case, PathBuf::from("resolved_case.json"));
                assert_eq!(run_dir, PathBuf::from("out/run-0"));
                assert_eq!(work_dir, Some(PathBuf::from("/bench/work/run-0")));
                assert_eq!(run_id, None);
            }
            _ => panic!("expected sol run-once command"),
        }
    }

    #[test]
    fn test_sol_help_hides_internal_binary_identity() {
        let command = Cli::command();
        let sol = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "sol")
            .expect("sol subcommand")
            .clone();
        let help = render_help(sol);

        assert!(!help.contains("binary-identity"));
    }

    #[test]
    fn test_sol_binary_identity_cli_parses_hidden_command() {
        let cli = Cli::try_parse_from(["nockchain-bench", "sol", "binary-identity"])
            .expect("parse binary-identity");

        match cli.command {
            Commands::Sol(SolCommands::BinaryIdentity) => {}
            _ => panic!("expected sol binary-identity command"),
        }
    }

    #[test]
    fn test_sol_help_lists_validate_command() {
        let command = Cli::command();
        let sol = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "sol")
            .expect("sol subcommand")
            .clone();
        let help = render_help(sol);

        assert!(help.contains("validate"));
    }

    #[test]
    fn test_sol_help_lists_sweep_command() {
        let command = Cli::command();
        let sol = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "sol")
            .expect("sol subcommand")
            .clone();
        let help = render_help(sol);

        assert!(help.contains("sweep"));
    }

    #[test]
    fn test_sol_help_hides_validate_probe() {
        let command = Cli::command();
        let sol = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "sol")
            .expect("sol subcommand")
            .clone();
        let help = render_help(sol);

        assert!(!help.contains("validate-probe"));
    }

    #[test]
    fn docker_image_cli_validate_parses_required_flags() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "validate", "--fixture", "fixture.soltest", "--output",
            "out", "--docker-image", "ghcr.io/org/nockchain-bench@sha256:abc", "--memory-limit",
            "2g", "--work-dir-mode", "docker-tmpfs", "--cpuset", "0-3", "--cpu-quota", "200000",
            "--cpu-period", "100000",
        ])
        .expect("parse validate");

        match cli.command {
            Commands::Sol(SolCommands::Validate {
                fixture,
                output,
                docker_image,
                docker_build_tag,
                memory_limit,
                work_dir_mode,
                cpuset,
                cpu_quota,
                cpu_period,
            }) => {
                assert_eq!(fixture, PathBuf::from("fixture.soltest"));
                assert_eq!(output, PathBuf::from("out"));
                assert_eq!(
                    docker_image,
                    Some("ghcr.io/org/nockchain-bench@sha256:abc".to_string())
                );
                assert_eq!(docker_build_tag, None);
                assert_eq!(memory_limit, "2g");
                assert_eq!(work_dir_mode, BenchWorkDirMode::DockerTmpfs);
                assert_eq!(cpuset.as_deref(), Some("0-3"));
                assert_eq!(cpu_quota, Some(200000));
                assert_eq!(cpu_period, Some(100000));
            }
            _ => panic!("expected sol validate command"),
        }
    }

    #[test]
    fn test_sol_validate_probe_cli_parses_hidden_command() {
        let cli = Cli::try_parse_from(["nockchain-bench", "sol", "validate-probe"])
            .expect("parse validate-probe");

        match cli.command {
            Commands::Sol(SolCommands::ValidateProbe) => {}
            _ => panic!("expected sol validate-probe command"),
        }
    }

    #[test]
    fn docker_image_cli_bench_parses_byo_image_mode() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "bench", "--fixture", "fixture.soltest", "--output", "out",
            "--docker-image", "ghcr.io/org/nockchain-bench@sha256:abc", "--memory-limit", "2g",
            "--work-dir-mode", "docker-volume", "--cpuset", "0-3", "--cpu-quota", "200000",
            "--cpu-period", "100000", "--allow-version-skew",
        ])
        .expect("parse docker bench");

        match cli.command {
            Commands::Sol(SolCommands::Bench {
                docker_image,
                docker_build_tag,
                memory_limit,
                work_dir_mode,
                cpuset,
                cpu_quota,
                cpu_period,
                allow_version_skew,
                ..
            }) => {
                assert_eq!(
                    docker_image,
                    Some("ghcr.io/org/nockchain-bench@sha256:abc".to_string())
                );
                assert_eq!(docker_build_tag, None);
                assert_eq!(memory_limit.as_deref(), Some("2g"));
                assert_eq!(work_dir_mode, Some(BenchWorkDirMode::DockerVolume));
                assert_eq!(cpuset.as_deref(), Some("0-3"));
                assert_eq!(cpu_quota, Some(200000));
                assert_eq!(cpu_period, Some(100000));
                assert!(allow_version_skew);
            }
            _ => panic!("expected sol bench command"),
        }
    }

    #[test]
    fn docker_image_cli_bench_parses_auto_build_mode() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "bench", "--fixture", "fixture.soltest", "--output", "out",
            "--docker-build-tag", "nockchain-bench:local", "--memory-limit", "2g",
            "--work-dir-mode", "docker-volume",
        ])
        .expect("parse docker bench");

        match cli.command {
            Commands::Sol(SolCommands::Bench {
                docker_image,
                docker_build_tag,
                memory_limit,
                work_dir_mode,
                ..
            }) => {
                assert_eq!(docker_image, None);
                assert_eq!(docker_build_tag, Some("nockchain-bench:local".to_string()));
                assert_eq!(memory_limit.as_deref(), Some("2g"));
                assert_eq!(work_dir_mode, Some(BenchWorkDirMode::DockerVolume));
            }
            _ => panic!("expected sol bench command"),
        }
    }

    #[test]
    fn docker_image_cli_rejects_mutually_exclusive_image_source_flags() {
        let result = Cli::try_parse_from([
            "nockchain-bench", "sol", "bench", "--fixture", "fixture.soltest", "--output", "out",
            "--docker-image", "ghcr.io/org/nockchain-bench@sha256:abc", "--docker-build-tag",
            "nockchain-bench:local", "--memory-limit", "2g", "--work-dir-mode", "docker-volume",
        ]);

        assert!(result.is_err(), "expected clap to reject conflicting flags");
    }

    #[test]
    fn docker_image_cli_bench_defaults_to_native_mode_without_docker_source_flags() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "bench", "--fixture", "fixture.soltest", "--output", "out",
        ])
        .expect("parse native bench");

        match cli.command {
            Commands::Sol(SolCommands::Bench {
                docker_image,
                docker_build_tag,
                memory_limit,
                work_dir_mode,
                ..
            }) => {
                assert_eq!(docker_image, None);
                assert_eq!(docker_build_tag, None);
                assert_eq!(memory_limit, None);
                assert_eq!(work_dir_mode, None);
            }
            _ => panic!("expected sol bench command"),
        }
    }

    #[test]
    fn sol_bench_cli_parses_trusted_read_shorthand() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "bench", "--checkpoint", "checkpoint.chkjam", "--kernel",
            "kernel.jam", "--start-height", "7", "--count", "3", "--peek-mode", "cold-each",
            "--output", "out",
        ])
        .expect("parse trusted read bench");

        match cli.command {
            Commands::Sol(SolCommands::Bench {
                checkpoint,
                kernel,
                start_height,
                end_height,
                count,
                peek_mode,
                fixture,
                plan,
                ..
            }) => {
                assert_eq!(checkpoint, Some(PathBuf::from("checkpoint.chkjam")));
                assert_eq!(kernel, PathBuf::from("kernel.jam"));
                assert_eq!(start_height, Some(7));
                assert_eq!(end_height, None);
                assert_eq!(count, Some(3));
                assert_eq!(peek_mode, BenchPeekMode::ColdEach);
                assert_eq!(fixture, None);
                assert_eq!(plan, None);
            }
            _ => panic!("expected sol bench command"),
        }
    }

    #[test]
    fn sol_bench_cli_parses_trusted_snapshot_read_shorthand() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "bench", "--snapshot-pma", "snapshot.pma",
            "--snapshot-manifest", "snapshot.manifest", "--kernel", "kernel.jam", "--start-height",
            "7", "--count", "3", "--output", "out",
        ])
        .expect("parse trusted snapshot read bench");

        match cli.command {
            Commands::Sol(SolCommands::Bench {
                checkpoint,
                snapshot_pma,
                snapshot_manifest,
                fixture,
                plan,
                ..
            }) => {
                assert_eq!(checkpoint, None);
                assert_eq!(snapshot_pma, Some(PathBuf::from("snapshot.pma")));
                assert_eq!(snapshot_manifest, Some(PathBuf::from("snapshot.manifest")));
                assert_eq!(fixture, None);
                assert_eq!(plan, None);
            }
            _ => panic!("expected sol bench command"),
        }
    }

    #[test]
    fn sol_bench_cli_rejects_snapshot_pair_combined_with_checkpoint_fixture_or_plan() {
        for conflicting_flag in ["--checkpoint", "--fixture", "--plan"] {
            let result = Cli::try_parse_from([
                "nockchain-bench", "sol", "bench", conflicting_flag, "other.json",
                "--snapshot-pma", "snapshot.pma", "--snapshot-manifest", "snapshot.manifest",
                "--output", "out",
            ]);
            assert!(
                result.is_err(),
                "{conflicting_flag} should conflict with snapshot pair"
            );
        }
    }

    #[test]
    fn sol_extract_cli_parses_snapshot_pair() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "extract", "--snapshot-pma", "snapshot.pma",
            "--snapshot-manifest", "snapshot.manifest", "--kernel", "kernel.jam", "--start-height",
            "1", "--end-height", "2",
        ])
        .expect("parse snapshot extract");

        match cli.command {
            Commands::Sol(SolCommands::Extract {
                checkpoint,
                snapshot_pma,
                snapshot_manifest,
                kernel,
                ..
            }) => {
                assert_eq!(checkpoint, None);
                assert_eq!(snapshot_pma, Some(PathBuf::from("snapshot.pma")));
                assert_eq!(snapshot_manifest, Some(PathBuf::from("snapshot.manifest")));
                assert_eq!(kernel, PathBuf::from("kernel.jam"));
            }
            _ => panic!("expected sol extract command"),
        }
    }

    #[test]
    fn sol_bench_cli_rejects_end_height_before_start_at_parse_time() {
        let result = Cli::try_parse_from([
            "nockchain-bench", "sol", "bench", "--checkpoint", "checkpoint.chkjam",
            "--start-height", "11", "--end-height", "10", "--output", "out",
        ]);

        assert!(
            result.is_err(),
            "end-height before start-height should fail parse"
        );
        let rendered = result.err().expect("clap parse error").to_string();
        assert!(rendered.contains("--end-height"));
        assert!(rendered.contains("--start-height"));
    }

    #[test]
    fn test_sol_sweep_cli_parses_required_flags() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "sweep", "--matrix", "matrix.json", "--output", "out",
            "--comparison-markdown",
        ])
        .expect("parse sweep");

        match cli.command {
            Commands::Sol(SolCommands::Sweep {
                matrix,
                output,
                interleave,
                randomize_order,
                comparison_markdown,
                ..
            }) => {
                assert_eq!(matrix, PathBuf::from("matrix.json"));
                assert_eq!(output, PathBuf::from("out"));
                assert!(!interleave);
                assert!(!randomize_order);
                assert!(comparison_markdown);
            }
            _ => panic!("expected sol sweep command"),
        }
    }

    #[test]
    fn test_sol_sweep_help_hides_removed_multi_axis_flag() {
        let command = Cli::command();
        let sweep = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "sol")
            .expect("sol subcommand")
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "sweep")
            .expect("sweep subcommand")
            .clone();
        let help = render_help(sweep);

        assert!(!help.contains("--allow-multi-axis"));
    }

    #[test]
    fn test_sol_sweep_cli_rejects_cpu_profiler_flags() {
        let result = Cli::try_parse_from([
            "nockchain-bench", "sol", "sweep", "--matrix", "matrix.json", "--output", "out",
            "--cpu-profiler", "samply", "--cpu-profile-rate", "1000",
        ]);
        assert!(result.is_err(), "trusted sweep CPU profiler should fail");
        let rendered = result.err().expect("clap parse error").to_string();
        assert!(rendered.contains("--cpu-profiler"));
    }

    #[test]
    fn test_sol_sweep_cli_rejects_cpu_profiler_without_rate() {
        let result = Cli::try_parse_from([
            "nockchain-bench", "sol", "sweep", "--matrix", "matrix.json", "--output", "out",
            "--cpu-profiler", "samply",
        ]);
        assert!(result.is_err(), "trusted sweep CPU profiler should fail");
        let rendered = result.err().expect("clap parse error").to_string();
        assert!(rendered.contains("--cpu-profiler"));
    }

    #[test]
    fn test_sol_sweep_cli_rejects_cpu_profile_rate_without_profiler() {
        let result = Cli::try_parse_from([
            "nockchain-bench", "sol", "sweep", "--matrix", "matrix.json", "--output", "out",
            "--cpu-profile-rate", "1000",
        ]);
        assert!(
            result.is_err(),
            "profiling rate without profiler should fail"
        );
        let rendered = result.err().expect("clap parse error").to_string();
        assert!(rendered.contains("--cpu-profile-rate"));
    }

    #[test]
    fn test_sol_sweep_cli_rejects_zero_cpu_profile_rate() {
        let result = Cli::try_parse_from([
            "nockchain-bench", "sol", "sweep", "--matrix", "matrix.json", "--output", "out",
            "--cpu-profiler", "samply", "--cpu-profile-rate", "0",
        ]);
        assert!(result.is_err(), "trusted sweep CPU profiler should fail");
        let rendered = result.err().expect("clap parse error").to_string();
        assert!(rendered.contains("--cpu-profiler") || rendered.contains("--cpu-profile-rate"));
    }

    #[test]
    fn test_sol_sweep_cli_rejects_unsupported_cpu_profiler_values() {
        let result = Cli::try_parse_from([
            "nockchain-bench", "sol", "sweep", "--matrix", "matrix.json", "--output", "out",
            "--cpu-profiler", "not-a-profiler",
        ]);
        assert!(result.is_err(), "trusted sweep CPU profiler should fail");
        let rendered = result.err().expect("clap parse error").to_string();
        assert!(rendered.contains("--cpu-profiler"));
    }

    #[test]
    fn test_sol_quick_bench_cli_parses_cpu_profiler_flags() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-bench", "--fixture", "fixture.soltest",
            "--cpu-profiler", "samply", "--cpu-profile-rate", "1000", "--cpu-profile-output",
            "quick-bench-profile.json.gz",
        ])
        .expect("parse quick-bench with cpu profiler");

        match cli.command {
            Commands::Sol(SolCommands::QuickBench { fixture, .. }) => {
                assert_eq!(fixture, PathBuf::from("fixture.soltest"));
            }
            _ => panic!("expected sol quick-bench command"),
        }
    }

    #[test]
    fn test_sol_quick_bench_cli_parses_fsync_on() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-bench", "--fixture", "fixture.soltest", "--fsync",
            "on",
        ])
        .expect("parse quick-bench with fsync on");

        match cli.command {
            Commands::Sol(SolCommands::QuickBench { fsync, .. }) => {
                assert_eq!(fsync, BenchFsyncMode::On);
            }
            _ => panic!("expected sol quick-bench command"),
        }
    }

    #[test]
    fn test_sol_quick_bench_cli_defaults_fsync_on() {
        let cli = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-bench", "--fixture", "fixture.soltest",
        ])
        .expect("parse quick-bench with default fsync");

        match cli.command {
            Commands::Sol(SolCommands::QuickBench { fsync, .. }) => {
                assert_eq!(fsync, BenchFsyncMode::On);
            }
            _ => panic!("expected sol quick-bench command"),
        }
    }

    #[test]
    fn test_sol_quick_bench_cli_rejects_cpu_profile_rate_without_profiler() {
        let result = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-bench", "--fixture", "fixture.soltest",
            "--cpu-profile-rate", "1000",
        ]);
        assert!(
            result.is_err(),
            "profiling rate without profiler should fail"
        );
        let rendered = result.err().expect("clap parse error").to_string();
        assert!(rendered.contains("--cpu-profiler"));
    }

    #[test]
    fn test_sol_quick_bench_cli_rejects_zero_cpu_profile_rate() {
        let result = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-bench", "--fixture", "fixture.soltest",
            "--cpu-profiler", "samply", "--cpu-profile-output", "quick-bench-profile.json.gz",
            "--cpu-profile-rate", "0",
        ]);
        assert!(result.is_err(), "zero profiling rate should fail");
        let rendered = result.err().expect("clap parse error").to_string();
        assert!(rendered.contains("--cpu-profile-rate"));
    }

    #[test]
    fn test_sol_quick_bench_cli_requires_cpu_profile_output_when_profiler_is_set() {
        let result = Cli::try_parse_from([
            "nockchain-bench", "sol", "quick-bench", "--fixture", "fixture.soltest",
            "--cpu-profiler", "samply",
        ]);
        assert!(
            result.is_err(),
            "cpu profiler without output path should fail"
        );
        let rendered = result.err().expect("clap parse error").to_string();
        assert!(rendered.contains("--cpu-profile-output"));
    }

    #[test]
    fn test_sol_sweep_cli_rejects_conflicting_schedule_flags() {
        let result = Cli::try_parse_from([
            "nockchain-bench", "sol", "sweep", "--matrix", "matrix.json", "--output", "out",
            "--interleave", "--randomize-order",
        ]);
        assert!(
            result.is_err(),
            "conflicting sweep flags should fail during parse"
        );

        let rendered = result.err().expect("clap parse error").to_string();
        assert!(rendered.contains("--interleave"));
        assert!(rendered.contains("--randomize-order"));
    }
}
