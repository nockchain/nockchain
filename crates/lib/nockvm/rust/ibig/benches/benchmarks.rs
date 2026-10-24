//! Benchmarks.
//!
//! Note: these don't work on 16-bit machines.

#![allow(deprecated)]

use std::fmt::Write;

use criterion::{
    black_box, criterion_group, criterion_main, AxisScale, BenchmarkId, Criterion,
    PlotConfiguration,
};
use ibig::modular::ModuloRing;
use ibig::ops::DivRem;
use ibig::{ubig, UBig};
use rand::prelude::*;

fn random_ubig<R>(bits: usize, rng: &mut R) -> UBig
where
    R: Rng + ?Sized,
{
    rng.gen_range(ubig!(1) << (bits - 1)..ubig!(1) << bits)
}

fn bench_add(criterion: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(1);
    let mut group = criterion.benchmark_group("add");
    group.plot_config(PlotConfiguration::default().summary_scale(AxisScale::Logarithmic));

    for log_bits in 1..=6 {
        let bits = 10usize.pow(log_bits);
        let a = random_ubig(bits, &mut rng);
        let b = random_ubig(bits, &mut rng);
        group.bench_with_input(BenchmarkId::from_parameter(bits), &bits, |bencher, _| {
            bencher.iter(|| black_box(&a) + black_box(&b))
        });
    }

    group.finish();
}

fn bench_sub(criterion: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(1);
    let mut group = criterion.benchmark_group("sub");
    group.plot_config(PlotConfiguration::default().summary_scale(AxisScale::Logarithmic));

    for log_bits in 1..=6 {
        let bits = 10usize.pow(log_bits);
        let a = random_ubig(bits, &mut rng);
        let b = random_ubig(bits, &mut rng);
        let c = a + &b;
        group.bench_with_input(BenchmarkId::from_parameter(bits), &bits, |bencher, _| {
            bencher.iter(|| black_box(&c) - black_box(&b))
        });
    }

    group.finish();
}

fn bench_mul(criterion: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(1);
    let mut group = criterion.benchmark_group("mul");
    group.plot_config(PlotConfiguration::default().summary_scale(AxisScale::Logarithmic));

    for log_bits in 1..=6 {
        let bits = 10usize.pow(log_bits);
        let a = random_ubig(bits, &mut rng);
        let b = random_ubig(bits, &mut rng);
        group.bench_with_input(BenchmarkId::from_parameter(bits), &bits, |bencher, _| {
            bencher.iter(|| black_box(&a) * black_box(&b))
        });
    }

    group.finish();
}

fn bench_div(criterion: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(1);
    let mut group = criterion.benchmark_group("div");
    group.plot_config(PlotConfiguration::default().summary_scale(AxisScale::Logarithmic));

    for log_bits in 1..=6 {
        let bits = 10usize.pow(log_bits);
        let a = random_ubig(2 * bits, &mut rng);
        let b = random_ubig(bits, &mut rng);
        group.bench_with_input(BenchmarkId::from_parameter(bits), &bits, |bencher, _| {
            bencher.iter(|| black_box(&a).div_rem(black_box(&b)))
        });
    }

    group.finish();
}

fn bench_gcd(criterion: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(1);
    let mut group = criterion.benchmark_group("gcd");
    group.plot_config(PlotConfiguration::default().summary_scale(AxisScale::Logarithmic));

    for log_bits in 1..=5 {
        let bits = 10usize.pow(log_bits);
        let a = random_ubig(bits, &mut rng);
        let b = random_ubig(bits, &mut rng);
        group.bench_with_input(BenchmarkId::from_parameter(bits), &bits, |bencher, _| {
            bencher.iter(|| black_box(&a).gcd(black_box(&b)))
        });
    }

    group.finish();

    let mut group = criterion.benchmark_group("extended_gcd");
    group.plot_config(PlotConfiguration::default().summary_scale(AxisScale::Logarithmic));

    for log_bits in 1..=5 {
        let bits = 10usize.pow(log_bits);
        let a = random_ubig(bits, &mut rng);
        let b = random_ubig(bits, &mut rng);
        group.bench_with_input(BenchmarkId::from_parameter(bits), &bits, |bencher, _| {
            bencher.iter(|| black_box(&a).extended_gcd(black_box(&b)))
        });
    }

    group.finish();
}

