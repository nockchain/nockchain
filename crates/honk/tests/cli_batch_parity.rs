use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("honk crate should be below the repository root")
        .to_path_buf()
}

fn run_honk(arguments: &[&Path]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_honk"));
    for argument in arguments {
        command.arg(argument);
    }
    command.output().expect("run honk")
}

fn assert_success(output: Output) {
    assert!(
        output.status.success(),
        "honk failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn batch_artifacts_match_separate_cli_invocations() {
    let root = repository_root();
    let temp = TempDir::new().expect("temporary output directory");
    let prelude = root.join("hoon/common/hoon.hoon");
    let dependencies = root.join("hoon");
    let entries = [
        root.join("crates/honk/test-assets/type-probes/auras.hoon"),
        root.join("crates/honk/test-assets/type-probes/wet_gate.hoon"),
    ];
    let batch_outputs = [temp.path().join("batch-0.jam"), temp.path().join("batch-1.jam")];
    let single_outputs = [temp.path().join("single-0.jam"), temp.path().join("single-1.jam")];
    let manifest = temp.path().join("batch.tsv");
    let manifest_contents = entries
        .iter()
        .zip(&batch_outputs)
        .map(|(entry, output)| format!("{}\t{}\tarbitrary", output.display(), entry.display()))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&manifest, format!("{manifest_contents}\n")).expect("batch manifest");

    assert_success(run_honk(&[
        Path::new("--new"),
        Path::new("--batch-manifest"),
        &manifest,
        Path::new("--prelude"),
        &prelude,
        &dependencies,
    ]));

    for ((entry, batch_output), single_output) in
        entries.iter().zip(&batch_outputs).zip(&single_outputs)
    {
        assert_success(run_honk(&[
            Path::new("--new"),
            Path::new("--arbitrary"),
            Path::new("--output"),
            single_output,
            Path::new("--prelude"),
            &prelude,
            entry,
            &dependencies,
        ]));
        assert_eq!(
            std::fs::read(batch_output).expect("batch artifact"),
            std::fs::read(single_output).expect("single artifact"),
            "batch artifact differs for {}",
            entry.display()
        );
    }
}
