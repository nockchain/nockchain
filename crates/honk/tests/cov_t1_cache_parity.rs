//! Coverage-driven tests: products served from the persistent build cache must
//! be byte-identical to uncached builds, in every output mode, for both cached
//! object kinds (entry products and dependency vases), across modes, in batch
//! builds, and after recovering from damaged or undecodable cache objects.
//!
//! Added to close branch-coverage gaps; see the coverage report in the PR.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Standard,
    Arbitrary,
    Dynock,
    DynockTyped,
}

const MODES: [Mode; 4] = [Mode::Standard, Mode::Arbitrary, Mode::Dynock, Mode::DynockTyped];

impl Mode {
    fn flag(self) -> Option<&'static str> {
        match self {
            Mode::Standard => None,
            Mode::Arbitrary => Some("--arbitrary"),
            Mode::Dynock => Some("--dynock"),
            Mode::DynockTyped => Some("--dynock-typed"),
        }
    }

    fn batch_name(self) -> &'static str {
        match self {
            Mode::Standard => "standard",
            Mode::Arbitrary => "arbitrary",
            Mode::Dynock => "dynock",
            Mode::DynockTyped => "dynock-typed",
        }
    }
}

/// A dependency tree shared by two entries: `main.hoon` (a plain noun, from
/// the existing cache fixture) and `kernel.hoon` (a kernel gate, so it builds
/// in standard mode too). Caches and outputs live outside the dependency
/// directory, whose contents feed the standard-mode directory hash.
struct Fixture {
    root: PathBuf,
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .to_path_buf();
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("deps/cache");
        fs::create_dir_all(&cache).unwrap();
        for name in ["main.hoon", "mid.hoon", "leaf.hoon", "stable.hoon"] {
            fs::copy(
                root.join("crates/honk/test-assets/cache").join(name),
                cache.join(name),
            )
            .unwrap();
        }
        fs::write(
            cache.join("kernel.hoon"),
            "/=  mid  /cache/mid\n/=  stable  /cache/stable\n|=  hash=@uvI\n[hash mid stable]\n",
        )
        .unwrap();
        fs::create_dir(temp.path().join("out")).unwrap();
        Self { root, temp }
    }

    fn deps(&self) -> PathBuf {
        self.temp.path().join("deps")
    }

    fn entry(&self, name: &str) -> PathBuf {
        self.deps().join("cache").join(name)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.temp.path().join(name)
    }

    fn output(&self, name: &str) -> PathBuf {
        self.temp.path().join("out").join(name)
    }

    fn command(&self, cache: Option<&Path>) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_honk"));
        if let Some(cache) = cache {
            command.arg("--cache-dir").arg(cache);
        }
        command
            .arg("--prelude")
            .arg(self.root.join("hoon/common/hoon.hoon"));
        command
    }

    /// Build `entry` in `mode` and return the output bytes and stderr.
    fn build(
        &self,
        entry: &str,
        mode: Mode,
        cache: Option<&Path>,
        label: &str,
    ) -> (Vec<u8>, String) {
        let output = self.output(&format!("{label}.jam"));
        let mut command = self.command(cache);
        if let Some(flag) = mode.flag() {
            command.arg(flag);
        }
        command
            .arg("--output")
            .arg(&output)
            .arg(self.entry(entry))
            .arg(self.deps());
        let result = command.output().expect("run honk");
        let log = String::from_utf8_lossy(&result.stderr).into_owned();
        assert!(result.status.success(), "honk {label} failed:\n{log}");
        (fs::read(&output).unwrap(), log)
    }

    /// Build a batch manifest of `(entry, mode, label)` lines; return the
    /// outputs in order and stderr.
    fn batch(
        &self,
        entries: &[(&str, Mode, &str)],
        cache: Option<&Path>,
        manifest_name: &str,
    ) -> (std::process::Output, Vec<PathBuf>) {
        let mut manifest = String::new();
        let mut outputs = Vec::new();
        for (entry, mode, label) in entries {
            let output = self.output(&format!("{label}.jam"));
            manifest.push_str(&format!(
                "{}\t{}\t{}\n",
                output.display(),
                self.entry(entry).display(),
                mode.batch_name()
            ));
            outputs.push(output);
        }
        let manifest_path = self.path(manifest_name);
        fs::write(&manifest_path, manifest).unwrap();
        let mut command = self.command(cache);
        command
            .arg("--batch-manifest")
            .arg(&manifest_path)
            .arg(self.deps());
        (command.output().expect("run honk"), outputs)
    }
}

/// The `hits=.. misses=.. writes=.. corrupt=..` summary a cached build prints.
fn cache_stats(log: &str) -> &str {
    log.lines()
        .filter_map(|line| line.strip_prefix("[honk-cache] "))
        .find(|summary| summary.starts_with("hits="))
        .unwrap_or_else(|| panic!("no cache summary in:\n{log}"))
}

fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Every metadata file in `cache`, with its parsed JSON.
fn metadata_files(cache: &Path) -> Vec<(PathBuf, serde_json::Value)> {
    let mut found = Vec::new();
    for shard in fs::read_dir(cache.join("v1/objects")).unwrap() {
        for entry in fs::read_dir(shard.unwrap().path()).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
                let metadata = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                found.push((path, metadata));
            }
        }
    }
    found
}

fn metadata_for(cache: &Path, logical_source: &str) -> (PathBuf, serde_json::Value) {
    metadata_files(cache)
        .into_iter()
        .find(|(_, metadata)| metadata["logical_source"] == logical_source)
        .unwrap_or_else(|| panic!("no cache object for {logical_source}"))
}

fn pack_path(cache: &Path, pack_hex: &str) -> PathBuf {
    cache
        .join("v1/packs")
        .join(&pack_hex[..2])
        .join(&pack_hex[2..])
        .with_extension("ndag")
}

/// Point the object for `logical_source` at a well-formed pack whose indexed
/// root holds `payload`. The envelope checks pass, so the compiler itself has
/// to reject the payload.
fn replace_payload(cache: &Path, logical_source: &str, payload: &nockasm::Noun) {
    let (path, mut metadata) = metadata_for(cache, logical_source);
    let root_name = metadata["root_name"].as_str().unwrap().to_string();
    let graph = nockasm::lift_bundle(&[nockasm::DagInput {
        name: &root_name,
        noun: payload,
        mode: nockasm::DagMode::Noun,
    }])
    .unwrap()
    .to_bytes();
    let pack_hex = blake3::hash(&graph).to_hex().to_string();
    let pack = pack_path(cache, &pack_hex);
    fs::create_dir_all(pack.parent().unwrap()).unwrap();
    fs::write(&pack, &graph).unwrap();
    metadata["pack_blake3"] = serde_json::json!(pack_hex);
    metadata["pack_bytes"] = serde_json::json!(graph.len());
    fs::write(path, serde_json::to_vec_pretty(&metadata).unwrap()).unwrap();
}

#[test]
fn cov_t1_cache_served_products_match_uncached_builds_in_every_mode() {
    let fixture = Fixture::new();
    let references: Vec<(Mode, Vec<u8>)> = MODES
        .iter()
        .map(|&mode| {
            let (bytes, _) = fixture.build("kernel.hoon", mode, None, &format!("ref-{mode:?}"));
            (mode, bytes)
        })
        .collect();

    // Seed a cache with the shared dependencies through a different entry.
    let seed = fixture.path("seed-cache");
    let (_, log) = fixture.build("main.hoon", Mode::Arbitrary, Some(&seed), "seed");
    assert_eq!(cache_stats(&log), "hits=0 misses=4 writes=4 corrupt=0");

    for (mode, reference) in &references {
        let cache = fixture.path(&format!("cache-{mode:?}"));
        copy_dir(&seed, &cache);
        // `mid` and `stable` are served as dependency vases.
        let (bytes, log) = fixture.build(
            "kernel.hoon",
            *mode,
            Some(&cache),
            &format!("deps-{mode:?}"),
        );
        assert_eq!(
            cache_stats(&log),
            "hits=2 misses=1 writes=1 corrupt=0",
            "{mode:?}"
        );
        assert!(
            &bytes == reference,
            "{mode:?}: cached dependency vases changed the build"
        );
        // The whole entry product is served.
        let (bytes, log) = fixture.build(
            "kernel.hoon",
            *mode,
            Some(&cache),
            &format!("warm-{mode:?}"),
        );
        assert_eq!(
            cache_stats(&log),
            "hits=1 misses=0 writes=0 corrupt=0",
            "{mode:?}"
        );
        assert!(
            &bytes == reference,
            "{mode:?}: a cached entry product changed the build"
        );
    }

    // Cache keys do not include the output mode: a product cached by an
    // arbitrary-mode build is reused for every other mode.
    let arbitrary_cache = fixture.path("cache-Arbitrary");
    for (mode, reference) in &references {
        let (bytes, log) = fixture.build(
            "kernel.hoon",
            *mode,
            Some(&arbitrary_cache),
            &format!("cross-{mode:?}"),
        );
        assert_eq!(
            cache_stats(&log),
            "hits=1 misses=0 writes=0 corrupt=0",
            "{mode:?}"
        );
        assert!(
            &bytes == reference,
            "{mode:?}: a product cached in arbitrary mode changed the build"
        );
    }
}

