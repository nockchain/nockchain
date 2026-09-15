#![allow(clippy::result_large_err)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use nockapp::kernel::boot::{self, Cli as BootCli};
use nockapp::noun::slab::NounSlab;
use nockapp::utils::NOCK_STACK_SIZE_HUGE;
use nockapp::wire::{SystemWire, Wire, WireRepr};
use nockapp::NockAppError;
use nockvm::mem::NockStack;
use nockvm::noun::{Noun, D, T};
use noun_serde::{NounDecode, NounEncode};
use roswell::{
    check_success, cue_file_to_stack, list_to_noun, make_tas, produce_full_hot_state,
    validate_puzzle_length, Roswell, RoswellCommand,
};
use tracing::info;
use zkvm_jetpack::form::ProofStreamWindow;

static H_ZOON_BENCH_SALT: AtomicU64 = AtomicU64::new(1);

#[derive(Parser)]
#[command(version, long_about = None)]
struct RoswellCli {
    #[command(flatten)]
    boot: BootCli,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Run all tests starting with NAME")]
    Test {
        #[arg(value_name = "NAME")]
        name: String,
    },
    #[command(name = "run-test", about = "Run all tests starting with NAME")]
    RunTest {
        #[arg(value_name = "NAME")]
        name: String,
    },
    #[command(name = "test-ci", about = "Run all CI tests")]
    TestCI,
    #[command(name = "run-suite", about = "Run all CI tests")]
    RunSuite,
    #[command(about = "Run all verifier tests")]
    TestVerifier,
    #[command(about = "Compare release verifier latency for equivalent v2 and v3 proofs")]
    BenchVerifier {
        #[arg(long, default_value_t = 8, help = "Measured runs per proof version")]
        runs: u64,
        #[arg(long, default_value_t = 2, help = "Warmup runs per proof version")]
        warmups: u64,
        #[arg(long, default_value_t = 64, help = "Power-of-two puzzle length")]
        puzzle_length: u64,
    },
    #[command(about = "Run all cryptography tests")]
    TestCrypto,
    #[command(about = "Run all nockchain tests")]
    TestDumb,
    #[command(about = "Run all nockchain benchmarks")]
    BenchDumb,
    #[command(name = "bench-h-zoon", about = "Time h-zoon digest-key hot paths")]
    BenchHZoon {
        #[arg(long, default_value_t = 7, help = "Measured runs per benchmark")]
        runs: u64,
        #[arg(long, default_value_t = 1, help = "Warmup runs per benchmark")]
        warmups: u64,
    },
    #[command(about = "Run all wallet tests")]
    TestWallet,
    #[command(about = "Run a shard of wallet tests")]
    TestWalletShard {
        #[arg(help = "Zero-based shard index")]
        shard: u64,
        #[arg(help = "Total number of shards")]
        total: u64,
    },
    #[command(about = "Run all zoon tests")]
    TestZoon,
    #[command(about = "Run all bridge tests")]
    TestBridge,
    #[command(
        name = "test-puzzle",
        about = "Test the built-in puzzle with optional overrides"
    )]
    TestPuzzle {
        #[arg(help = "Proof version")]
        v: u64,
        #[arg(help = "Puzzle length")]
        n: u64,
        #[arg(long, help = "Optional list of terms to override")]
        override_terms: Option<Vec<String>>,
    },
    #[command(
        name = "prove-puzzle",
        about = "Generate a complete proof for the built-in puzzle"
    )]
    ProvePuzzle {
        #[arg(help = "Proof version")]
        v: u64,
        #[arg(help = "Puzzle length")]
        n: u64,
        #[arg(long, help = "Save proof jam using this file stem")]
        filename: Option<String>,
    },
    #[command(
        name = "make-proof-snapshot",
        about = "Generate a proof-state snapshot for the built-in puzzle"
    )]
    MakeProofSnapshot {
        #[arg(help = "Proof version")]
        v: u64,
        #[arg(help = "Puzzle length")]
        n: u64,
        #[arg(long, help = "Save snapshot jam using this file stem")]
        filename: Option<String>,
    },
    #[command(
        name = "make-proof-stream-window",
        about = "Generate a proof stream window for the built-in puzzle"
    )]
    MakeProofStreamWindow {
        #[arg(help = "Proof version")]
        v: u64,
        #[arg(help = "Puzzle length")]
        n: u64,
        #[arg(help = "Start object index")]
        start: u64,
        #[arg(long, help = "Exclusive end object index")]
        end: Option<u64>,
        #[arg(long, help = "Save stream window jam using this file stem")]
        filename: Option<String>,
    },
    #[command(
        name = "assemble-proof-stream",
        about = "Assemble a complete proof from proof stream windows"
    )]
    AssembleProofStream {
        #[arg(long = "window", value_name = "PATH", required = true)]
        windows: Vec<PathBuf>,
        #[arg(long, help = "Save proof jam using this file stem")]
        filename: Option<String>,
    },
    #[command(
        name = "assemble-proof-continuation",
        about = "Assemble a complete proof from a snapshot and proof stream windows"
    )]
    AssembleProofContinuation {
        #[arg(long, value_name = "PATH")]
        snapshot: PathBuf,
        #[arg(long = "window", value_name = "PATH", required = true)]
        windows: Vec<PathBuf>,
        #[arg(long, help = "Save proof jam using this file stem")]
        filename: Option<String>,
    },
    #[command(name = "check-proof", about = "Verify a complete proof jam")]
    CheckProof {
        #[arg(long, value_name = "PATH")]
        proof: PathBuf,
    },
    #[command(about = "Compute a jammed Nock expression")]
    Compute {
        #[arg(long, value_name = "PATH")]
        nock: PathBuf,
    },
    #[command(about = "Benchmark unjetted decrement performance")]
    DecBenchmark {
        #[arg(help = "The number to decrement")]
        n: u64,
    },
}

