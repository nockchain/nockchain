//! Monte-Carlo sanity check that the empirical rejection rate of a malicious
//! prover (who produces commitments with an `f`-fraction of bogus tile
//! leaves) tracks the theoretical bound `1 - (1 - f)^sigma`.
//!
//! The model: an attacker picks a random `f * num_tiles` subset of tiles to
//! lie about, builds a `comm_m` that depends on the lies (so the FS-derived
//! sample indices are uncorrelated with the lie set), and the verifier
//! samples `sigma` tile indices uniformly. The attack is detected iff any
//! sampled index belongs to the lie set.
//!
//! We don't need to run the matmul or build a real Merkle tree to model
//! detection probability — only the sampling overlap matters. We do use the
//! real Fiat-Shamir index derivation to confirm its sampling is uniform
//! enough to match the binomial bound.

use std::collections::HashSet;

use ai_pow::fiat_shamir::challenge_indices;

/// A tiny seeded LCG so the simulation is deterministic.
struct Lcg(u64);
impl Lcg {
    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
    fn pick_subset(&mut self, n: u32, k: u32) -> HashSet<u32> {
        let mut s = HashSet::new();
        while s.len() < k as usize {
            let r = (self.next_u64() % n as u64) as u32;
            s.insert(r);
        }
        s
    }
}

fn empirical_rejection_rate(num_tiles: u32, f: f64, sigma: u32, trials: u32, seed: u64) -> f64 {
    let mut lcg = Lcg(seed);
    let mut detected = 0u32;
    let n_lies = (f * num_tiles as f64).round() as u32;
    for trial in 0..trials {
        let lies = lcg.pick_subset(num_tiles, n_lies);
        // Use a per-trial seed for FS sampling — independent of the lie set.
        let mut s = [0u8; 32];
        s[..8].copy_from_slice(&(trial as u64).to_le_bytes());
        s[8..16].copy_from_slice(&seed.to_le_bytes());
        let indices = challenge_indices(&s, sigma, u64::from(num_tiles));
        if indices.iter().any(|i| lies.contains(&(*i as u32))) {
            detected += 1;
        }
    }
    detected as f64 / trials as f64
}

fn theoretical(f: f64, sigma: u32) -> f64 {
    1.0 - (1.0 - f).powi(sigma as i32)
}

#[test]
fn rejection_rate_matches_theory() {
    let num_tiles = 1024u32;
    let trials = 5_000u32;
    let sigma = 16u32;
    let seed = 0xCAFEBABE_u64;
    for &f in &[0.01f64, 0.05, 0.1, 0.5] {
        let emp = empirical_rejection_rate(num_tiles, f, sigma, trials, seed);
        let th = theoretical(f, sigma);
        // 95% CI on a Bernoulli proportion p with n trials is ~1.96*sqrt(p(1-p)/n).
        let ci = 1.96 * (th * (1.0 - th) / trials as f64).sqrt();
        // Allow a bit of slack beyond the CI to absorb sampling-without-
        // replacement bias and our LCG approximation of the lie-set draw.
        let tol = ci + 0.01;
        assert!(
            (emp - th).abs() < tol,
            "f={f} sigma={sigma}: emp={emp:.4} th={th:.4} tol={tol:.4}",
        );
    }
}

#[test]
fn larger_sigma_drives_rejection_to_one() {
    // At f=0.5, sigma=20 puts theoretical detection above 0.999999.
    let num_tiles = 1024u32;
    let trials = 2_000u32;
    let sigma = 20u32;
    let f = 0.5f64;
    let emp = empirical_rejection_rate(num_tiles, f, sigma, trials, 0xDEADBEEF);
    assert!(emp >= 0.99, "expected near-1 rejection, got {emp}");
}

#[test]
fn fs_sample_indices_distribute_uniformly() {
    // Histogram check: over many seeds, every tile index should be picked
    // approximately equal number of times by the FS index derivation.
    let num_tiles = 64u32;
    let sigma = 16u32;
    let trials = 20_000u32;
    let mut hist = vec![0u32; num_tiles as usize];
    for trial in 0..trials {
        let mut s = [0u8; 32];
        s[..8].copy_from_slice(&(trial as u64).to_le_bytes());
        for &i in &challenge_indices(&s, sigma, u64::from(num_tiles)) {
            hist[i as usize] += 1;
        }
    }
    let expected = (trials * sigma) as f64 / num_tiles as f64;
    let mut max_dev = 0.0f64;
    for &h in &hist {
        let dev = ((h as f64) - expected).abs() / expected;
        if dev > max_dev {
            max_dev = dev;
        }
    }
    // 95% CI on per-bucket count is ~1.96 * sqrt(expected); relative ~ ~0.06
    // for expected=5000. Allow generous slack.
    assert!(max_dev < 0.10, "max relative deviation was {max_dev}");
}
