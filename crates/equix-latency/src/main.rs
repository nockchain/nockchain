use std::error::Error;
use std::time::{Duration, Instant};
use std::{env, fmt};

const DEFAULT_SAMPLES: usize = 100;
const DEFAULT_CHALLENGE_BYTES: usize = 65_536;
const DEFAULT_SEED: u64 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuntimeChoice {
    Default,
    CompileOnly,
    InterpretOnly,
}

impl RuntimeChoice {
    fn apply(self, builder: &mut equix::EquiXBuilder) {
        match self {
            Self::Default => {}
            Self::CompileOnly => {
                builder.runtime(equix::RuntimeOption::CompileOnly);
            }
            Self::InterpretOnly => {
                builder.runtime(equix::RuntimeOption::InterpretOnly);
            }
        }
    }
}

impl fmt::Display for RuntimeChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => f.write_str("default"),
            Self::CompileOnly => f.write_str("compile-only"),
            Self::InterpretOnly => f.write_str("interpret-only"),
        }
    }
}

#[derive(Debug)]
struct Args {
    samples: usize,
    challenge_bytes: usize,
    seed: u64,
    runtime: RuntimeChoice,
    json: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            samples: DEFAULT_SAMPLES,
            challenge_bytes: DEFAULT_CHALLENGE_BYTES,
            seed: DEFAULT_SEED,
            runtime: RuntimeChoice::Default,
            json: false,
        }
    }
}

#[derive(Debug)]
struct Sample {
    index: usize,
    attempts: u64,
    solve: Duration,
    verify: Duration,
    solutions: usize,
}

#[derive(Debug)]
struct Summary {
    samples: usize,
    challenge_bytes: usize,
    seed: u64,
    runtime: RuntimeChoice,
    solve: DurationStats,
    verify: DurationStats,
    attempts: U64Stats,
    max_solutions: usize,
}

#[derive(Debug)]
struct DurationStats {
    min_ns: u128,
    p50_ns: u128,
    p95_ns: u128,
    max_ns: u128,
    mean_ns: u128,
}

#[derive(Debug)]
struct U64Stats {
    min: u64,
    p50: u64,
    p95: u64,
    max: u64,
    mean: u64,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args(env::args().skip(1))?;
    let samples = run(&args)?;
    let summary = summarize(&samples, &args)?;
    print_summary(&summary, &samples, args.json);
    Ok(())
}

fn parse_args<I>(mut args: I) -> Result<Args, Box<dyn Error>>
where
    I: Iterator<Item = String>,
{
    let mut parsed = Args::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--samples" => {
                let Some(value) = args.next() else {
                    return Err("missing value for --samples".into());
                };
                parsed.samples = parse_nonzero_usize("--samples", &value)?;
            }
            "--challenge-bytes" => {
                let Some(value) = args.next() else {
                    return Err("missing value for --challenge-bytes".into());
                };
                parsed.challenge_bytes = parse_nonzero_usize("--challenge-bytes", &value)?;
            }
            "--seed" => {
                let Some(value) = args.next() else {
                    return Err("missing value for --seed".into());
                };
                parsed.seed = value
                    .parse::<u64>()
                    .map_err(|err| format!("invalid --seed value {value:?}: {err}"))?;
            }
            "--runtime" => {
                let Some(value) = args.next() else {
                    return Err("missing value for --runtime".into());
                };
                parsed.runtime = match value.as_str() {
                    "default" => RuntimeChoice::Default,
                    "compile-only" => RuntimeChoice::CompileOnly,
                    "interpret-only" => RuntimeChoice::InterpretOnly,
                    _ => {
                        return Err(format!(
                            "invalid --runtime value {value:?}; expected default, compile-only, or interpret-only"
                        )
                        .into());
                    }
                };
            }
            "--json" => parsed.json = true,
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {arg:?}").into()),
        }
    }
    Ok(parsed)
}

fn parse_nonzero_usize(name: &str, value: &str) -> Result<usize, Box<dyn Error>> {
    let parsed = value
        .parse::<usize>()
        .map_err(|err| format!("invalid {name} value {value:?}: {err}"))?;
    if parsed == 0 {
        return Err(format!("{name} must be greater than zero").into());
    }
    Ok(parsed)
}

fn print_help() {
    println!(
        "Usage: equix-latency [--samples N] [--challenge-bytes N] [--seed N] [--runtime default|compile-only|interpret-only] [--json]"
    );
}

