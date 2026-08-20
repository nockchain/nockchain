use std::time::Instant;

use ai_pow::matmul::TileState;
use ai_pow::pearl_compat::pearl_jackpot_hash;
use ai_pow_miner::gemma4_cuda::{Gemma4CudaSession, GEMMA4_K, GEMMA4_RANK, GEMMA4_TILE};
use anyhow::{bail, Context, Result};

#[derive(Clone, Copy)]
struct Options {
    device: usize,
    m: usize,
    n: usize,
    iterations: usize,
    warmup_iterations: usize,
}

fn main() -> Result<()> {
    let options = parse_options()?;
    let kernel_info = Gemma4CudaSession::kernel_info(options.device)?;
    let total_tickets = (options.m / GEMMA4_TILE)
        .checked_mul(options.n / GEMMA4_TILE)
        .context("ticket count overflow")?;
    eprintln!(
        "preparing m={} n={} k={} rank={} tickets={}",
        options.m, options.n, GEMMA4_K, GEMMA4_RANK, total_tickets
    );

    let mut random_state = 0x0123_4567_89ab_cdefu64;
    let mut next_value = || {
        random_state ^= random_state << 13;
        random_state ^= random_state >> 7;
        random_state ^= random_state << 17;
        ((random_state >> 32) as u8 & 0x7f) as i8 - 64
    };
    let mut a = Vec::with_capacity(options.m * GEMMA4_K);
    a.resize_with(options.m * GEMMA4_K, &mut next_value);
    let mut b = Vec::with_capacity(options.n * GEMMA4_K);
    b.resize_with(options.n * GEMMA4_K, &mut next_value);
    let key = std::array::from_fn(|index| (index as u8).wrapping_mul(17).wrapping_add(3));

    let prepare_started = Instant::now();
    let mut session = Gemma4CudaSession::new(options.device, options.m, options.n, &a, &b, &key)?;
    let prepare_ms = prepare_started.elapsed().as_secs_f64() * 1_000.0;

    let check_ordinals = [0, (total_tickets / 2) as u64, total_tickets as u64 - 1];
    for ordinal in check_ordinals {
        let device = session.debug_ticket(ordinal)?;
        let scalar = scalar_ticket(&a, &b, options.n, ordinal);
        if device.state != scalar {
            bail!("device transcript differs from scalar oracle at ordinal {ordinal}");
        }
        if device.jackpot != pearl_jackpot_hash(&scalar, &key) {
            bail!("device jackpot differs from scalar oracle at ordinal {ordinal}");
        }
    }

    let target = [0u8; 32];
    for _ in 0..options.warmup_iterations {
        let warmup = session.search(0, session.total_tickets(), &target)?;
        if warmup.winner.is_some() {
            bail!("zero-target warmup returned a winner");
        }
    }

    let mut kernel_ms = Vec::with_capacity(options.iterations);
    let mut wall_ms = Vec::with_capacity(options.iterations);
    for _ in 0..options.iterations {
        let started = Instant::now();
        let result = session.search(0, session.total_tickets(), &target)?;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        if result.winner.is_some() {
            bail!("zero-target benchmark returned a winner");
        }
        kernel_ms.push(f64::from(result.kernel_ms));
        wall_ms.push(elapsed_ms);
    }
    kernel_ms.sort_by(f64::total_cmp);
    wall_ms.sort_by(f64::total_cmp);
    let median_kernel_ms = median(&kernel_ms);
    let median_wall_ms = median(&wall_ms);
    let tickets_per_second = total_tickets as f64 * 1_000.0 / median_kernel_ms;
    println!("device\tsms\tthreads_per_cta\tactive_ctas_per_sm\tregisters_per_thread\tstatic_shared_bytes\tdynamic_shared_bytes\tm\tn\tk\trank\ttickets\tprepare_ms\tkernel_min_ms\tkernel_median_ms\tkernel_max_ms\twall_median_ms\ttickets_per_s\ttmac_per_s");
    let tmac_per_second =
        tickets_per_second * (GEMMA4_TILE * GEMMA4_TILE * GEMMA4_K) as f64 / 1.0e12;
    println!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.6}",
        options.device,
        kernel_info.sm_count,
        kernel_info.threads_per_cta,
        kernel_info.active_ctas_per_sm,
        kernel_info.registers_per_thread,
        kernel_info.static_shared_bytes,
        kernel_info.dynamic_shared_bytes,
        options.m,
        options.n,
        GEMMA4_K,
        GEMMA4_RANK,
        total_tickets,
        prepare_ms,
        kernel_ms[0],
        median_kernel_ms,
        kernel_ms[kernel_ms.len() - 1],
        median_wall_ms,
        tickets_per_second,
        tmac_per_second,
    );
    Ok(())
}

fn parse_options() -> Result<Options> {
    let mut options = Options {
        device: 0,
        m: 4096,
        n: 43008,
        iterations: 7,
        warmup_iterations: 100,
    };
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        let value = match argument.as_str() {
            "--help" | "-h" => {
                println!("Usage: ai_pow_gemma4_bench [--device N] [--m N] [--n N] [--warmup-iterations N] [--iterations N]");
                return Ok(options);
            }
            "--device" | "--m" | "--n" | "--warmup-iterations" | "--iterations" => args
                .next()
                .with_context(|| format!("{argument} requires a value"))?,
            _ => bail!("unknown argument {argument}"),
        };
        match argument.as_str() {
            "--device" => options.device = value.parse().context("invalid --device")?,
            "--m" => options.m = value.parse().context("invalid --m")?,
            "--n" => options.n = value.parse().context("invalid --n")?,
            "--warmup-iterations" => {
                options.warmup_iterations = value.parse().context("invalid --warmup-iterations")?
            }
            "--iterations" => options.iterations = value.parse().context("invalid --iterations")?,
            _ => unreachable!(),
        }
    }
    if options.iterations == 0 {
        bail!("--iterations must be nonzero");
    }
    if options.m == 0 || options.m % 256 != 0 || options.n == 0 || options.n % 128 != 0 {
        bail!("shape requires nonzero m%256==0 and n%128==0");
    }
    Ok(options)
}

fn scalar_ticket(a: &[i8], b: &[i8], n: usize, ordinal: u64) -> TileState {
    let col_tiles = n / GEMMA4_TILE;
    let row_tile = ordinal as usize / col_tiles;
    let col_tile = ordinal as usize % col_tiles;
    let mut cells = [0i32; GEMMA4_TILE * GEMMA4_TILE];
    let mut state = TileState::zero();
    for step in 0..GEMMA4_K / GEMMA4_RANK {
        for row in 0..GEMMA4_TILE {
            let a_base = (row_tile * GEMMA4_TILE + row) * GEMMA4_K + step * GEMMA4_RANK;
            for col in 0..GEMMA4_TILE {
                let b_base = (col_tile * GEMMA4_TILE + col) * GEMMA4_K + step * GEMMA4_RANK;
                let mut delta = 0i32;
                for index in 0..GEMMA4_RANK {
                    delta += i32::from(a[a_base + index]) * i32::from(b[b_base + index]);
                }
                let cell = row * GEMMA4_TILE + col;
                cells[cell] = cells[cell].saturating_add(delta);
            }
        }
        let x = cells
            .iter()
            .fold(0u32, |value, cell| value ^ (*cell as u32)) as i32;
        state.fold(step as u32, x);
    }
    state
}

fn median(values: &[f64]) -> f64 {
    if values.len() % 2 == 0 {
        (values[values.len() / 2 - 1] + values[values.len() / 2]) * 0.5
    } else {
        values[values.len() / 2]
    }
}
