use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use nockapp::kernel::boot;
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockvm::ext::NounExt;
use nockvm::mem::{NockStack, NOCK_STACK_SIZE};
use nockvm::noun::{D, T};
use noun_serde::{NounDecode, NounEncode};
use roswell::{ProofStreamWindowRequest, PuzzleRequest, Roswell};
use zkvm_jetpack::form::{Proof, ProofSnapshot, ProofStreamRange, ProofStreamWindow, ProofVersion};

fn roswell_bin() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_roswell") {
        return PathBuf::from(path);
    }
    if let Ok(path) = std::env::var("ROSWELL_BIN") {
        return PathBuf::from(path);
    }
    panic!("roswell binary path not provided");
}

fn fixture(name: &str) -> PathBuf {
    if let Ok(dir) = std::env::var("ROSWELL_FIXTURE_DIR") {
        return Path::new(&dir).join(name);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn temp_stem(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("roswell-{name}-{}-{nonce}", std::process::id()))
}

fn run_roswell(args: &[&str]) -> Output {
    Command::new(roswell_bin())
        .args(["--new", "--ephemeral"])
        .args(args)
        .output()
        .expect("run roswell")
}

fn assert_success(output: Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn assert_failure(output: Output, context: &str) {
    assert!(
        !output.status.success(),
        "{context} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn generate_stream_window(v: u64, n: u64, start: u64, end: Option<u64>, name: &str) -> PathBuf {
    let stem = temp_stem(name);
    let stem_arg = stem.to_string_lossy().into_owned();
    let mut args = vec![
        String::from("make-proof-stream-window"),
        v.to_string(),
        n.to_string(),
        start.to_string(),
    ];
    if let Some(end) = end {
        args.push(String::from("--end"));
        args.push(end.to_string());
    }
    args.push(String::from("--filename"));
    args.push(stem_arg);
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = run_roswell(&arg_refs);
    assert_success(output, "generate stream window");
    stem.with_extension("jam")
}

fn generate_snapshot(v: u64, n: u64, name: &str) -> PathBuf {
    let stem = temp_stem(name);
    let stem_arg = stem.to_string_lossy().into_owned();
    let output = run_roswell(&[
        "make-proof-snapshot",
        &v.to_string(),
        &n.to_string(),
        "--filename",
        &stem_arg,
    ]);
    assert_success(output, "generate proof snapshot");
    stem.with_extension("jam")
}

fn decode_snapshot(path: &Path) -> ProofSnapshot {
    let bytes = fs::read(path).expect("read proof snapshot");
    let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
    let noun = <nockvm::noun::Noun as NounExt>::cue_bytes_slice(&mut stack, &bytes)
        .expect("snapshot should cue");
    let space = stack.noun_space();
    ProofSnapshot::from_noun(&noun, &space).expect("snapshot should decode")
}

fn decode_stream_window(path: &Path) -> ProofStreamWindow {
    let bytes = fs::read(path).expect("read stream window");
    let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
    let noun = <nockvm::noun::Noun as NounExt>::cue_bytes_slice(&mut stack, &bytes)
        .expect("stream window should cue");
    let space = stack.noun_space();
    ProofStreamWindow::from_noun(&noun, &space).expect("stream window should decode")
}

fn write_stream_window(path: &Path, window: &ProofStreamWindow) {
    let mut slab = NounSlab::<NockJammer>::new();
    let noun = window.to_noun(&mut slab);
    slab.set_root(noun);
    fs::write(path, slab.jam()).expect("write stream window");
}

fn encode_proof(proof: &Proof) -> Vec<u8> {
    let mut slab = NounSlab::<NockJammer>::new();
    let noun = proof.to_noun(&mut slab);
    slab.set_root(noun);
    slab.jam().to_vec()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn library_api_generates_and_assembles_public_proofs() {
    let mut roswell = Roswell::boot(boot::ephemeral_test_boot_cli(true))
        .await
        .expect("boot public Roswell");
    let request = PuzzleRequest::new(ProofVersion::V2, 1);

    let proof = roswell
        .prove_puzzle(&request)
        .await
        .expect("prove built-in puzzle")
        .proof;
    assert!(
        roswell.check_proof(&proof).await.expect("check proof"),
        "library-generated proof should verify"
    );

    let snapshot = roswell
        .make_proof_snapshot(&request)
        .await
        .expect("make proof snapshot")
        .snapshot;
    let full_window = roswell
        .make_proof_stream_window(&ProofStreamWindowRequest {
            puzzle: request.clone(),
            range: ProofStreamRange {
                start: 0,
                end: None,
            },
        })
        .await
        .expect("make full proof stream window")
        .window;
    let assembled = roswell
        .assemble_proof_stream(std::slice::from_ref(&full_window), None)
        .await
        .expect("assemble full proof stream")
        .proof;
    assert_eq!(encode_proof(&proof), encode_proof(&assembled));

    let start = snapshot.transcript.objects.len() as u64;
    let continuation = roswell
        .make_proof_stream_window(&ProofStreamWindowRequest {
            puzzle: request,
            range: ProofStreamRange { start, end: None },
        })
        .await
        .expect("make continuation stream window")
        .window;
    let continued = roswell
        .assemble_proof_continuation(
            &snapshot,
            &continuation.context,
            std::slice::from_ref(&continuation),
            None,
        )
        .await
        .expect("assemble continuation stream")
        .proof;
    assert_eq!(encode_proof(&proof), encode_proof(&continued));
}

fn assemble_stream_windows(windows: &[&Path], output_stem: Option<&Path>) -> Output {
    let mut args = vec![String::from("assemble-proof-stream")];
    for window in windows {
        args.push(String::from("--window"));
        args.push(window.to_string_lossy().into_owned());
    }
    if let Some(stem) = output_stem {
        args.push(String::from("--filename"));
        args.push(stem.to_string_lossy().into_owned());
    }
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    run_roswell(&arg_refs)
}

fn assemble_proof_continuation(
    snapshot: &Path,
    windows: &[&Path],
    output_stem: Option<&Path>,
) -> Output {
    let mut args = vec![
        String::from("assemble-proof-continuation"),
        String::from("--snapshot"),
        snapshot.to_string_lossy().into_owned(),
    ];
    for window in windows {
        args.push(String::from("--window"));
        args.push(window.to_string_lossy().into_owned());
    }
    if let Some(stem) = output_stem {
        args.push(String::from("--filename"));
        args.push(stem.to_string_lossy().into_owned());
    }
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    run_roswell(&arg_refs)
}

#[test]
fn verifies_public_proof_fixtures() {
    for proof in ["proof-v0-len1.jam", "proof-v1-len1.jam", "proof-v2-len1.jam"] {
        let path = fixture(proof);
        let output = Command::new(roswell_bin())
            .args(["--new", "--ephemeral", "check-proof", "--proof"])
            .arg(&path)
            .output()
            .expect("run roswell check-proof");
        assert_success(output, proof);
    }
}

#[test]
fn rejects_mutated_public_proof_fixture() {
    let mut bytes = fs::read(fixture("proof-v2-len1.jam")).expect("read fixture");
    let index = bytes.len() / 2;
    bytes[index] ^= 0x01;

    let path = temp_stem("mutated-proof").with_extension("jam");
    fs::write(&path, bytes).expect("write mutated fixture");

    let output = Command::new(roswell_bin())
        .args(["--new", "--ephemeral", "check-proof", "--proof"])
        .arg(&path)
        .output()
        .expect("run roswell check-proof");
    let _ = fs::remove_file(&path);

    assert!(
        !output.status.success(),
        "mutated proof unexpectedly verified\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn regenerates_v2_public_proof_fixture() {
    let stem = temp_stem("v2-fixture");
    let stem_arg = stem.to_string_lossy().into_owned();
    let output = run_roswell(&["prove-puzzle", "2", "1", "--filename", &stem_arg]);
    assert_success(output, "regenerate v2 fixture");

    let generated_path = stem.with_extension("jam");
    let generated = fs::read(&generated_path).expect("read generated fixture");
    let expected = fs::read(fixture("proof-v2-len1.jam")).expect("read expected fixture");
    let _ = fs::remove_file(&generated_path);
    assert_eq!(generated, expected);
}

#[test]
fn generates_stream_windows_for_public_proof_versions() {
    for (version_arg, expected_version) in [
        (0, ProofVersion::V0),
        (1, ProofVersion::V1),
        (2, ProofVersion::V2),
        (3, ProofVersion::V3),
    ] {
        let path = generate_stream_window(version_arg, 1, 0, Some(1), "stream-version");
        let bytes = fs::read(&path).expect("read stream window");
        let _ = fs::remove_file(&path);
        let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
        let noun = <nockvm::noun::Noun as NounExt>::cue_bytes_slice(&mut stack, &bytes)
            .expect("stream window should cue");
        let space = stack.noun_space();
        let window = ProofStreamWindow::from_noun(&noun, &space).expect("window should decode");
        assert_eq!(window.format, 0);
        assert_eq!(window.proof_version, expected_version);
        assert_eq!(window.range.start, 0);
        assert_eq!(window.range.end, Some(1));
        assert_eq!(window.objects.len(), 1);
        assert!(window.context.total >= 1);
    }
}

#[test]
fn generates_deterministic_proof_snapshot() {
    let stem_a = temp_stem("snapshot-a");
    let stem_b = temp_stem("snapshot-b");
    let stem_a_arg = stem_a.to_string_lossy().into_owned();
    let stem_b_arg = stem_b.to_string_lossy().into_owned();

    let output = run_roswell(&["make-proof-snapshot", "2", "1", "--filename", &stem_a_arg]);
    assert_success(output, "generate first snapshot");
    let output = run_roswell(&["make-proof-snapshot", "2", "1", "--filename", &stem_b_arg]);
    assert_success(output, "generate second snapshot");

    let path_a = stem_a.with_extension("jam");
    let path_b = stem_b.with_extension("jam");
    let snapshot_a = fs::read(&path_a).expect("read first snapshot");
    let snapshot_b = fs::read(&path_b).expect("read second snapshot");
    let _ = fs::remove_file(&path_a);
    let _ = fs::remove_file(&path_b);

    let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
    let snapshot_noun = <nockvm::noun::Noun as NounExt>::cue_bytes_slice(&mut stack, &snapshot_a)
        .expect("snapshot should cue");
    let space = stack.noun_space();
    let snapshot =
        ProofSnapshot::from_noun(&snapshot_noun, &space).expect("snapshot should decode");
    assert_eq!(snapshot.format, 0);
    assert_eq!(snapshot.proof_version, ProofVersion::V2);
    assert_eq!(snapshot.table_count as usize, snapshot.tables.len());
    assert!(!snapshot.transcript.objects.is_empty());
    let encoded_snapshot = snapshot.to_noun(&mut stack);
    let reparsed_space = stack.noun_space();
    let reparsed_snapshot =
        ProofSnapshot::from_noun(&encoded_snapshot, &reparsed_space).expect("snapshot re-decodes");
    assert!(snapshot == reparsed_snapshot);
    let reencoded = encoded_snapshot.jam_self(&mut stack);
    assert_eq!(snapshot_a, reencoded.0.as_ref());

    assert!(!snapshot_a.is_empty());
    assert_eq!(snapshot_a, snapshot_b);
}

#[test]
fn stream_window_roundtrips_and_assembles() {
    let full_stem = temp_stem("stream-full");
    let proof_stem = temp_stem("stream-proof");
    let full_arg = full_stem.to_string_lossy().into_owned();
    let proof_arg = proof_stem.to_string_lossy().into_owned();

    let output = run_roswell(&["make-proof-stream-window", "2", "1", "0", "--filename", &full_arg]);
    assert_success(output, "generate full stream window");

    let full_path = full_stem.with_extension("jam");
    let full_bytes = fs::read(&full_path).expect("read full stream window");
    let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
    let full_noun = <nockvm::noun::Noun as NounExt>::cue_bytes_slice(&mut stack, &full_bytes)
        .expect("window should cue");
    let space = stack.noun_space();
    let full_window =
        ProofStreamWindow::from_noun(&full_noun, &space).expect("window should decode");
    assert_eq!(full_window.format, 0);
    assert_eq!(full_window.proof_version, ProofVersion::V2);
    assert_eq!(full_window.range.start, 0);
    assert_eq!(full_window.range.end, None);
    assert_eq!(
        full_window.context.total as usize,
        full_window.objects.len()
    );

    let encoded_window = full_window.to_noun(&mut stack);
    let reparsed_space = stack.noun_space();
    let reparsed_window =
        ProofStreamWindow::from_noun(&encoded_window, &reparsed_space).expect("window re-decodes");
    assert_eq!(full_window, reparsed_window);

    let full_path_arg = full_path.to_string_lossy().into_owned();
    let output = run_roswell(&[
        "assemble-proof-stream", "--window", &full_path_arg, "--filename", &proof_arg,
    ]);
    assert_success(output, "assemble full stream window");

    let proof_path = proof_stem.with_extension("jam");
    let proof_path_arg = proof_path.to_string_lossy().into_owned();
    let output = run_roswell(&["check-proof", "--proof", &proof_path_arg]);
    let _ = fs::remove_file(&full_path);
    let _ = fs::remove_file(&proof_path);
    assert_success(output, "verify assembled stream proof");
}

#[test]
fn adjacent_stream_windows_assemble() {
    let first_stem = temp_stem("stream-first");
    let second_stem = temp_stem("stream-second");
    let proof_stem = temp_stem("stream-split-proof");
    let first_arg = first_stem.to_string_lossy().into_owned();
    let second_arg = second_stem.to_string_lossy().into_owned();
    let proof_arg = proof_stem.to_string_lossy().into_owned();

    let output = run_roswell(&[
        "make-proof-stream-window", "2", "1", "0", "--end", "3", "--filename", &first_arg,
    ]);
    assert_success(output, "generate first stream window");
    let output =
        run_roswell(&["make-proof-stream-window", "2", "1", "3", "--filename", &second_arg]);
    assert_success(output, "generate second stream window");

    let first_path = first_stem.with_extension("jam");
    let second_path = second_stem.with_extension("jam");
    let first_path_arg = first_path.to_string_lossy().into_owned();
    let second_path_arg = second_path.to_string_lossy().into_owned();
    let output = run_roswell(&[
        "assemble-proof-stream", "--window", &first_path_arg, "--window", &second_path_arg,
        "--filename", &proof_arg,
    ]);
    assert_success(output, "assemble adjacent stream windows");

    let proof_path = proof_stem.with_extension("jam");
    let proof_path_arg = proof_path.to_string_lossy().into_owned();
    let output = run_roswell(&["check-proof", "--proof", &proof_path_arg]);
    let _ = fs::remove_file(&first_path);
    let _ = fs::remove_file(&second_path);
    let _ = fs::remove_file(&proof_path);
    assert_success(output, "verify split stream proof");
}

#[test]
fn proof_continuation_assembles_from_snapshot() {
    let snapshot = generate_snapshot(2, 1, "continuation-snapshot");
    let snapshot_decoded = decode_snapshot(&snapshot);
    let start = snapshot_decoded.transcript.objects.len() as u64;
    let continuation = generate_stream_window(2, 1, start, None, "continuation-window");
    let continuation_decoded = decode_stream_window(&continuation);
    assert!(start < continuation_decoded.context.total);

    let proof_stem = temp_stem("continuation-proof");
    let output = assemble_proof_continuation(&snapshot, &[&continuation], Some(&proof_stem));
    assert_success(output, "assemble proof continuation");

    let proof_path = proof_stem.with_extension("jam");
    let generated = fs::read(&proof_path).expect("read assembled proof");
    let expected = fs::read(fixture("proof-v2-len1.jam")).expect("read expected proof");
    assert_eq!(generated, expected);

    let proof_path_arg = proof_path.to_string_lossy().into_owned();
    let output = run_roswell(&["check-proof", "--proof", &proof_path_arg]);
    let _ = fs::remove_file(&snapshot);
    let _ = fs::remove_file(&continuation);
    let _ = fs::remove_file(&proof_path);
    assert_success(output, "verify continuation proof");
}

#[test]
fn split_proof_continuation_assembles() {
    let snapshot = generate_snapshot(2, 1, "continuation-split-snapshot");
    let snapshot_decoded = decode_snapshot(&snapshot);
    let start = snapshot_decoded.transcript.objects.len() as u64;
    let full_continuation = generate_stream_window(2, 1, start, None, "continuation-split-full");
    let full_decoded = decode_stream_window(&full_continuation);
    assert!(start + 1 < full_decoded.context.total);
    let first = generate_stream_window(2, 1, start, Some(start + 1), "continuation-split-first");
    let second = generate_stream_window(2, 1, start + 1, None, "continuation-split-second");

    let proof_stem = temp_stem("continuation-split-proof");
    let output = assemble_proof_continuation(&snapshot, &[&first, &second], Some(&proof_stem));
    assert_success(output, "assemble split proof continuation");

    let proof_path = proof_stem.with_extension("jam");
    let proof_path_arg = proof_path.to_string_lossy().into_owned();
    let output = run_roswell(&["check-proof", "--proof", &proof_path_arg]);

    for path in [snapshot, full_continuation, first, second, proof_path] {
        let _ = fs::remove_file(path);
    }
    assert_success(output, "verify split continuation proof");
}

#[test]
fn proof_continuation_rejects_invalid_windows() {
    let snapshot = generate_snapshot(2, 1, "continuation-invalid-snapshot");
    let snapshot_decoded = decode_snapshot(&snapshot);
    let start = snapshot_decoded.transcript.objects.len() as u64;
    let valid = generate_stream_window(2, 1, start, None, "continuation-invalid-valid");
    let valid_decoded = decode_stream_window(&valid);
    assert!(start + 1 < valid_decoded.context.total);

    let gap = generate_stream_window(2, 1, start + 1, None, "continuation-invalid-gap");
    let first = generate_stream_window(2, 1, start, Some(start + 1), "continuation-invalid-first");
    let overlap = generate_stream_window(2, 1, start, None, "continuation-invalid-overlap");

    let mut wrong_digest = valid_decoded.clone();
    wrong_digest.context.digest[0] ^= 1;
    let wrong_digest_path = temp_stem("continuation-wrong-digest").with_extension("jam");
    write_stream_window(&wrong_digest_path, &wrong_digest);

    let mut wrong_version = valid_decoded.clone();
    wrong_version.proof_version = ProofVersion::V1;
    let wrong_version_path = temp_stem("continuation-wrong-version").with_extension("jam");
    write_stream_window(&wrong_version_path, &wrong_version);

    let mut corrupted = valid_decoded;
    assert!(corrupted.objects.len() > 1);
    corrupted.objects.swap(0, 1);
    let corrupted_path = temp_stem("continuation-corrupted").with_extension("jam");
    write_stream_window(&corrupted_path, &corrupted);

    assert_failure(
        assemble_proof_continuation(&snapshot, &[&gap], None),
        "gap continuation",
    );
    assert_failure(
        assemble_proof_continuation(&snapshot, &[&first, &overlap], None),
        "overlap continuation",
    );
    assert_failure(
        assemble_proof_continuation(&snapshot, &[&wrong_digest_path], None),
        "wrong-digest continuation",
    );
    assert_failure(
        assemble_proof_continuation(&snapshot, &[&wrong_version_path], None),
        "wrong-version continuation",
    );
    assert_failure(
        assemble_proof_continuation(&snapshot, &[&corrupted_path], None),
        "corrupted continuation",
    );

    for path in [
        snapshot, valid, gap, first, overlap, wrong_digest_path, wrong_version_path, corrupted_path,
    ] {
        let _ = fs::remove_file(path);
    }
}

#[test]
fn stream_assembly_rejects_wrong_start() {
    let window_stem = temp_stem("stream-wrong-start");
    let window_arg = window_stem.to_string_lossy().into_owned();
    let output =
        run_roswell(&["make-proof-stream-window", "2", "1", "1", "--filename", &window_arg]);
    assert_success(output, "generate wrong-start stream window");

    let window_path = window_stem.with_extension("jam");
    let window_path_arg = window_path.to_string_lossy().into_owned();
    let output = run_roswell(&["assemble-proof-stream", "--window", &window_path_arg]);
    let _ = fs::remove_file(&window_path);
    assert!(
        !output.status.success(),
        "wrong-start stream window unexpectedly assembled\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn stream_assembly_rejects_invalid_window_sets() {
    let first = generate_stream_window(2, 1, 0, Some(3), "stream-invalid-first");
    let second = generate_stream_window(2, 1, 3, None, "stream-invalid-second");
    let overlap = generate_stream_window(2, 1, 2, None, "stream-invalid-overlap");
    let wrong_version = generate_stream_window(1, 1, 3, None, "stream-invalid-version");
    let wrong_puzzle = generate_stream_window(2, 2, 3, None, "stream-invalid-puzzle");

    assert_failure(
        assemble_stream_windows(&[&first], None),
        "missing stream window coverage",
    );
    assert_failure(
        assemble_stream_windows(&[&first, &overlap], None),
        "overlapping stream windows",
    );
    assert_failure(
        assemble_stream_windows(&[&second, &first], None),
        "out-of-order stream windows",
    );
    assert_failure(
        assemble_stream_windows(&[&first, &wrong_version], None),
        "wrong-version stream windows",
    );
    assert_failure(
        assemble_stream_windows(&[&first, &wrong_puzzle], None),
        "wrong-puzzle stream windows",
    );

    for path in [first, second, overlap, wrong_version, wrong_puzzle] {
        let _ = fs::remove_file(path);
    }
}

#[test]
fn stream_assembly_rejects_mutated_stream_objects() {
    let full = generate_stream_window(2, 1, 0, None, "stream-mutated-full");
    let bytes = fs::read(&full).expect("read full stream window");
    let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
    let noun = <nockvm::noun::Noun as NounExt>::cue_bytes_slice(&mut stack, &bytes)
        .expect("stream window should cue");
    let space = stack.noun_space();
    let mut window = ProofStreamWindow::from_noun(&noun, &space).expect("window should decode");
    assert!(window.objects.len() > 1);
    window.objects.swap(0, 1);

    let mutated = temp_stem("stream-mutated").with_extension("jam");
    let mut slab = NounSlab::<NockJammer>::new();
    let mutated_noun = window.to_noun(&mut slab);
    slab.set_root(mutated_noun);
    fs::write(&mutated, slab.jam()).expect("write mutated stream window");

    assert_failure(
        assemble_stream_windows(&[&mutated], None),
        "mutated stream window objects",
    );

    let _ = fs::remove_file(&full);
    let _ = fs::remove_file(&mutated);
}

#[test]
fn computes_jammed_nock_expression() {
    let path = temp_stem("compute").with_extension("jam");
    let mut slab = NounSlab::<NockJammer>::new();
    let formula = T(&mut slab, &[D(1), D(42)]);
    let nock = T(&mut slab, &[D(0), formula]);
    slab.set_root(nock);
    fs::write(&path, slab.jam()).expect("write nock expression");

    let path_arg = path.to_string_lossy().into_owned();
    let output = run_roswell(&["compute", "--nock", &path_arg]);
    let _ = fs::remove_file(&path);
    assert_success(output, "compute fixture");
}

#[test]
fn rejects_non_power_of_two_puzzle_length() {
    let output = run_roswell(&["test-puzzle", "2", "3"]);
    assert!(
        !output.status.success(),
        "invalid puzzle length unexpectedly passed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn help_exposes_only_public_puzzle_commands() {
    let output = Command::new(roswell_bin())
        .arg("--help")
        .output()
        .expect("run roswell help");
    assert!(
        output.status.success(),
        "roswell --help failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let help = String::from_utf8_lossy(&output.stdout);

    for expected in [
        "run-suite", "run-test", "test-puzzle", "prove-puzzle", "make-proof-snapshot",
        "make-proof-stream-window", "assemble-proof-stream", "assemble-proof-continuation",
        "check-proof", "compute",
    ] {
        assert!(
            help.contains(expected),
            "missing {expected} in help:\n{help}"
        );
    }
}