fn run(args: &Args) -> Result<Vec<Sample>, Box<dyn Error>> {
    let mut samples = Vec::with_capacity(args.samples);
    let mut challenge = vec![0u8; args.challenge_bytes];
    let mut builder = equix::EquiXBuilder::new();
    args.runtime.apply(&mut builder);

    for index in 0..args.samples {
        let sample_index =
            u64::try_from(index).map_err(|err| format!("sample index overflowed: {err}"))?;
        let mut attempts = 0u64;
        let solve_start = Instant::now();
        let solutions = loop {
            attempts = attempts
                .checked_add(1)
                .ok_or("attempt counter overflowed while solving")?;
            fill_challenge(&mut challenge, args.seed, sample_index, attempts);
            match builder.solve(&challenge) {
                Ok(solutions) if !solutions.is_empty() => break solutions,
                Ok(_) => {}
                Err(equix::Error::Hash(equix::HashError::ProgramConstraints)) => {}
                Err(err) => {
                    return Err(format!(
                        "EquiX solve failed for sample {index}, attempt {attempts}: {err}"
                    )
                    .into());
                }
            }
        };
        let solve = solve_start.elapsed();
        let verify_start = Instant::now();
        let Some(solution) = solutions.first() else {
            return Err(format!("EquiX solve returned no solution for sample {index}").into());
        };
        builder
            .verify(&challenge, solution)
            .map_err(|err| format!("EquiX verify failed for sample {index}: {err}"))?;
        let verify = verify_start.elapsed();
        samples.push(Sample {
            index,
            attempts,
            solve,
            verify,
            solutions: solutions.len(),
        });
    }
    Ok(samples)
}