#[tokio::main]
async fn main() -> Result<(), NockAppError> {
    let cli = RoswellCli::parse();
    boot::init_default_tracing(&cli.boot);
    let mut stack: NockStack = NockStack::new(NOCK_STACK_SIZE_HUGE, 0);

    if let Commands::BenchHZoon { runs, warmups } = &cli.command {
        bench_h_zoon(&cli, *runs, *warmups).await?;
        return Ok(());
    }

    if let Commands::BenchVerifier {
        runs,
        warmups,
        puzzle_length,
    } = &cli.command
    {
        bench_verifier(&cli, *runs, *warmups, *puzzle_length).await?;
        return Ok(());
    }

    let mut roswell =
        Roswell::boot_with_hot_state(cli.boot.clone(), &produce_full_hot_state()).await?;
    let wire = SystemWire.to_wire();
    let mut dummy_slab = NounSlab::new();

    let effects = match &cli.command {
        Commands::Test { name } | Commands::RunTest { name } => {
            let name = make_tas(&mut dummy_slab, name).as_noun();
            let slab = roswell.roswell_command("test", &[name], &mut dummy_slab)?;
            roswell.app.poke(wire, slab).await?
        }
        Commands::TestCI | Commands::RunSuite => {
            let slab = roswell.roswell_command("test-ci", &[], &mut dummy_slab)?;
            roswell.app.poke(wire, slab).await?
        }
        Commands::TestVerifier => {
            let slab = roswell.roswell_command("test-verifier", &[], &mut dummy_slab)?;
            roswell.app.poke(wire, slab).await?
        }
        Commands::BenchVerifier { .. } => {
            return Err(NockAppError::OtherError(
                "bench-verifier should run before regular app boot".to_string(),
            ));
        }
        Commands::TestCrypto => {
            let slab = roswell.roswell_command("test-crypto", &[], &mut dummy_slab)?;
            roswell.app.poke(wire, slab).await?
        }
        Commands::TestDumb => {
            let slab = roswell.roswell_command("test-dumb", &[], &mut dummy_slab)?;
            roswell.app.poke(wire, slab).await?
        }
        Commands::BenchDumb => {
            let slab = roswell.roswell_command("bench-dumb", &[], &mut dummy_slab)?;
            roswell.app.poke(wire, slab).await?
        }
        Commands::BenchHZoon { .. } => {
            return Err(NockAppError::OtherError(
                "bench-h-zoon should run before regular app boot".to_string(),
            ));
        }
        Commands::TestWallet => {
            let slab = roswell.roswell_command("test-wallet", &[], &mut dummy_slab)?;
            roswell.app.poke(wire, slab).await?
        }
        Commands::TestWalletShard { shard, total } => {
            let slab = roswell.roswell_command(
                "test-wallet-shard",
                &[D(*shard), D(*total)],
                &mut dummy_slab,
            )?;
            roswell.app.poke(wire, slab).await?
        }
        Commands::TestZoon => {
            let slab = roswell.roswell_command("test-zoon", &[], &mut dummy_slab)?;
            roswell.app.poke(wire, slab).await?
        }
        Commands::TestBridge => {
            let slab = roswell.roswell_command("test-bridge", &[], &mut dummy_slab)?;
            roswell.app.poke(wire, slab).await?
        }
        Commands::TestPuzzle {
            v,
            n,
            override_terms,
        } => {
            let pow_len = validate_puzzle_length(*n)?;
            let override_terms = parse_override(&mut dummy_slab, override_terms.clone());
            let slab = roswell.roswell_command(
                "test-puzzle",
                &[D(*v), pow_len, override_terms.unwrap_or(D(0))],
                &mut dummy_slab,
            )?;
            roswell.app.poke(wire, slab).await?
        }
        Commands::ProvePuzzle { v, n, filename } => {
            let name = match filename {
                Some(filename) => {
                    let file_tas = make_tas(&mut dummy_slab, filename).as_noun();
                    T(&mut dummy_slab, &[D(0), file_tas])
                }
                None => D(0),
            };
            let pow_len = validate_puzzle_length(*n)?;
            let slab = roswell.roswell_command(
                "prove-puzzle",
                &[D(*v), pow_len, name, D(0)],
                &mut dummy_slab,
            )?;
            roswell.app.poke(wire, slab).await?
        }
        Commands::MakeProofSnapshot { v, n, filename } => {
            let name = match filename {
                Some(filename) => {
                    let file_tas = make_tas(&mut dummy_slab, filename).as_noun();
                    T(&mut dummy_slab, &[D(0), file_tas])
                }
                None => D(0),
            };
            let pow_len = validate_puzzle_length(*n)?;
            let slab = roswell.roswell_command(
                "make-proof-snapshot",
                &[D(*v), pow_len, name, D(0)],
                &mut dummy_slab,
            )?;
            roswell.app.poke(wire, slab).await?
        }
        Commands::MakeProofStreamWindow {
            v,
            n,
            start,
            end,
            filename,
        } => {
            let name = match filename {
                Some(filename) => {
                    let file_tas = make_tas(&mut dummy_slab, filename).as_noun();
                    T(&mut dummy_slab, &[D(0), file_tas])
                }
                None => D(0),
            };
            let end = end
                .map(|value| T(&mut dummy_slab, &[D(0), D(value)]))
                .unwrap_or(D(0));
            let range = T(&mut dummy_slab, &[D(*start), end]);
            let pow_len = validate_puzzle_length(*n)?;
            let slab = roswell.roswell_command(
                "make-proof-stream-window",
                &[D(*v), pow_len, range, name, D(0)],
                &mut dummy_slab,
            )?;
            roswell.app.poke(wire, slab).await?
        }
        Commands::AssembleProofStream { windows, filename } => {
            let window_nouns = windows
                .iter()
                .map(|path| cue_file_to_stack(path, &mut stack))
                .collect::<Result<Vec<_>, _>>()?;
            let windows_list = list_to_noun(&mut stack, window_nouns);
            let name = match filename {
                Some(filename) => {
                    let file_tas = make_tas(&mut stack, filename).as_noun();
                    T(&mut stack, &[D(0), file_tas])
                }
                None => D(0),
            };
            let space = stack.noun_space();
            let mut assemble_slab = NounSlab::new();
            let assemble_cmd = roswell.roswell_command_with_space(
                "assemble-proof-stream",
                &[windows_list, name],
                Some(&space),
                &mut assemble_slab,
            )?;
            roswell.app.poke(wire, assemble_cmd).await?
        }
        Commands::AssembleProofContinuation {
            snapshot,
            windows,
            filename,
        } => {
            let snapshot_noun = cue_file_to_stack(snapshot, &mut stack)?;
            let window_nouns = windows
                .iter()
                .map(|path| cue_file_to_stack(path, &mut stack))
                .collect::<Result<Vec<_>, _>>()?;
            let Some(first_window_noun) = window_nouns.first() else {
                return Err(NockAppError::OtherError(
                    "at least one proof stream window is required".to_string(),
                ));
            };
            let first_window = {
                let space = stack.noun_space();
                ProofStreamWindow::from_noun(first_window_noun, &space).map_err(|err| {
                    NockAppError::OtherError(format!(
                        "failed to decode first stream window: {err:?}"
                    ))
                })?
            };
            let context = first_window.context.to_noun(&mut stack);
            let windows_list = list_to_noun(&mut stack, window_nouns);
            let name = match filename {
                Some(filename) => {
                    let file_tas = make_tas(&mut stack, filename).as_noun();
                    T(&mut stack, &[D(0), file_tas])
                }
                None => D(0),
            };
            let space = stack.noun_space();
            let mut assemble_slab = NounSlab::new();
            let assemble_cmd = roswell.roswell_command_with_space(
                "assemble-proof-continuation",
                &[snapshot_noun, context, windows_list, name],
                Some(&space),
                &mut assemble_slab,
            )?;
            roswell.app.poke(wire, assemble_cmd).await?
        }
        Commands::CheckProof { proof } => {
            let proof_noun = cue_file_to_stack(proof, &mut stack)?;
            let inner_some = T(&mut stack, &[D(0), proof_noun]);
            let outer_some = T(&mut stack, &[D(0), inner_some]);
            let space = stack.noun_space();
            let mut verify_slab = NounSlab::new();
            let verify_cmd = roswell.roswell_command_with_space(
                "verify-proof",
                &[outer_some],
                Some(&space),
                &mut verify_slab,
            )?;
            roswell.app.poke(SystemWire.to_wire(), verify_cmd).await?
        }
        Commands::Compute { nock } => {
            let nock_noun = cue_file_to_stack(nock, &mut stack)?;
            let space = stack.noun_space();
            let mut compute_slab = NounSlab::new();
            let compute_cmd = roswell.roswell_command_with_space(
                "compute",
                &[nock_noun],
                Some(&space),
                &mut compute_slab,
            )?;
            roswell.app.poke(SystemWire.to_wire(), compute_cmd).await?
        }
        Commands::DecBenchmark { n } => {
            let noun_n = D(*n);
            let slab = roswell.roswell_command("dec-benchmark", &[noun_n], &mut dummy_slab)?;
            roswell.app.poke(wire, slab).await?
        }
    };

    let success = check_success(effects)?;
    if !success {
        return Err(NockAppError::OtherError(String::from(
            "Roswell command failed",
        )));
    }

    info!("Roswell command completed successfully");
    roswell.save().await?;

    Ok(())
}

