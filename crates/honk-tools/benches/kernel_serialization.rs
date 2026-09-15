//! Compare serialization paths for one fully compiled kernel noun.
//!
//! Compilation and cueing are setup. Timed cases start from an already-owned
//! noun and run in separate child processes so their allocator high-water
//! marks do not contaminate one another:
//!
//! - `jam`: NockVM noun to canonical JAM bytes;
//! - `ast`: Nockasm noun to the sharing-preserving `NasmDag` AST;
//! - `text`: Nockasm noun through `NasmDag` to DAG text.
//!
//! The standard runner builds the Dumbnet kernel before invoking this bench:
//! `just honk-nockasm-serialization-bench`.

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};
use std::{env, fs};

use bytes::Bytes;
use nockapp::noun::slab::{NockJammer, NounSlab};

const DEFAULT_KERNEL_JAM: &str = "target/honk-nockasm-serialization/dumb.jam";

fn workspace_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Case {
    Jam,
    Ast,
    Text,
}

impl Case {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "jam" => Ok(Self::Jam),
            "ast" => Ok(Self::Ast),
            "text" => Ok(Self::Text),
            _ => Err(format!(
                "unknown case {value:?}; expected jam, ast, or text"
            )),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Jam => "jam",
            Self::Ast => "nockasm-dag-ast",
            Self::Text => "nockasm-dag-text",
        }
    }

    fn env_suffix(self) -> &'static str {
        match self {
            Self::Jam => "JAM",
            Self::Ast => "AST",
            Self::Text => "TEXT",
        }
    }

    fn default_samples(self) -> usize {
        20
    }

    fn default_warmups(self) -> usize {
        3
    }
}

struct ChildResult {
    durations: Vec<Duration>,
    output_bytes: Option<u64>,
    peak_rss_bytes: Option<u64>,
    wall: Duration,
    failure: Option<String>,
}

fn env_usize(name: &str, default: usize) -> Result<usize, String> {
    match env::var(name) {
        Ok(value) => value
            .parse::<usize>()
            .map_err(|err| format!("invalid {name}={value:?}: {err}")),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(err) => Err(format!("could not read {name}: {err}")),
    }
}

fn selected_cases() -> Result<Vec<Case>, String> {
    let raw = env::var("HONK_BENCH_CASES").unwrap_or_else(|_| "jam,ast,text".to_string());
    let cases = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(Case::parse)
        .collect::<Result<Vec<_>, _>>()?;
    if cases.is_empty() {
        return Err("HONK_BENCH_CASES selected no cases".to_string());
    }
    Ok(cases)
}