fn fill_challenge(out: &mut [u8], seed: u64, sample: u64, attempt: u64) {
    let mut state = seed ^ (sample.wrapping_mul(0x9e37_79b9_7f4a_7c15)) ^ attempt.rotate_left(17);
    for chunk in out.chunks_mut(8) {
        state = splitmix64(state);
        let bytes = state.to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
}

fn splitmix64(mut state: u64) -> u64 {
    state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn summarize(samples: &[Sample], args: &Args) -> Result<Summary, Box<dyn Error>> {
    let solve_values = samples
        .iter()
        .map(|sample| sample.solve.as_nanos())
        .collect();
    let verify_values = samples
        .iter()
        .map(|sample| sample.verify.as_nanos())
        .collect();
    let attempt_values = samples.iter().map(|sample| sample.attempts).collect();
    let max_solutions = samples
        .iter()
        .map(|sample| sample.solutions)
        .max()
        .ok_or("missing benchmark samples")?;
    Ok(Summary {
        samples: samples.len(),
        challenge_bytes: args.challenge_bytes,
        seed: args.seed,
        runtime: args.runtime,
        solve: duration_stats(solve_values)?,
        verify: duration_stats(verify_values)?,
        attempts: u64_stats(attempt_values)?,
        max_solutions,
    })
}

fn duration_stats(mut values: Vec<u128>) -> Result<DurationStats, Box<dyn Error>> {
    values.sort_unstable();
    let total = values
        .iter()
        .try_fold(0u128, |acc, value| acc.checked_add(*value))
        .ok_or("duration total overflowed")?;
    let count =
        u128::try_from(values.len()).map_err(|err| format!("sample count overflowed: {err}"))?;
    let mean = total
        .checked_div(count)
        .ok_or("duration sample count was zero")?;
    Ok(DurationStats {
        min_ns: percentile_u128(&values, 0, 1)?,
        p50_ns: percentile_u128(&values, 50, 100)?,
        p95_ns: percentile_u128(&values, 95, 100)?,
        max_ns: percentile_u128(&values, 1, 1)?,
        mean_ns: mean,
    })
}

fn u64_stats(mut values: Vec<u64>) -> Result<U64Stats, Box<dyn Error>> {
    values.sort_unstable();
    let total = values
        .iter()
        .try_fold(0u64, |acc, value| acc.checked_add(*value))
        .ok_or("attempt total overflowed")?;
    let count =
        u64::try_from(values.len()).map_err(|err| format!("sample count overflowed: {err}"))?;
    let mean = total
        .checked_div(count)
        .ok_or("attempt sample count was zero")?;
    Ok(U64Stats {
        min: percentile_u64(&values, 0, 1)?,
        p50: percentile_u64(&values, 50, 100)?,
        p95: percentile_u64(&values, 95, 100)?,
        max: percentile_u64(&values, 1, 1)?,
        mean,
    })
}

fn percentile_u128(
    values: &[u128],
    numerator: usize,
    denominator: usize,
) -> Result<u128, Box<dyn Error>> {
    if values.is_empty() {
        return Err("cannot compute percentile over empty values".into());
    }
    if denominator == 0 {
        return Err("percentile denominator must be nonzero".into());
    }
    let index = (values.len() - 1)
        .checked_mul(numerator)
        .ok_or("percentile index overflowed")?
        / denominator;
    Ok(values[index])
}

fn percentile_u64(
    values: &[u64],
    numerator: usize,
    denominator: usize,
) -> Result<u64, Box<dyn Error>> {
    if values.is_empty() {
        return Err("cannot compute percentile over empty values".into());
    }
    if denominator == 0 {
        return Err("percentile denominator must be nonzero".into());
    }
    let index = (values.len() - 1)
        .checked_mul(numerator)
        .ok_or("percentile index overflowed")?
        / denominator;
    Ok(values[index])
}

fn print_summary(summary: &Summary, samples: &[Sample], json: bool) {
    if json {
        print_json(summary, samples);
    } else {
        print_text(summary);
    }
}

fn print_text(summary: &Summary) {
    println!("EquiX latency benchmark");
    println!("samples: {}", summary.samples);
    println!("challenge_bytes: {}", summary.challenge_bytes);
    println!("seed: {}", summary.seed);
    println!("runtime: {}", summary.runtime);
    println!("max_solutions_per_attempt: {}", summary.max_solutions);
    println!(
        "solve_ns min={} p50={} p95={} max={} mean={}",
        summary.solve.min_ns,
        summary.solve.p50_ns,
        summary.solve.p95_ns,
        summary.solve.max_ns,
        summary.solve.mean_ns
    );
    println!(
        "verify_ns min={} p50={} p95={} max={} mean={}",
        summary.verify.min_ns,
        summary.verify.p50_ns,
        summary.verify.p95_ns,
        summary.verify.max_ns,
        summary.verify.mean_ns
    );
    println!(
        "attempts min={} p50={} p95={} max={} mean={}",
        summary.attempts.min,
        summary.attempts.p50,
        summary.attempts.p95,
        summary.attempts.max,
        summary.attempts.mean
    );
}

fn print_json(summary: &Summary, samples: &[Sample]) {
    println!("{{");
    println!("  \"samples\": {},", summary.samples);
    println!("  \"challenge_bytes\": {},", summary.challenge_bytes);
    println!("  \"seed\": {},", summary.seed);
    println!("  \"runtime\": \"{}\",", summary.runtime);
    println!(
        "  \"max_solutions_per_attempt\": {},",
        summary.max_solutions
    );
    println!(
        "  \"solve_ns\": {{\"min\": {}, \"p50\": {}, \"p95\": {}, \"max\": {}, \"mean\": {}}},",
        summary.solve.min_ns,
        summary.solve.p50_ns,
        summary.solve.p95_ns,
        summary.solve.max_ns,
        summary.solve.mean_ns
    );
    println!(
        "  \"verify_ns\": {{\"min\": {}, \"p50\": {}, \"p95\": {}, \"max\": {}, \"mean\": {}}},",
        summary.verify.min_ns,
        summary.verify.p50_ns,
        summary.verify.p95_ns,
        summary.verify.max_ns,
        summary.verify.mean_ns
    );
    println!(
        "  \"attempts\": {{\"min\": {}, \"p50\": {}, \"p95\": {}, \"max\": {}, \"mean\": {}}},",
        summary.attempts.min,
        summary.attempts.p50,
        summary.attempts.p95,
        summary.attempts.max,
        summary.attempts.mean
    );
    println!("  \"per_sample\": [");
    for (index, sample) in samples.iter().enumerate() {
        let suffix = if index + 1 == samples.len() { "" } else { "," };
        println!(
            "    {{\"index\": {}, \"attempts\": {}, \"solve_ns\": {}, \"verify_ns\": {}, \"solutions\": {}}}{}",
            sample.index,
            sample.attempts,
            sample.solve.as_nanos(),
            sample.verify.as_nanos(),
            sample.solutions,
            suffix
        );
    }
    println!("  ]");
    println!("}}");
}
