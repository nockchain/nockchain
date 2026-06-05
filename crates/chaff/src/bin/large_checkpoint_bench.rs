// Manual one-shot benchmark for large checkpoints
// Run with: cargo run --release --bin large_checkpoint_bench
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::time::Instant;

use bincode::config::{self, Configuration};
use bincode::Decode;
use bytes::Bytes;
use chaff::Chaff;
use nockapp::nockapp::save::{JammedCheckpointV1, JammedCheckpointV2, JAM_MAGIC_BYTES};
use nockapp::noun::slab::{slab_noun_equality, NockJammer, NounSlab};

const SNAPSHOT_VERSION_2: u32 = 2;

#[derive(Decode)]
struct CheckpointEnvelope {
    magic_bytes: u64,
    version: u32,
    payload: Vec<u8>,
}

fn extract_jammed_state(bytes: &[u8]) -> Bytes {
    let config = config::standard();

    // Try to decode as envelope format (V2)
    if let Ok((envelope, _)) =
        bincode::decode_from_slice::<CheckpointEnvelope, Configuration>(bytes, config)
    {
        if envelope.magic_bytes == JAM_MAGIC_BYTES && envelope.version == SNAPSHOT_VERSION_2 {
            let (checkpoint, _) = bincode::decode_from_slice::<JammedCheckpointV2, Configuration>(
                &envelope.payload, config,
            )
            .expect("V2 checkpoint payload should decode");
            return checkpoint.state_jam.0;
        }
    }

    // Try to decode as V1 (non-envelope format)
    if let Ok((checkpoint, _)) =
        bincode::decode_from_slice::<JammedCheckpointV1, Configuration>(bytes, config)
    {
        if checkpoint.magic_bytes == JAM_MAGIC_BYTES {
            return checkpoint.jam.0;
        }
    }

    panic!("Failed to decode checkpoint as either V1 or V2 format");
}