fn percentile(sorted: &[Duration], numerator: usize, denominator: usize) -> Duration {
    let rank = (sorted.len() * numerator).div_ceil(denominator);
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs_f64();
    if seconds >= 1.0 {
        format!("{seconds:.3} s")
    } else if seconds >= 0.001 {
        format!("{:.3} ms", seconds * 1_000.0)
    } else {
        format!("{:.3} us", seconds * 1_000_000.0)
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes_f = bytes as f64;
    if bytes_f >= GIB {
        format!("{:.2} GiB", bytes_f / GIB)
    } else if bytes_f >= MIB {
        format!("{:.2} MiB", bytes_f / MIB)
    } else if bytes_f >= KIB {
        format!("{:.2} KiB", bytes_f / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn command_text(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unavailable".to_string())
}

fn fingerprint() -> String {
    let sha = command_text("git", &["rev-parse", "HEAD"]);
    let rustc = command_text("rustc", &["-Vv"]);
    let cpu = if cfg!(target_os = "macos") {
        command_text("sysctl", &["-n", "machdep.cpu.brand_string"])
    } else {
        command_text(
            "sh",
            &["-c", "sed -n 's/^model name[[:space:]]*: //p' /proc/cpuinfo | head -1"],
        )
    };
    let memory = if cfg!(target_os = "macos") {
        command_text("sysctl", &["-n", "hw.memsize"])
            .parse::<u64>()
            .map(format_bytes)
            .unwrap_or_else(|_| "unavailable".to_string())
    } else {
        command_text(
            "sh",
            &["-c", "sed -n 's/^MemTotal:[[:space:]]*//p' /proc/meminfo"],
        )
    };
    format!(
        "git: {sha}\nhost: {} {}\ncpu: {cpu}\nlogical CPUs: {}\nmemory: {memory}\nrustc:\n{rustc}\n",
        env::consts::OS,
        env::consts::ARCH,
        std::thread::available_parallelism()
            .map(|count| count.get().to_string())
            .unwrap_or_else(|_| "unavailable".to_string()),
    )
}

fn parse_child_output(stdout: &str, wall: Duration) -> Result<ChildResult, String> {
    let mut durations = Vec::new();
    let mut output_bytes = None;
    let mut peak_rss_bytes = None;
    for line in stdout.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        match fields.as_slice() {
            ["SAMPLE", nanos, bytes] => {
                durations.push(Duration::from_nanos(
                    nanos
                        .parse::<u64>()
                        .map_err(|err| format!("invalid child duration {nanos:?}: {err}"))?,
                ));
                let bytes = bytes
                    .parse::<u64>()
                    .map_err(|err| format!("invalid child output size {bytes:?}: {err}"))?;
                if let Some(expected) = output_bytes {
                    if expected != bytes {
                        return Err(format!(
                            "child output size changed between samples: {expected} != {bytes}"
                        ));
                    }
                }
                output_bytes = Some(bytes);
            }
            ["RSS", bytes] => {
                peak_rss_bytes = Some(
                    bytes
                        .parse::<u64>()
                        .map_err(|err| format!("invalid child peak RSS {bytes:?}: {err}"))?,
                );
            }
            [] => {}
            _ => return Err(format!("unexpected child output: {line:?}")),
        }
    }
    if durations.is_empty() {
        return Err("child returned no samples".to_string());
    }
    Ok(ChildResult {
        durations,
        output_bytes,
        peak_rss_bytes,
        wall,
        failure: None,
    })
}

fn run_child(case: Case, kernel: &Path, samples: usize, warmups: usize) -> ChildResult {
    let executable = env::current_exe().expect("current benchmark executable");
    eprintln!(
        "running {}: {samples} sample(s), {warmups} warmup(s)",
        case.name()
    );
    let started = Instant::now();
    let output = Command::new(executable)
        .arg("--child")
        .arg(match case {
            Case::Jam => "jam",
            Case::Ast => "ast",
            Case::Text => "text",
        })
        .arg(kernel)
        .arg(samples.to_string())
        .arg(warmups.to_string())
        .output();
    let wall = started.elapsed();
    match output {
        Ok(output) if output.status.success() => {
            match parse_child_output(&String::from_utf8_lossy(&output.stdout), wall) {
                Ok(result) => result,
                Err(err) => ChildResult {
                    durations: Vec::new(),
                    output_bytes: None,
                    peak_rss_bytes: None,
                    wall,
                    failure: Some(err),
                },
            }
        }
        Ok(output) => ChildResult {
            durations: Vec::new(),
            output_bytes: None,
            peak_rss_bytes: None,
            wall,
            failure: Some(format!(
                "child exited with {}{}",
                output.status,
                if output.stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {}", String::from_utf8_lossy(&output.stderr).trim())
                }
            )),
        },
        Err(err) => ChildResult {
            durations: Vec::new(),
            output_bytes: None,
            peak_rss_bytes: None,
            wall,
            failure: Some(format!("could not start child: {err}")),
        },
    }
}

fn summarize(case: Case, input_bytes: u64, mut result: ChildResult) -> String {
    if let Some(failure) = result.failure.take() {
        return format!(
            "{}: FAILED after {} ({failure})",
            case.name(),
            format_duration(result.wall)
        );
    }
    result.durations.sort_unstable();
    let p50 = percentile(&result.durations, 50, 100);
    let p95 = percentile(&result.durations, 95, 100);
    let p99 = percentile(&result.durations, 99, 100);
    let maximum = *result.durations.last().expect("non-empty samples");
    let throughput = input_bytes as f64 / p50.as_secs_f64();
    let output = match (case, result.output_bytes) {
        (Case::Ast, Some(nodes)) => format!("{nodes} nodes"),
        (Case::Ast, None) => "n/a".to_string(),
        (_, Some(bytes)) => format_bytes(bytes),
        (_, None) => "n/a".to_string(),
    };
    let rss = result
        .peak_rss_bytes
        .map(format_bytes)
        .unwrap_or_else(|| "unavailable".to_string());
    format!(
        "{}: n={} p50={} p95={} p99={} max={} logical-throughput={}/s output={} peak-rss={} child-wall={}",
        case.name(),
        result.durations.len(),
        format_duration(p50),
        format_duration(p95),
        format_duration(p99),
        format_duration(maximum),
        format_bytes(throughput as u64),
        output,
        rss,
        format_duration(result.wall),
    )
}

fn parent() -> Result<(), String> {
    let kernel = workspace_path(
        env::var_os("HONK_KERNEL_JAM")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_KERNEL_JAM)),
    );
    if !kernel.is_file() {
        return Err(format!(
            "kernel fixture {} does not exist; run `just honk-nockasm-serialization-bench`",
            kernel.display()
        ));
    }
    let input_bytes = fs::metadata(&kernel)
        .map_err(|err| format!("could not stat {}: {err}", kernel.display()))?
        .len();
    let cases = selected_cases()?;

    let mut report = format!(
        "Honk kernel serialization benchmark\nscenario: prepared, fully compiled kernel noun; cue/setup excluded\nfixture: {}\ninput JAM: {} ({input_bytes} bytes)\n{}",
        kernel.display(),
        format_bytes(input_bytes),
        fingerprint(),
    );
    println!("{report}");

    if cases.iter().any(|case| *case != Case::Jam) {
        let validation = validate_in_child(&kernel)?;
        println!("{validation}");
        report.push_str(&validation);
        report.push('\n');
    }

    for case in cases {
        let suffix = case.env_suffix();
        let samples = env_usize(
            &format!("HONK_BENCH_{suffix}_SAMPLES"),
            case.default_samples(),
        )?;
        if samples == 0 {
            return Err(format!("HONK_BENCH_{suffix}_SAMPLES must be positive"));
        }
        let warmups = env_usize(
            &format!("HONK_BENCH_{suffix}_WARMUPS"),
            case.default_warmups(),
        )?;
        let result = run_child(case, &kernel, samples, warmups);
        let summary = summarize(case, input_bytes, result);
        println!("{summary}");
        report.push_str(&summary);
        report.push('\n');
    }

    if let Some(path) = env::var_os("HONK_BENCH_REPORT") {
        let path = workspace_path(PathBuf::from(path));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("could not create {}: {err}", parent.display()))?;
        }
        fs::write(&path, report)
            .map_err(|err| format!("could not write {}: {err}", path.display()))?;
        println!("report: {}", path.display());
    }
    Ok(())
}