fn bench_to_hex(criterion: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(1);
    let mut group = criterion.benchmark_group("to_hex");
    group.plot_config(PlotConfiguration::default().summary_scale(AxisScale::Logarithmic));

    for log_bits in 1..=6 {
        let bits = 10usize.pow(log_bits);
        let a = random_ubig(bits, &mut rng);
        let mut out = String::with_capacity(bits / 4 + 1);
        group.bench_with_input(BenchmarkId::from_parameter(bits), &bits, |bencher, _| {
            bencher.iter(|| {
                out.clear();
                write!(&mut out, "{:x}", black_box(&a)).unwrap_or_else(|_| {
                    panic!(
                        "Called `expect()` at {}:{} (git sha: {})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA").unwrap_or("unknown")
                    )
                });
                out.len()
            })
        });
    }

    group.finish();
}

fn bench_to_dec(criterion: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(1);
    let mut group = criterion.benchmark_group("to_dec");
    group.plot_config(PlotConfiguration::default().summary_scale(AxisScale::Logarithmic));

    for log_bits in 1..=6 {
        let bits = 10usize.pow(log_bits);
        let a = random_ubig(bits, &mut rng);
        let mut out = String::with_capacity(bits / 3 + 1);
        group.bench_with_input(BenchmarkId::from_parameter(bits), &bits, |bencher, _| {
            bencher.iter(|| {
                out.clear();
                write!(&mut out, "{}", black_box(&a)).unwrap_or_else(|_| {
                    panic!(
                        "Called `expect()` at {}:{} (git sha: {})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA").unwrap_or("unknown")
                    )
                });
                out.len()
            })
        });
    }

    group.finish();
}

fn bench_from_hex(criterion: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(1);
    let mut group = criterion.benchmark_group("from_hex");
    group.plot_config(PlotConfiguration::default().summary_scale(AxisScale::Logarithmic));

    for log_bits in 1..=6 {
        let bits = 10usize.pow(log_bits);
        let a = random_ubig(bits, &mut rng);
        let s = a.in_radix(16).to_string();
        group.bench_with_input(BenchmarkId::from_parameter(bits), &bits, |bencher, _| {
            bencher.iter(|| UBig::from_str_radix(black_box(&s), 16))
        });
    }

    group.finish();
}

fn bench_from_dec(criterion: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(1);
    let mut group = criterion.benchmark_group("from_dec");
    group.plot_config(PlotConfiguration::default().summary_scale(AxisScale::Logarithmic));

    for log_bits in 1..=6 {
        let bits = 10usize.pow(log_bits);
        let a = random_ubig(bits, &mut rng);
        let s = a.in_radix(10).to_string();
        group.bench_with_input(BenchmarkId::from_parameter(bits), &bits, |bencher, _| {
            bencher.iter(|| UBig::from_str_radix(black_box(&s), 10))
        });
    }

    group.finish();
}

fn bench_pow(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("pow");
    group.plot_config(PlotConfiguration::default().summary_scale(AxisScale::Logarithmic));

    for log_power in 1..=6 {
        let p = 10usize.pow(log_power);
        group.bench_with_input(BenchmarkId::from_parameter(p), &p, |bencher, p| {
            bencher.iter(|| ubig!(3).pow(*p))
        });
    }

    group.finish();
}

fn bench_modulo_mul(criterion: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(1);
    let mut group = criterion.benchmark_group("modulo_mul");
    group.plot_config(PlotConfiguration::default().summary_scale(AxisScale::Logarithmic));

    for log_bits in 1..=6 {
        let bits = 10usize.pow(log_bits);
        let m = random_ubig(bits, &mut rng);
        let ring = ModuloRing::new(&m);
        let a = ring.from(random_ubig(bits, &mut rng));
        let b = ring.from(random_ubig(bits, &mut rng));
        group.bench_with_input(BenchmarkId::from_parameter(bits), &bits, |bencher, _| {
            bencher.iter(|| black_box(&a) * black_box(&b))
        });
    }

    group.finish();
}

fn bench_modulo_pow(criterion: &mut Criterion) {
    let mut rng = StdRng::seed_from_u64(1);
    let mut group = criterion.benchmark_group("modulo_pow");
    group.plot_config(PlotConfiguration::default().summary_scale(AxisScale::Logarithmic));

    for log_bits in 1..=4 {
        if log_bits == 4 {
            group.sample_size(10);
        }
        let bits = 10usize.pow(log_bits);
        let m = random_ubig(bits, &mut rng);
        let ring = ModuloRing::new(&m);
        let a = ring.from(random_ubig(bits, &mut rng));
        let b = random_ubig(bits, &mut rng);
        group.bench_with_input(BenchmarkId::from_parameter(bits), &bits, |bencher, _| {
            bencher.iter(|| black_box(&a).pow(&b))
        });
    }

    group.finish();
}
criterion_group!(
    benches, bench_add, bench_sub, bench_mul, bench_div, bench_gcd, bench_to_hex, bench_to_dec,
    bench_from_hex, bench_from_dec, bench_pow, bench_modulo_mul, bench_modulo_pow,
);

criterion_main!(benches);