#[test]
fn cov_t1_cache_recovery_from_damaged_objects_is_byte_identical() {
    let fixture = Fixture::new();
    let (reference, _) = fixture.build("kernel.hoon", Mode::Standard, None, "ref");
    let cache = fixture.path("cache");
    let (cold, log) = fixture.build("kernel.hoon", Mode::Standard, Some(&cache), "cold");
    assert_eq!(cache_stats(&log), "hits=0 misses=4 writes=4 corrupt=0");
    assert!(cold == reference);

    // An entry product whose pack decodes but whose payload is not a product.
    replace_payload(&cache, "cache/kernel.hoon", &nockasm::noun![0 0]);
    let (bytes, log) = fixture.build("kernel.hoon", Mode::Standard, Some(&cache), "bad-product");
    assert!(
        log.contains("[honk-cache] ignoring invalid payload"),
        "{log}"
    );
    assert_eq!(cache_stats(&log), "hits=2 misses=1 writes=1 corrupt=1");
    assert!(bytes == reference);
    // The rewrite repaired it.
    let (bytes, log) = fixture.build("kernel.hoon", Mode::Standard, Some(&cache), "repaired");
    assert_eq!(cache_stats(&log), "hits=1 misses=0 writes=0 corrupt=0");
    assert!(bytes == reference);

    // A dependency vase with an unsupported payload version. Dropping the
    // entry's metadata forces the dependencies to be consulted.
    replace_payload(&cache, "cache/stable.hoon", &nockasm::noun![2 0 0 0 0]);
    let (entry_metadata, _) = metadata_for(&cache, "cache/kernel.hoon");
    fs::remove_file(entry_metadata).unwrap();
    let (bytes, log) = fixture.build("kernel.hoon", Mode::Standard, Some(&cache), "bad-vase");
    assert!(log.contains("unsupported cached vase version"), "{log}");
    assert_eq!(cache_stats(&log), "hits=1 misses=2 writes=2 corrupt=1");
    assert!(bytes == reference);

    // Every pack damaged on disk: each object read is a corrupt miss.
    let packs: std::collections::BTreeSet<PathBuf> = metadata_files(&cache)
        .into_iter()
        .map(|(_, metadata)| pack_path(&cache, metadata["pack_blake3"].as_str().unwrap()))
        .collect();
    for pack in packs {
        let mut graph = fs::read(&pack).unwrap();
        let last = graph.len() - 1;
        graph[last] ^= 0xff;
        fs::write(&pack, graph).unwrap();
    }
    let (bytes, log) = fixture.build("kernel.hoon", Mode::Standard, Some(&cache), "bad-packs");
    assert!(log.contains("cache pack hash mismatch"), "{log}");
    assert_eq!(cache_stats(&log), "hits=0 misses=4 writes=4 corrupt=4");
    assert!(bytes == reference);

    // Truncated metadata is a corrupt miss too.
    for (path, _) in metadata_files(&cache) {
        let json = fs::read(&path).unwrap();
        fs::write(&path, &json[..json.len() / 2]).unwrap();
    }
    let (bytes, log) = fixture.build("kernel.hoon", Mode::Standard, Some(&cache), "bad-metadata");
    assert!(cache_stats(&log).ends_with("corrupt=4"), "{log}");
    assert!(bytes == reference);
}

#[test]
fn cov_t1_batch_builds_served_from_the_cache_match_uncached_builds() {
    let fixture = Fixture::new();
    let (main_reference, _) = fixture.build("main.hoon", Mode::Arbitrary, None, "ref-main");
    let (kernel_reference, _) = fixture.build("kernel.hoon", Mode::Standard, None, "ref-kernel");
    let cache = fixture.path("cache");

    // Cold: both entries and all four dependencies are compiled and written.
    let cold = [
        ("main.hoon", Mode::Arbitrary, "cold-main"),
        ("kernel.hoon", Mode::Standard, "cold-kernel"),
    ];
    let (result, outputs) = fixture.batch(&cold, Some(&cache), "cold.tsv");
    let log = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(result.status.success(), "{log}");
    assert_eq!(cache_stats(&log), "hits=0 misses=5 writes=5 corrupt=0");
    assert!(fs::read(&outputs[0]).unwrap() == main_reference);
    assert!(fs::read(&outputs[1]).unwrap() == kernel_reference);

    // Warm: both entry products are served.
    let warm = [
        ("main.hoon", Mode::Arbitrary, "warm-main"),
        ("kernel.hoon", Mode::Standard, "warm-kernel"),
    ];
    let (result, outputs) = fixture.batch(&warm, Some(&cache), "warm.tsv");
    let log = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(result.status.success(), "{log}");
    assert_eq!(cache_stats(&log), "hits=2 misses=0 writes=0 corrupt=0");
    assert!(fs::read(&outputs[0]).unwrap() == main_reference);
    assert!(fs::read(&outputs[1]).unwrap() == kernel_reference);
}

/// A batch that lists one entry twice (for example in two output modes)
/// compiles it twice and queues two cache objects under the same key; the
/// flush then fails on the duplicate root name and the build exits non-zero.
#[test]
#[ignore = "known bug: repeated batch entries queue duplicate cache roots"]
fn cov_t1_batch_with_a_repeated_entry_populates_the_cache() {
    let fixture = Fixture::new();
    let cache = fixture.path("cache");
    let (result, _) = fixture.batch(
        &[
            ("kernel.hoon", Mode::Dynock, "twice-dynock"),
            ("kernel.hoon", Mode::Arbitrary, "twice-arbitrary"),
        ],
        Some(&cache),
        "twice.tsv",
    );
    let log = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(result.status.success(), "{log}");
}
