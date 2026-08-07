use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run_honk(root: &Path, deps: &Path, cache: &Path, output: &Path, fresh: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_honk"));
    if fresh {
        command.arg("--new");
    }
    command
        .arg("--cache-dir")
        .arg(cache)
        .arg("--arbitrary")
        .arg("--output")
        .arg(output)
        .arg("--prelude")
        .arg(root.join("hoon/common/hoon.hoon"))
        .arg(deps.join("cache/main.hoon"))
        .arg(deps);
    command.output().expect("run honk")
}

fn assert_success(output: &Output) -> String {
    assert!(
        output.status.success(),
        "honk failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn copy_fixture(root: &Path, destination: &Path) {
    let source = root.join("crates/honk/test-assets/cache");
    let cache = destination.join("cache");
    fs::create_dir(&cache).unwrap();
    for name in ["main.hoon", "mid.hoon", "leaf.hoon", "stable.hoon"] {
        fs::copy(source.join(name), cache.join(name)).unwrap();
    }
}

fn find_object(cache: &Path, logical_source: &str) -> PathBuf {
    let objects = cache.join("v1/objects");
    for shard in fs::read_dir(objects).unwrap() {
        for entry in fs::read_dir(shard.unwrap().path()).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let metadata: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            if metadata["logical_source"] == logical_source {
                let pack = metadata["pack_blake3"].as_str().unwrap();
                return cache
                    .join("v1/packs")
                    .join(&pack[..2])
                    .join(&pack[2..])
                    .with_extension("ndag");
            }
        }
    }
    panic!("cache object for {logical_source} not found")
}

#[test]
fn persistent_cache_is_byte_exact_selective_fresh_and_corruption_safe() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf();
    let temporary = tempfile::tempdir().unwrap();
    let deps = temporary.path().join("deps");
    fs::create_dir(&deps).unwrap();
    copy_fixture(&root, &deps);
    let cache = temporary.path().join("build-cache");

    let cold = temporary.path().join("cold.jam");
    let cold_log = assert_success(&run_honk(&root, &deps, &cache, &cold, false));
    assert!(cold_log.contains("hits=0 misses=4 writes=4 corrupt=0"));

    let warm = temporary.path().join("warm.jam");
    let warm_log = assert_success(&run_honk(&root, &deps, &cache, &warm, false));
    assert!(warm_log.contains("hits=1 misses=0 writes=0 corrupt=0"));
    assert_eq!(fs::read(&cold).unwrap(), fs::read(&warm).unwrap());

    fs::write(deps.join("cache/leaf.hoon"), "%after\n").unwrap();
    let changed = temporary.path().join("changed.jam");
    let changed_log = assert_success(&run_honk(&root, &deps, &cache, &changed, false));
    assert!(changed_log.contains("hits=1 misses=3 writes=3 corrupt=0"));
    assert_ne!(fs::read(&cold).unwrap(), fs::read(&changed).unwrap());

    let fresh = temporary.path().join("fresh.jam");
    let fresh_log = assert_success(&run_honk(&root, &deps, &cache, &fresh, true));
    assert!(fresh_log.contains("hits=0 misses=4 writes=4 corrupt=0"));
    assert_eq!(fs::read(&changed).unwrap(), fs::read(&fresh).unwrap());

    let stable = find_object(&cache, "cache/stable.hoon");
    fs::write(stable, b"corrupt but recoverable").unwrap();
    fs::write(deps.join("cache/leaf.hoon"), "%after-again\n").unwrap();
    let recovered = temporary.path().join("recovered.jam");
    let recovered_log = assert_success(&run_honk(&root, &deps, &cache, &recovered, false));
    assert!(
        recovered_log.contains("hits=0 misses=4 writes=4 corrupt=1"),
        "unexpected recovery stats:\n{recovered_log}"
    );
}