#[derive(Clone, Copy)]
enum VerifierBenchVersion {
    V2,
    V3,
}

impl VerifierBenchVersion {
    const fn atom(self) -> u64 {
        match self {
            Self::V2 => 2,
            Self::V3 => 3,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::V2 => "v2",
            Self::V3 => "v3",
        }
    }
}

#[derive(Default)]
struct VerifierBenchSamples {
    v2: Vec<Duration>,
    v3: Vec<Duration>,
}

impl VerifierBenchSamples {
    fn push(&mut self, version: VerifierBenchVersion, duration: Duration) {
        match version {
            VerifierBenchVersion::V2 => self.v2.push(duration),
            VerifierBenchVersion::V3 => self.v3.push(duration),
        }
    }

    fn get(&self, version: VerifierBenchVersion) -> &[Duration] {
        match version {
            VerifierBenchVersion::V2 => &self.v2,
            VerifierBenchVersion::V3 => &self.v3,
        }
    }
}

const VERIFIER_BENCH_VERSIONS: [VerifierBenchVersion; 2] =
    [VerifierBenchVersion::V2, VerifierBenchVersion::V3];

async fn bench_verifier(
    cli: &RoswellCli,
    runs: u64,
    warmups: u64,
    puzzle_length: u64,
) -> Result<(), NockAppError> {
    if cfg!(debug_assertions) {
        return Err(NockAppError::OtherError(
            "bench-verifier requires a release build".to_string(),
        ));
    }
    if runs == 0 || runs % 2 != 0 {
        return Err(NockAppError::OtherError(
            "bench-verifier requires a positive even number of measured runs".to_string(),
        ));
    }
    if warmups % 2 != 0 {
        return Err(NockAppError::OtherError(
            "bench-verifier requires an even number of warmup runs".to_string(),
        ));
    }

    let puzzle_length_noun = validate_puzzle_length(puzzle_length)?;
    let mut boot_cli = cli.boot.clone();
    boot_cli.new = true;
    boot_cli.ephemeral = true;
    let mut roswell = Roswell::boot_with_hot_state(boot_cli, &produce_full_hot_state()).await?;

    let setup_start = Instant::now();
    for version in VERIFIER_BENCH_VERSIONS {
        let setup = roswell
            .poke_command(
                RoswellCommand::PrepareVerifierBench,
                &[D(version.atom()), puzzle_length_noun],
                None,
            )
            .await?;
        setup.ensure_success(RoswellCommand::PrepareVerifierBench.as_str())?;
    }
    let setup_elapsed = setup_start.elapsed();

    for warmup_index in 0..warmups {
        for version in verifier_bench_order(warmup_index) {
            run_verifier_bench_arm(&mut roswell, version).await?;
        }
    }

    let mut samples = VerifierBenchSamples::default();
    for run_index in 0..runs {
        for version in verifier_bench_order(run_index) {
            let duration = run_verifier_bench_arm(&mut roswell, version).await?;
            samples.push(version, duration);
        }
    }

    let v2_median = median_duration(samples.get(VerifierBenchVersion::V2))?;
    if v2_median.is_zero() {
        return Err(NockAppError::OtherError(
            "v2 verifier benchmark had zero duration".to_string(),
        ));
    }
    let paired_delta_ms = samples
        .v2
        .iter()
        .zip(&samples.v3)
        .map(|(v2, v3)| (v3.as_secs_f64() - v2.as_secs_f64()) * 1_000.0)
        .collect::<Vec<_>>();
    let paired_delta_percent = samples
        .v2
        .iter()
        .zip(&samples.v3)
        .map(|(v2, v3)| ((v3.as_secs_f64() / v2.as_secs_f64()) - 1.0) * 100.0)
        .collect::<Vec<_>>();
    let delta_ms = median_f64(&paired_delta_ms)?;
    let delta_percent = median_f64(&paired_delta_percent)?;
    let mean_delta_ms = mean_f64(&paired_delta_ms)?;
    let mean_delta_percent = mean_f64(&paired_delta_percent)?;

    println!("Zoe verifier benchmark");
    println!("proof versions: v2 baseline vs v3 Zoe");
    println!("puzzle length: {puzzle_length}");
    println!("runs: {runs}, warmups: {warmups}");
    println!("runtime: fresh ephemeral kernel (no PMA or event-log I/O)");
    println!(
        "proof handling: both proofs preloaded; no timed proof serialization or per-arm proof ingress/copy"
    );
    if warmups == 0 {
        println!("result type: verifier-event latency without explicit verifier warmups");
    } else {
        println!("result type: warmed steady-state verifier-event latency");
    }
    println!(
        "proof generation: {} (excluded)",
        format_duration(setup_elapsed)
    );
    println!(
        "measurement boundary: app poke wall time, including common dispatch, cause/effect copying, and state cleanup; boot, proof generation, command construction, and effect decoding excluded"
    );
    for version in VERIFIER_BENCH_VERSIONS {
        let version_samples = samples.get(version);
        println!(
            "{} median:  {}",
            version.label(),
            format_duration(median_duration(version_samples)?)
        );
        println!(
            "{} mean:    {}",
            version.label(),
            format_duration(mean_duration(version_samples)?)
        );
        println!(
            "{} samples: {}",
            version.label(),
            format_samples(version_samples)
        );
    }
    println!("Zoe v3 paired median delta: {delta_ms:+.3}ms ({delta_percent:+.2}%)");
    println!("Zoe v3 paired mean delta:   {mean_delta_ms:+.3}ms ({mean_delta_percent:+.2}%)");

    roswell.save().await?;
    Ok(())
}