fn main() {
    // Get checkpoint path from args or use default
    let checkpoint_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("large.chkjam"));

    let jammed_state = if checkpoint_path.is_file() {
        println!("Loading checkpoint from: {}", checkpoint_path.display());

        let load_start = Instant::now();
        let checkpoint_bytes =
            std::fs::read(&checkpoint_path).expect("Failed to read checkpoint file");
        let load_time = load_start.elapsed();
        println!(
            "Loaded {} bytes ({:.2} GB) in {:.2}s",
            checkpoint_bytes.len(),
            checkpoint_bytes.len() as f64 / (1024.0 * 1024.0 * 1024.0),
            load_time.as_secs_f64()
        );

        let extract_start = Instant::now();
        let jammed_state = extract_jammed_state(&checkpoint_bytes);
        let extract_time = extract_start.elapsed();
        println!(
            "Extracted jammed state: {} bytes ({:.2} GB) in {:.2}s",
            jammed_state.len(),
            jammed_state.len() as f64 / (1024.0 * 1024.0 * 1024.0),
            extract_time.as_secs_f64()
        );

        // Drop original bytes to free memory
        drop(checkpoint_bytes);
        jammed_state
    } else {
        println!(
            "Warning: checkpoint {} not found; using generated fixture instead",
            checkpoint_path.display()
        );
        let mut slab = NounSlab::<NockJammer>::new();
        let fixture = nockvm::noun::T(&mut slab, &[nockvm::noun::D(1); 12]);
        slab.set_root(fixture);
        let jammed = slab.jam();
        println!(
            "Generated fixture jam size: {} bytes ({:.2} MB)",
            jammed.len(),
            jammed.len() as f64 / (1024.0 * 1024.0)
        );
        jammed
    };

    println!("\n=== CUE BENCHMARKS ===\n");

    // Benchmark NockJammer cue
    println!("Cueing with NockJammer (bitvec)...");
    let cue_start = Instant::now();
    let mut nock_slab = NounSlab::<NockJammer>::new();
    let nock_noun = nock_slab
        .cue_into(jammed_state.clone())
        .expect("NockJammer cue failed");
    nock_slab.set_root(nock_noun);
    let nock_cue_time = cue_start.elapsed();
    println!(
        "NockJammer cue: {:.2}s ({:.2} MB/s)",
        nock_cue_time.as_secs_f64(),
        (jammed_state.len() as f64 / (1024.0 * 1024.0)) / nock_cue_time.as_secs_f64()
    );

    // Benchmark Chaff cue
    println!("\nCueing with Chaff (BitReader)...");
    let cue_start = Instant::now();
    let mut chaff_slab = NounSlab::<Chaff>::new();
    let chaff_noun = chaff_slab
        .cue_into(jammed_state.clone())
        .expect("Chaff cue failed");
    chaff_slab.set_root(chaff_noun);
    let chaff_cue_time = cue_start.elapsed();
    println!(
        "Chaff cue: {:.2}s ({:.2} MB/s)",
        chaff_cue_time.as_secs_f64(),
        (jammed_state.len() as f64 / (1024.0 * 1024.0)) / chaff_cue_time.as_secs_f64()
    );

    let cue_speedup = nock_cue_time.as_secs_f64() / chaff_cue_time.as_secs_f64();
    println!("\nCue speedup: {:.2}x", cue_speedup);

    // ========================================
    // COMPREHENSIVE VERIFICATION - Part 1: Noun Equality
    // (Must do this BEFORE jam benchmarks consume the slabs)
    // ========================================
    println!("\n=== COMPREHENSIVE VERIFICATION ===\n");

    println!("--- Part 1: Noun Equality (Cue Verification) ---\n");
    println!("Comparing cued nouns using slab_noun_equality...");
    let noun_eq_start = Instant::now();
    let nouns_equal = slab_noun_equality(unsafe { nock_slab.root() }, unsafe { chaff_slab.root() });
    let noun_eq_time = noun_eq_start.elapsed();
    if nouns_equal {
        println!(
            "✓ Cued nouns are structurally equal (verified in {:.2}s)",
            noun_eq_time.as_secs_f64()
        );
    } else {
        println!("✗ Cued nouns are NOT equal!");
    }

    // Clone slabs for the 4x4 jam matrix (coerce_jammer consumes the slab)
    let nock_slab_for_nock_jam = nock_slab.clone();
    let nock_slab_for_chaff_jam = nock_slab.clone();
    let chaff_slab_for_nock_jam = chaff_slab.clone();
    let chaff_slab_for_chaff_jam = chaff_slab.clone();

    // Keep one original for round-trip verification
    let nock_slab_original = nock_slab;

    println!("\n=== JAM BENCHMARKS ===\n");

    // Benchmark NockJammer jam (using nock-cued slab)
    println!("Jamming with NockJammer (bitvec)...");
    let jam_start = Instant::now();
    let nock_nock_jammed = nock_slab_for_nock_jam.coerce_jammer::<NockJammer>().jam();
    let nock_jam_time = jam_start.elapsed();
    println!(
        "NockJammer jam: {:.2}s ({:.2} MB/s)",
        nock_jam_time.as_secs_f64(),
        (nock_nock_jammed.len() as f64 / (1024.0 * 1024.0)) / nock_jam_time.as_secs_f64()
    );

    // Benchmark Chaff jam (using chaff-cued slab)
    println!("\nJamming with Chaff (BitWriter)...");
    let jam_start = Instant::now();
    let chaff_chaff_jammed = chaff_slab_for_chaff_jam.coerce_jammer::<Chaff>().jam();
    let chaff_jam_time = jam_start.elapsed();
    println!(
        "Chaff jam: {:.2}s ({:.2} MB/s)",
        chaff_jam_time.as_secs_f64(),
        (chaff_chaff_jammed.len() as f64 / (1024.0 * 1024.0)) / chaff_jam_time.as_secs_f64()
    );

    let jam_speedup = nock_jam_time.as_secs_f64() / chaff_jam_time.as_secs_f64();
    println!("\nJam speedup: {:.2}x", jam_speedup);

    // --- Part 2: 4x4 Jam Matrix (byte equality verification) ---
    println!("\n--- Part 2: 4x4 Jam Matrix (Byte Equality) ---\n");

    // Compute the remaining cross-jams:
    // - nock_chaff_jammed: nock-cued slab -> jammed with Chaff
    // - chaff_nock_jammed: chaff-cued slab -> jammed with NockJammer

    println!("Computing cross-jams for 4x4 matrix...");

    println!("  [NockJammer cue -> Chaff jam]...");
    let cross_start = Instant::now();
    let nock_chaff_jammed = nock_slab_for_chaff_jam.coerce_jammer::<Chaff>().jam();
    println!(
        "    Done in {:.2}s ({} bytes)",
        cross_start.elapsed().as_secs_f64(),
        nock_chaff_jammed.len()
    );

    println!("  [Chaff cue -> NockJammer jam]...");
    let cross_start = Instant::now();
    let chaff_nock_jammed = chaff_slab_for_nock_jam.coerce_jammer::<NockJammer>().jam();
    println!(
        "    Done in {:.2}s ({} bytes)",
        cross_start.elapsed().as_secs_f64(),
        chaff_nock_jammed.len()
    );

    println!("\n  Jam output sizes:");
    println!(
        "    [NockJammer cue -> NockJammer jam] (NN): {} bytes",
        nock_nock_jammed.len()
    );
    println!(
        "    [NockJammer cue -> Chaff jam]      (NC): {} bytes",
        nock_chaff_jammed.len()
    );
    println!(
        "    [Chaff cue -> NockJammer jam]      (CN): {} bytes",
        chaff_nock_jammed.len()
    );
    println!(
        "    [Chaff cue -> Chaff jam]           (CC): {} bytes",
        chaff_chaff_jammed.len()
    );

    // Helper to check byte equality
    fn check_byte_eq(name: &str, a: &[u8], b: &[u8]) -> bool {
        if a == b {
            println!("  ✓ {}: exact match ({} bytes)", name, a.len());
            true
        } else if a.len() != b.len() {
            println!(
                "  ✗ {}: length mismatch ({} vs {} bytes)",
                name,
                a.len(),
                b.len()
            );
            false
        } else {
            // Find first difference
            for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
                if x != y {
                    println!(
                        "  ✗ {}: content differs at byte {} (0x{:02x} vs 0x{:02x})",
                        name, i, x, y
                    );
                    return false;
                }
            }
            unreachable!()
        }
    }

    println!("\n  Pairwise comparisons (6 pairs from 4 outputs):");

    // All 6 pairwise comparisons (C(4,2) = 6)
    let mut all_match = true;
    all_match &= check_byte_eq("NN vs NC", &nock_nock_jammed, &nock_chaff_jammed);
    all_match &= check_byte_eq("NN vs CN", &nock_nock_jammed, &chaff_nock_jammed);
    all_match &= check_byte_eq("NN vs CC", &nock_nock_jammed, &chaff_chaff_jammed);
    all_match &= check_byte_eq("NC vs CN", &nock_chaff_jammed, &chaff_nock_jammed);
    all_match &= check_byte_eq("NC vs CC", &nock_chaff_jammed, &chaff_chaff_jammed);
    all_match &= check_byte_eq("CN vs CC", &chaff_nock_jammed, &chaff_chaff_jammed);

    // --- Part 3: Cross-verification (re-cue jammed outputs) ---
    println!("\n--- Part 3: Round-trip Verification ---\n");

    // Re-cue the chaff-jammed output with NockJammer and compare
    println!("Re-cueing Chaff jam output (CC) with NockJammer...");
    let recue_start = Instant::now();
    let mut recue_slab = NounSlab::<NockJammer>::new();
    let recue_noun = recue_slab
        .cue_into(chaff_chaff_jammed.clone())
        .expect("Re-cue should succeed");
    recue_slab.set_root(recue_noun);
    println!("  Done in {:.2}s", recue_start.elapsed().as_secs_f64());

    // Compare re-cued noun with original nock-cued noun
    println!("Comparing re-cued noun with original NockJammer-cued noun...");
    let roundtrip_eq_start = Instant::now();
    let roundtrip_equal = slab_noun_equality(unsafe { nock_slab_original.root() }, unsafe {
        recue_slab.root()
    });
    let roundtrip_eq_time = roundtrip_eq_start.elapsed();
    if roundtrip_equal {
        println!(
            "✓ Round-trip noun equality verified (in {:.2}s)",
            roundtrip_eq_time.as_secs_f64()
        );
    } else {
        println!("✗ Round-trip noun equality FAILED!");
        all_match = false;
    }

    // Also re-cue with Chaff to verify Chaff cue of Chaff jam
    println!("\nRe-cueing Chaff jam output (CC) with Chaff...");
    let recue_start = Instant::now();
    let mut recue_chaff_slab = NounSlab::<Chaff>::new();
    let recue_chaff_noun = recue_chaff_slab
        .cue_into(chaff_chaff_jammed.clone())
        .expect("Re-cue with Chaff should succeed");
    recue_chaff_slab.set_root(recue_chaff_noun);
    println!("  Done in {:.2}s", recue_start.elapsed().as_secs_f64());

    println!("Comparing Chaff re-cued noun with original...");
    let roundtrip_chaff_eq_start = Instant::now();
    let roundtrip_chaff_equal = slab_noun_equality(unsafe { nock_slab_original.root() }, unsafe {
        recue_chaff_slab.root()
    });
    let roundtrip_chaff_eq_time = roundtrip_chaff_eq_start.elapsed();
    if roundtrip_chaff_equal {
        println!(
            "✓ Chaff round-trip noun equality verified (in {:.2}s)",
            roundtrip_chaff_eq_time.as_secs_f64()
        );
    } else {
        println!("✗ Chaff round-trip noun equality FAILED!");
        all_match = false;
    }

    // --- Final Summary ---
    println!("\n--- Verification Summary ---\n");
    if nouns_equal && all_match {
        println!("✓ ALL VERIFICATIONS PASSED");
        println!("  - Cued nouns are structurally equal (slab_noun_equality)");
        println!("  - All 4 jam outputs are byte-identical (4x4 matrix)");
        println!("  - Round-trip (jam->cue with NockJammer) preserves noun equality");
        println!("  - Round-trip (jam->cue with Chaff) preserves noun equality");
    } else {
        println!("✗ VERIFICATION FAILURES DETECTED");
        if !nouns_equal {
            println!("  - Cued nouns are NOT equal");
        }
        if !all_match {
            println!("  - Jam outputs or round-trip check failed");
        }
    }

    println!("\n=== SUMMARY ===\n");
    println!(
        "Input size: {:.2} GB",
        jammed_state.len() as f64 / (1024.0 * 1024.0 * 1024.0)
    );
    println!("Cue speedup: {:.2}x (Chaff vs NockJammer)", cue_speedup);
    println!("Jam speedup: {:.2}x (Chaff vs NockJammer)", jam_speedup);
}
