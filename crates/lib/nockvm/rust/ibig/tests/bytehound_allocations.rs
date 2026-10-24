use std::hint::black_box;

use ibig::UBig;

const SMALL_UBIG_COUNT: usize = 300_000;
const SMALL_UBIG_SHIFT_BITS: usize = 80;

const LARGE_UBIG_COUNT: usize = 32;
const LARGE_UBIG_SHIFT_BITS: usize = 1_048_576; // 128 Ki bits per value.

fn make_small_multword_ubig(i: usize) -> UBig {
    // Intentionally use non-_stack operations to exercise global-allocation paths.
    (UBig::from((i + 1) as u64) << SMALL_UBIG_SHIFT_BITS) + ((i & 0xff) as u8)
}

fn make_large_ubig(i: usize, base_shift_bits: usize) -> UBig {
    let shift = base_shift_bits + i;
    (UBig::from(1u8) << shift) + ((i & 0xff) as u8)
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

#[test]
#[ignore = "manual workload for LD_PRELOAD heap tracking"]
fn bytehound_many_small_ubigs() {
    let count = env_usize("IBIG_BYTEHOUND_SMALL_COUNT", SMALL_UBIG_COUNT);
    let mut values = Vec::with_capacity(count);

    for i in 0..count {
        values.push(make_small_multword_ubig(i));
    }

    let checksum = values.iter().fold(0usize, |acc, v| acc ^ v.bit_len());
    black_box(checksum);
    assert_eq!(values.len(), count);
    assert_ne!(checksum, 0);
}

#[test]
#[ignore = "manual workload for LD_PRELOAD heap tracking"]
fn bytehound_few_large_ubigs() {
    let count = env_usize("IBIG_BYTEHOUND_LARGE_COUNT", LARGE_UBIG_COUNT);
    let base_shift_bits = env_usize("IBIG_BYTEHOUND_LARGE_SHIFT_BITS", LARGE_UBIG_SHIFT_BITS);
    let mut values = Vec::with_capacity(count);

    for i in 0..count {
        values.push(make_large_ubig(i, base_shift_bits));
    }

    let checksum = values
        .iter()
        .fold(0usize, |acc, v| acc.wrapping_add(v.bit_len()));
    black_box(checksum);
    assert_eq!(values.len(), count);
    assert!(checksum > base_shift_bits);
}