fn verifier_bench_order(index: u64) -> [VerifierBenchVersion; 2] {
    if index % 2 == 0 {
        [VerifierBenchVersion::V2, VerifierBenchVersion::V3]
    } else {
        [VerifierBenchVersion::V3, VerifierBenchVersion::V2]
    }
}

async fn run_verifier_bench_arm(
    roswell: &mut Roswell,
    version: VerifierBenchVersion,
) -> Result<Duration, NockAppError> {
    let mut slab = NounSlab::new();
    let command = roswell.roswell_command(
        RoswellCommand::BenchVerifier.as_str(),
        &[D(version.atom())],
        &mut slab,
    )?;
    let start = Instant::now();
    let effects = roswell.app.poke(SystemWire.to_wire(), command).await?;
    let elapsed = start.elapsed();
    let success = check_success(effects)?;
    if !success {
        return Err(NockAppError::OtherError(format!(
            "{} verifier benchmark arm failed",
            version.label()
        )));
    }
    Ok(elapsed)
}

fn mean_duration(samples: &[Duration]) -> Result<Duration, NockAppError> {
    if samples.is_empty() {
        return Err(NockAppError::OtherError(
            "no benchmark samples collected".to_string(),
        ));
    }
    let mean_seconds =
        samples.iter().map(Duration::as_secs_f64).sum::<f64>() / samples.len() as f64;
    Ok(Duration::from_secs_f64(mean_seconds))
}