fn validate_in_child(kernel: &Path) -> Result<String, String> {
    let started = Instant::now();
    let output = Command::new(env::current_exe().expect("current benchmark executable"))
        .arg("--validate")
        .arg(kernel)
        .output()
        .map_err(|err| format!("could not start DAG validation child: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "DAG validation failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let details = String::from_utf8_lossy(&output.stdout);
    let details = details.trim();
    if !details.starts_with("validated DAG round trip:") {
        return Err(format!("unexpected DAG validation output: {details:?}"));
    }
    Ok(format!(
        "{details} validation-wall={}",
        format_duration(started.elapsed())
    ))
}

fn peak_rss_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if rc != 0 {
        return 0;
    }
    let rss = unsafe { usage.assume_init() }.ru_maxrss as u64;
    if cfg!(target_os = "macos") {
        rss
    } else {
        rss.saturating_mul(1024)
    }
}

fn run_samples<F>(samples: usize, warmups: usize, mut operation: F)
where
    F: FnMut() -> u64,
{
    for _ in 0..warmups {
        black_box(operation());
    }
    for _ in 0..samples {
        let started = Instant::now();
        let output_bytes = black_box(operation());
        let elapsed = started.elapsed();
        println!("SAMPLE {} {output_bytes}", elapsed.as_nanos());
    }
    println!("RSS {}", peak_rss_bytes());
}

fn child(case: Case, kernel: &Path, samples: usize, warmups: usize) -> Result<(), String> {
    let bytes = fs::read(kernel)
        .map_err(|err| format!("could not read kernel fixture {}: {err}", kernel.display()))?;
    match case {
        Case::Jam => {
            let mut expected = bytes.clone();
            while expected.last() == Some(&0) {
                expected.pop();
            }
            let mut slab = NounSlab::<NockJammer>::new();
            slab.cue_into(Bytes::from(bytes))
                .map_err(|err| format!("NockVM cue failed: {err}"))?;
            let check = slab.jam();
            if check.as_ref() != expected {
                return Err("NockVM JAM did not reproduce the fixture bytes".to_string());
            }
            run_samples(samples, warmups, || {
                let jam = slab.jam();
                let len = jam.len() as u64;
                black_box(jam);
                len
            });
        }
        Case::Ast => {
            let noun = nockasm::cue(&bytes).map_err(|err| format!("Nockasm cue failed: {err}"))?;
            run_samples(samples, warmups, || {
                let dag = nockasm::lift_dag(&noun).expect("DAG node count fits u32");
                let nodes = dag.nodes().len() as u64;
                black_box(&dag);
                drop(dag);
                nodes
            });
        }
        Case::Text => {
            let noun = nockasm::cue(&bytes).map_err(|err| format!("Nockasm cue failed: {err}"))?;
            run_samples(samples, warmups, || {
                let dag = nockasm::lift_dag(&noun).expect("DAG node count fits u32");
                let text = dag.render();
                let len = text.len() as u64;
                black_box(&text);
                drop(text);
                drop(dag);
                len
            });
        }
    }
    Ok(())
}

fn validate(kernel: &Path) -> Result<(), String> {
    let bytes = fs::read(kernel)
        .map_err(|err| format!("could not read kernel fixture {}: {err}", kernel.display()))?;
    let noun = nockasm::cue(&bytes).map_err(|err| format!("Nockasm cue failed: {err}"))?;
    let expected_jam = nockasm::jam(&noun);
    let dag = nockasm::lift_dag(&noun).map_err(|err| format!("Nockasm DAG lift failed: {err}"))?;
    let nodes = dag.nodes().len();
    if nockasm::jam(&dag.lower()) != expected_jam {
        return Err("Nockasm DAG AST did not lower to the kernel noun".to_string());
    }
    let text = dag.render();
    let text_bytes = text.len();
    let parsed = nockasm::parse_dag(&text)
        .map_err(|err| format!("rendered Nockasm DAG text did not parse: {err}"))?;
    if parsed != dag {
        return Err("parsed Nockasm DAG text changed its nodes or root".to_string());
    }
    drop(dag);
    drop(text);
    if nockasm::jam(&parsed.lower()) != expected_jam {
        return Err("parsed Nockasm DAG text did not lower to the kernel noun".to_string());
    }
    println!("validated DAG round trip: nodes={nodes} text-bytes={text_bytes}");
    Ok(())
}

fn main() -> ExitCode {
    let args = env::args_os().collect::<Vec<_>>();
    let result = if args.get(1).is_some_and(|arg| arg == "--validate") {
        if args.len() != 3 {
            Err("internal usage: --validate <kernel.jam>".to_string())
        } else {
            validate(Path::new(&args[2]))
        }
    } else if args.get(1).is_some_and(|arg| arg == "--child") {
        if args.len() != 6 {
            Err("internal usage: --child <case> <kernel.jam> <samples> <warmups>".to_string())
        } else {
            let case = args[2]
                .to_str()
                .ok_or_else(|| "child case is not UTF-8".to_string())
                .and_then(Case::parse);
            let samples = args[4]
                .to_str()
                .ok_or_else(|| "child sample count is not UTF-8".to_string())
                .and_then(|value| {
                    value
                        .parse::<usize>()
                        .map_err(|err| format!("invalid child sample count: {err}"))
                });
            let warmups = args[5]
                .to_str()
                .ok_or_else(|| "child warmup count is not UTF-8".to_string())
                .and_then(|value| {
                    value
                        .parse::<usize>()
                        .map_err(|err| format!("invalid child warmup count: {err}"))
                });
            case.and_then(|case| {
                samples.and_then(|samples| {
                    warmups.and_then(|warmups| child(case, Path::new(&args[3]), samples, warmups))
                })
            })
        }
    } else {
        parent()
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("kernel serialization benchmark: {err}");
            ExitCode::FAILURE
        }
    }
}
