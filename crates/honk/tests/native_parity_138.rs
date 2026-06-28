//! Byte-parity gate: does honk's NATIVE mint of the hoon-138 prelude reproduce
//! hoonc's, exactly?
//!
//! honk's canonical hoon-138 build normally substitutes hoonc's precompiled
//! prelude formula + subject-type (embedded in the binary at build time from
//! hoonc's own output — `crates/honk/assets/honc-formula-138.jam` etc.).
//! `HONK_NATIVE_PARITY=1` disables that substitution so honk mints the prelude
//! with its own `++ut`. Everything else in the `--arbitrary` build (the entry
//! compile, cold state, jam packaging) is identical native code in both runs.
//!
//! Therefore the two `--arbitrary` artifacts are byte-identical IFF honk's
//! native prelude mint == hoonc's prelude. This compares honk-against-honk
//! (default-embedded vs native), so the gate is self-contained — it needs no
//! hoonc binary and no committed 2.3 MB reference fixture. If the native mint
//! ever diverges from hoonc, the bytes differ and this test fails, naming the
//! first differing offset and the two hashes.
//!
//! This is a DEFAULT gate — no `#[ignore]`. This workspace's `[profile.test]`
//! `inherits = "release"`, so `cargo test` builds both this test and
//! `CARGO_BIN_EXE_honk` OPTIMIZED — the native prelude mint is practical (~40 s;
//! it would take minutes and a lot of RAM unoptimized). No special flags:
//!
//!   cargo test -p honk --test native_parity_138 -- --nocapture
//!
//! The whole test is ~78 s (it also runs the default embedded build as the
//! reference). The native mint needs ample RAM; it completes well under the
//! machine's memory (the old ~49-min OOM is resolved).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// Repo root, derived from this crate's manifest dir (`<repo>/crates/honk`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/honk is two levels below the repo root")
        .to_path_buf()
}

/// Run `honk --arbitrary` on the canonical hoon-138 prelude (as its own entry),
/// optionally with `HONK_NATIVE_PARITY=1`, writing the jam to `out`.
fn arbitrary_build(out: &Path, native_parity: bool) -> std::time::Duration {
    let honk = env!("CARGO_BIN_EXE_honk");
    let root = repo_root();
    let entry = "hoon/common/hoon.hoon";

    let mut cmd = Command::new(honk);
    cmd.current_dir(&root)
        .arg("--arbitrary")
        .arg("--output")
        .arg(out)
        .arg("--prelude")
        .arg(entry)
        .arg(entry) // positional: entry source
        .arg("hoon"); // positional: dependency root
    if native_parity {
        cmd.env("HONK_NATIVE_PARITY", "1");
    }

    let start = Instant::now();
    let status = cmd.status().expect("failed to spawn honk");
    let elapsed = start.elapsed();
    assert!(
        status.success(),
        "honk --arbitrary (native_parity={native_parity}) exited with {status}"
    );
    assert!(
        out.exists(),
        "honk --arbitrary (native_parity={native_parity}) produced no artifact at {}",
        out.display()
    );
    elapsed
}

/// Cheap stable digest for divergence diagnostics (std-only — integration tests
/// can't reach honk's private deps). FNV-1a over the bytes; not cryptographic,
/// just enough to tell two artifacts apart in a failure message.
fn digest(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

#[test]
fn hoon_138_native_mint_matches_hoonc() {
    let tmp = std::env::temp_dir().join(format!("honk_parity_138_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let embedded = tmp.join("embedded.jam");
    let native = tmp.join("native.jam");

    // Reference: the default build, which substitutes hoonc's embedded prelude.
    let t_ref = arbitrary_build(&embedded, false);
    // Candidate: the same build, but honk mints the prelude itself.
    let t_nat = arbitrary_build(&native, true);

    let ref_bytes = std::fs::read(&embedded).expect("read embedded-build artifact");
    let nat_bytes = std::fs::read(&native).expect("read native-build artifact");

    eprintln!(
        "[parity-138] embedded build: {} bytes in {:.1}s | native mint: {} bytes in {:.1}s",
        ref_bytes.len(),
        t_ref.as_secs_f64(),
        nat_bytes.len(),
        t_nat.as_secs_f64(),
    );

    if ref_bytes == nat_bytes {
        eprintln!("[parity-138] PASS (exact): native hoon-138 mint == hoonc");
        let _ = std::fs::remove_dir_all(&tmp);
        return;
    }

    // Report the first divergence + hashes before failing, then keep the
    // artifacts around for inspection.
    let first_diff = ref_bytes
        .iter()
        .zip(nat_bytes.iter())
        .position(|(a, b)| a != b)
        .map(|i| i.to_string())
        .unwrap_or_else(|| format!("len {} vs {}", ref_bytes.len(), nat_bytes.len()));
    panic!(
        "native hoon-138 mint diverges from hoonc:\n  \
         embedded(ref) {} bytes, digest {}\n  \
         native(cand)  {} bytes, digest {}\n  \
         first differing byte: {}\n  \
         artifacts kept at {} for jam-diff inspection",
        ref_bytes.len(),
        digest(&ref_bytes),
        nat_bytes.len(),
        digest(&nat_bytes),
        first_diff,
        tmp.display(),
    );
}