fn median_f64(samples: &[f64]) -> Result<f64, NockAppError> {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    if sorted.is_empty() {
        return Err(NockAppError::OtherError(
            "no benchmark samples collected".to_string(),
        ));
    }
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        Ok((sorted[middle - 1] + sorted[middle]) / 2.0)
    } else {
        Ok(sorted[middle])
    }
}

fn mean_f64(samples: &[f64]) -> Result<f64, NockAppError> {
    if samples.is_empty() {
        return Err(NockAppError::OtherError(
            "no benchmark samples collected".to_string(),
        ));
    }
    Ok(samples.iter().sum::<f64>() / samples.len() as f64)
}

#[derive(Clone, Copy)]
enum HZoonBenchKind {
    Noop,
    ZMapBuild,
    HMapBuild,
    ZMapRead,
    HMapRead,
    ZMapUpdate,
    HMapUpdate,
}

impl HZoonBenchKind {
    fn command(self) -> &'static str {
        match self {
            HZoonBenchKind::Noop => "bench-h-zoon-noop",
            HZoonBenchKind::ZMapBuild => "bench-h-zoon-z-map-build",
            HZoonBenchKind::HMapBuild => "bench-h-zoon-h-map-build",
            HZoonBenchKind::ZMapRead => "bench-h-zoon-z-map-read",
            HZoonBenchKind::HMapRead => "bench-h-zoon-h-map-read",
            HZoonBenchKind::ZMapUpdate => "bench-h-zoon-z-map-update",
            HZoonBenchKind::HMapUpdate => "bench-h-zoon-h-map-update",
        }
    }

    fn label(self) -> &'static str {
        match self {
            HZoonBenchKind::Noop => "noop",
            HZoonBenchKind::ZMapBuild => "z-build",
            HZoonBenchKind::HMapBuild => "h-build",
            HZoonBenchKind::ZMapRead => "z-read",
            HZoonBenchKind::HMapRead => "h-read",
            HZoonBenchKind::ZMapUpdate => "z-update",
            HZoonBenchKind::HMapUpdate => "h-update",
        }
    }
}

#[derive(Default)]
struct HZoonBenchSamples {
    noop: Vec<Duration>,
    z_map_build: Vec<Duration>,
    h_map_build: Vec<Duration>,
    z_map_read: Vec<Duration>,
    h_map_read: Vec<Duration>,
    z_map_update: Vec<Duration>,
    h_map_update: Vec<Duration>,
}

impl HZoonBenchSamples {
    fn push(&mut self, kind: HZoonBenchKind, duration: Duration) {
        match kind {
            HZoonBenchKind::Noop => self.noop.push(duration),
            HZoonBenchKind::ZMapBuild => self.z_map_build.push(duration),
            HZoonBenchKind::HMapBuild => self.h_map_build.push(duration),
            HZoonBenchKind::ZMapRead => self.z_map_read.push(duration),
            HZoonBenchKind::HMapRead => self.h_map_read.push(duration),
            HZoonBenchKind::ZMapUpdate => self.z_map_update.push(duration),
            HZoonBenchKind::HMapUpdate => self.h_map_update.push(duration),
        }
    }

    fn get(&self, kind: HZoonBenchKind) -> &[Duration] {
        match kind {
            HZoonBenchKind::Noop => &self.noop,
            HZoonBenchKind::ZMapBuild => &self.z_map_build,
            HZoonBenchKind::HMapBuild => &self.h_map_build,
            HZoonBenchKind::ZMapRead => &self.z_map_read,
            HZoonBenchKind::HMapRead => &self.h_map_read,
            HZoonBenchKind::ZMapUpdate => &self.z_map_update,
            HZoonBenchKind::HMapUpdate => &self.h_map_update,
        }
    }
}

const H_ZOON_BENCH_KINDS: [HZoonBenchKind; 7] = [
    HZoonBenchKind::Noop,
    HZoonBenchKind::ZMapBuild,
    HZoonBenchKind::HMapBuild,
    HZoonBenchKind::ZMapRead,
    HZoonBenchKind::HMapRead,
    HZoonBenchKind::ZMapUpdate,
    HZoonBenchKind::HMapUpdate,
];

async fn bench_h_zoon(cli: &RoswellCli, runs: u64, warmups: u64) -> Result<(), NockAppError> {
    if runs == 0 {
        return Err(NockAppError::OtherError(
            "bench-h-zoon requires at least one measured run".to_string(),
        ));
    }

    for _ in 0..warmups {
        for kind in H_ZOON_BENCH_KINDS {
            run_h_zoon_bench_arm(cli, kind).await?;
        }
    }

    let mut samples = HZoonBenchSamples::default();
    for run_index in 0..runs {
        let shift = run_index as usize % H_ZOON_BENCH_KINDS.len();
        for offset in 0..H_ZOON_BENCH_KINDS.len() {
            let kind = H_ZOON_BENCH_KINDS[(shift + offset) % H_ZOON_BENCH_KINDS.len()];
            let duration = run_h_zoon_bench_arm(cli, kind).await?;
            samples.push(kind, duration);
        }
    }

    let noop_median = median_duration(samples.get(HZoonBenchKind::Noop))?;
    let z_build_median = median_duration(samples.get(HZoonBenchKind::ZMapBuild))?;
    let h_build_median = median_duration(samples.get(HZoonBenchKind::HMapBuild))?;
    let z_read_median = median_duration(samples.get(HZoonBenchKind::ZMapRead))?;
    let h_read_median = median_duration(samples.get(HZoonBenchKind::HMapRead))?;
    let z_update_median = median_duration(samples.get(HZoonBenchKind::ZMapUpdate))?;
    let h_update_median = median_duration(samples.get(HZoonBenchKind::HMapUpdate))?;

    let z_build_net = duration_net(z_build_median, noop_median);
    let h_build_net = duration_net(h_build_median, noop_median);
    let z_read_loop = duration_net(z_read_median, z_build_median);
    let h_read_loop = duration_net(h_read_median, h_build_median);
    let z_update_loop = duration_net(z_update_median, z_build_median);
    let h_update_loop = duration_net(h_update_median, h_build_median);

    println!("h-zoon hot-path benchmark");
    println!("runs: {runs}, warmups: {warmups}");
    for kind in H_ZOON_BENCH_KINDS {
        let kind_samples = samples.get(kind);
        println!(
            "{:>5} median:  {}",
            kind.label(),
            format_duration(median_duration(kind_samples)?)
        );
        println!(
            "{:>5} samples: {}",
            kind.label(),
            format_samples(kind_samples)
        );
    }
    println!("baseline:          {}", format_duration(noop_median));
    println!("z-build net:       {}", format_duration(z_build_net));
    println!("h-build net:       {}", format_duration(h_build_net));
    println!("z-read loop net:   {}", format_duration(z_read_loop));
    println!("h-read loop net:   {}", format_duration(h_read_loop));
    println!("z-update loop net: {}", format_duration(z_update_loop));
    println!("h-update loop net: {}", format_duration(h_update_loop));
    print_h_zoon_speedup("build speedup", z_build_net, h_build_net)?;
    print_h_zoon_speedup("read speedup", z_read_loop, h_read_loop)?;
    print_h_zoon_speedup("update speedup", z_update_loop, h_update_loop)?;

    Ok(())
}

fn print_h_zoon_speedup(label: &str, z_map: Duration, h_map: Duration) -> Result<(), NockAppError> {
    if z_map.is_zero() || h_map.is_zero() {
        return Err(NockAppError::OtherError(format!(
            "{label} had zero net time, increase benchmark work"
        )));
    }

    let speedup = z_map.as_secs_f64() / h_map.as_secs_f64();
    let delta_percent = (1.0 - (h_map.as_secs_f64() / z_map.as_secs_f64())) * 100.0;
    println!("{label}: {speedup:.2}x ({delta_percent:+.1}% vs z-map)");
    Ok(())
}

async fn run_h_zoon_bench_arm(
    cli: &RoswellCli,
    kind: HZoonBenchKind,
) -> Result<Duration, NockAppError> {
    let boot_start = Instant::now();
    let mut roswell =
        Roswell::boot_with_hot_state(cli.boot.clone(), &produce_full_hot_state()).await?;
    eprintln!(
        "booted {} benchmark app in {}",
        kind.label(),
        format_duration(boot_start.elapsed())
    );
    let wire: WireRepr = SystemWire.to_wire();
    let mut slab = NounSlab::new();
    let salt = D(H_ZOON_BENCH_SALT.fetch_add(1, Ordering::Relaxed));
    let command = roswell.roswell_command(kind.command(), &[salt], &mut slab)?;
    let start = Instant::now();
    let effects = roswell.app.poke(wire, command).await?;
    let elapsed = start.elapsed();
    let success = check_success(effects)?;
    if !success {
        return Err(NockAppError::OtherError(format!(
            "h-zoon benchmark arm failed: {}",
            kind.command()
        )));
    }
    eprintln!("ran {} in {}", kind.label(), format_duration(elapsed));
    Ok(elapsed)
}

fn median_duration(samples: &[Duration]) -> Result<Duration, NockAppError> {
    let mut sorted = samples.to_vec();
    sorted.sort();
    if sorted.is_empty() {
        return Err(NockAppError::OtherError(
            "no benchmark samples collected".to_string(),
        ));
    }
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        let seconds = (sorted[middle - 1].as_secs_f64() + sorted[middle].as_secs_f64()) / 2.0;
        Ok(Duration::from_secs_f64(seconds))
    } else {
        Ok(sorted[middle])
    }
}

fn duration_net(total: Duration, baseline: Duration) -> Duration {
    match total.checked_sub(baseline) {
        Some(duration) => duration,
        None => Duration::ZERO,
    }
}

fn format_samples(samples: &[Duration]) -> String {
    let mut formatted = String::new();
    for (index, sample) in samples.iter().enumerate() {
        if index > 0 {
            formatted.push_str(", ");
        }
        formatted.push_str(&format_duration(*sample));
    }
    formatted
}

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs_f64();
    if secs < 1.0 {
        format!("{:.2}ms", secs * 1_000.0)
    } else {
        format!("{:.3}s", secs)
    }
}

fn parse_override(slab: &mut NounSlab, terms: Option<Vec<String>>) -> Option<Noun> {
    terms.as_ref()?;
    terms.map(|values| {
        let terms: Vec<Noun> = values
            .iter()
            .map(|term| make_tas(slab, term).as_noun())
            .collect();
        list_to_noun(slab, terms)
    })
}
