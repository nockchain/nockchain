//! Atomic, content-addressed storage for native Honk build products.

use std::collections::HashSet;
use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use nockasm::NasmBundle;
use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const CACHE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheObjectKind {
    DependencyVase,
    EntryProduct,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheMetadata {
    cache_version: u32,
    key: String,
    kind: CacheObjectKind,
    logical_source: String,
    dependency_keys: Vec<String>,
    pack_blake3: String,
    pack_bytes: u64,
    root_name: String,
}

pub struct CacheRead {
    pub bundle: Rc<NasmBundle>,
    pub root_name: String,
}

pub struct CacheWrite<'a> {
    pub key: blake3::Hash,
    pub kind: CacheObjectKind,
    pub logical_source: &'a str,
    pub dependency_keys: &'a [blake3::Hash],
    pub root_name: &'a str,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionStats {
    pub hits: u64,
    pub misses: u64,
    pub writes: u64,
    pub corrupt: u64,
}

#[derive(Debug)]
pub struct BuildCache {
    root: PathBuf,
    read_enabled: bool,
    replace_existing: bool,
    damaged: HashSet<blake3::Hash>,
    loaded_packs: std::collections::HashMap<String, Rc<NasmBundle>>,
    stats: SessionStats,
}

impl BuildCache {
    pub fn new(root: PathBuf, fresh: bool) -> Self {
        Self {
            root,
            read_enabled: !fresh,
            replace_existing: fresh,
            damaged: HashSet::new(),
            loaded_packs: std::collections::HashMap::new(),
            stats: SessionStats::default(),
        }
    }

    pub fn read(&mut self, key: blake3::Hash, kind: CacheObjectKind) -> Result<Option<CacheRead>> {
        if !self.read_enabled {
            self.stats.misses += 1;
            return Ok(None);
        }
        let metadata_path = self.metadata_path(key);
        if !metadata_path.is_file() {
            self.stats.misses += 1;
            return Ok(None);
        }
        let loaded = (|| -> Result<CacheRead> {
            let metadata: CacheMetadata = serde_json::from_slice(&fs::read(&metadata_path)?)?;
            if metadata.cache_version != CACHE_VERSION
                || metadata.key != key.to_hex().as_str()
                || metadata.kind != kind
            {
                return Err("cache metadata identity mismatch".into());
            }
            let bundle = if let Some(bundle) = self.loaded_packs.get(&metadata.pack_blake3) {
                bundle.clone()
            } else {
                let pack_path = self.pack_path(&metadata.pack_blake3)?;
                let bytes = fs::read(&pack_path)?;
                if bytes.len() as u64 != metadata.pack_bytes
                    || blake3::hash(&bytes).to_hex().as_str() != metadata.pack_blake3
                {
                    return Err("cache pack hash mismatch".into());
                }
                // blake3 above pins these as exactly the bytes the write path
                // produced, and both write paths only emit canonical
                // encodings, so decoding validates everything that matters —
                // re-encoding the whole bundle to prove canonicality cost a
                // full serialization of every pack on every warm start.
                let bundle = Rc::new(NasmBundle::from_bytes(&bytes)?);
                self.loaded_packs
                    .insert(metadata.pack_blake3.clone(), bundle.clone());
                bundle
            };
            if !bundle
                .roots()
                .iter()
                .any(|root| root.name() == metadata.root_name)
            {
                return Err("cache pack is missing the indexed root".into());
            }
            Ok(CacheRead {
                bundle,
                root_name: metadata.root_name,
            })
        })();
        match loaded {
            Ok(cached) => {
                self.stats.hits += 1;
                Ok(Some(cached))
            }
            Err(error) => {
                self.stats.corrupt += 1;
                self.stats.misses += 1;
                self.damaged.insert(key);
                eprintln!(
                    "[honk-cache] ignoring corrupt object {}: {error}",
                    key.to_hex()
                );
                Ok(None)
            }
        }
    }

    pub fn write_pack(&mut self, entries: &[CacheWrite<'_>], graph: &[u8]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let bundle = Rc::new(NasmBundle::from_bytes(graph)?);
        if bundle.to_bytes() != graph {
            return Err("refusing to write non-canonical cache pack".into());
        }
        self.write_pack_inner(entries, bundle, graph)
    }

    /// Write a pack whose bundle the caller just built. Skips the
    /// decode/re-encode canonicality round trip `write_pack` performs on
    /// untrusted bytes: `graph` must be `bundle.to_bytes()`, canonical by
    /// construction.
    pub fn write_pack_prebuilt(
        &mut self,
        entries: &[CacheWrite<'_>],
        bundle: Rc<NasmBundle>,
        graph: &[u8],
    ) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        debug_assert_eq!(bundle.to_bytes(), graph);
        self.write_pack_inner(entries, bundle, graph)
    }

    fn write_pack_inner(
        &mut self,
        entries: &[CacheWrite<'_>],
        bundle: Rc<NasmBundle>,
        graph: &[u8],
    ) -> Result<()> {
        for entry in entries {
            if !bundle
                .roots()
                .iter()
                .any(|root| root.name() == entry.root_name)
            {
                return Err(format!("cache pack is missing root {:?}", entry.root_name).into());
            }
        }
        let pack_hash = blake3::hash(graph);
        let pack_hex = pack_hash.to_hex().to_string();
        let pack_path = self.pack_path(&pack_hex)?;
        if self.replace_existing || !pack_path.is_file() {
            atomic_write(&pack_path, graph, Durability::Synced)?;
        }
        self.loaded_packs.insert(pack_hex.clone(), bundle);
        for entry in entries {
            let metadata_path = self.metadata_path(entry.key);
            if !self.replace_existing
                && !self.damaged.contains(&entry.key)
                && metadata_path.is_file()
            {
                continue;
            }
            let metadata = CacheMetadata {
                cache_version: CACHE_VERSION,
                key: entry.key.to_hex().to_string(),
                kind: entry.kind,
                logical_source: entry.logical_source.to_string(),
                dependency_keys: entry
                    .dependency_keys
                    .iter()
                    .map(|key| key.to_hex().to_string())
                    .collect(),
                pack_blake3: pack_hex.clone(),
                pack_bytes: graph.len() as u64,
                root_name: entry.root_name.to_string(),
            };
            // Metadata is rename-atomic but deliberately not fsynced: losing a
            // metadata file to a crash produces a cache miss, which the read
            // path already tolerates, and per-file F_FULLFSYNC on macOS costs
            // more than the entire logical write. The pack itself stays synced
            // above so metadata can never outlive the bytes it points at.
            atomic_write(
                &metadata_path,
                &serde_json::to_vec_pretty(&metadata)?,
                Durability::Relaxed,
            )?;
            self.damaged.remove(&entry.key);
            self.stats.writes += 1;
        }
        Ok(())
    }

    pub fn stats(&self) -> SessionStats {
        self.stats
    }

    /// Reclassify a graph that passed the envelope checks but whose compiler
    /// payload failed semantic decoding.
    pub fn reject_loaded_payload(&mut self, key: blake3::Hash) {
        self.stats.hits = self.stats.hits.saturating_sub(1);
        self.stats.misses += 1;
        self.stats.corrupt += 1;
        self.damaged.insert(key);
    }

    fn metadata_path(&self, key: blake3::Hash) -> PathBuf {
        let hex = key.to_hex().to_string();
        self.root
            .join(format!("v{CACHE_VERSION}/objects"))
            .join(&hex[..2])
            .join(&hex[2..])
            .with_extension("json")
    }

    fn pack_path(&self, pack_hex: &str) -> Result<PathBuf> {
        if pack_hex.len() != 64 || !pack_hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("invalid cache pack hash".into());
        }
        Ok(self
            .root
            .join(format!("v{CACHE_VERSION}/packs"))
            .join(&pack_hex[..2])
            .join(&pack_hex[2..])
            .with_extension("ndag"))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Durability {
    /// fsync the file and its directory: the write survives power loss.
    Synced,
    /// Rename-atomic only: readers never see partial bytes, but the file may
    /// vanish on power loss. Fine for anything the cache treats as a miss.
    Relaxed,
}

fn atomic_write(path: &Path, bytes: &[u8], durability: Durability) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("object");
    let temporary = parent.join(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_nanos()
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        if durability == Durability::Synced {
            file.sync_all()?;
        }
        fs::rename(&temporary, path)?;
        if durability == Durability::Synced {
            sync_directory(parent)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[derive(Default)]
struct DiskStats {
    objects: u64,
    packs: u64,
    pack_bytes: u64,
    metadata_bytes: u64,
    orphan_files: u64,
}

fn disk_stats(root: &Path) -> Result<DiskStats> {
    let objects = root.join(format!("v{CACHE_VERSION}/objects"));
    let packs = root.join(format!("v{CACHE_VERSION}/packs"));
    let mut stats = DiskStats::default();
    let mut referenced_packs = HashSet::new();
    if objects.exists() {
        for shard in fs::read_dir(objects)? {
            let shard = shard?;
            if !shard.file_type()?.is_dir() {
                continue;
            }
            for entry in fs::read_dir(shard.path())? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
                    match serde_json::from_slice::<CacheMetadata>(&fs::read(&path)?) {
                        Ok(metadata) => {
                            referenced_packs.insert(metadata.pack_blake3);
                            stats.objects += 1;
                            stats.metadata_bytes += entry.metadata()?.len();
                        }
                        Err(_) => stats.orphan_files += 1,
                    }
                } else if entry.file_type()?.is_file() {
                    stats.orphan_files += 1;
                }
            }
        }
    }
    if packs.exists() {
        for shard in fs::read_dir(packs)? {
            let shard = shard?;
            if !shard.file_type()?.is_dir() {
                continue;
            }
            for entry in fs::read_dir(shard.path())? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str()) == Some("ndag") {
                    let stem = path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or("");
                    let prefix = shard.file_name();
                    let hash = format!("{}{stem}", prefix.to_string_lossy());
                    if referenced_packs.contains(&hash) {
                        stats.packs += 1;
                        stats.pack_bytes += entry.metadata()?.len();
                    } else {
                        stats.orphan_files += 1;
                    }
                } else if entry.file_type()?.is_file() {
                    stats.orphan_files += 1;
                }
            }
        }
    }
    Ok(stats)
}

fn gc(root: &Path, max_age: Duration) -> Result<(u64, u64)> {
    let objects = root.join(format!("v{CACHE_VERSION}/objects"));
    let packs = root.join(format!("v{CACHE_VERSION}/packs"));
    if !objects.exists() && !packs.exists() {
        return Ok((0, 0));
    }
    let cutoff = SystemTime::now()
        .checked_sub(max_age)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut removed_objects = 0u64;
    let mut removed_bytes = 0u64;
    if objects.exists() {
        for shard in fs::read_dir(&objects)? {
            let shard = shard?;
            if !shard.file_type()?.is_dir() {
                continue;
            }
            for entry in fs::read_dir(shard.path())? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                    continue;
                }
                let modified = entry
                    .metadata()?
                    .modified()
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                if modified >= cutoff {
                    continue;
                }
                removed_bytes += entry.metadata()?.len();
                fs::remove_file(path)?;
                removed_objects += 1;
            }
            if fs::read_dir(shard.path())?.next().is_none() {
                fs::remove_dir(shard.path())?;
            }
        }
    }

    let mut referenced_packs = HashSet::new();
    if objects.exists() {
        for shard in fs::read_dir(&objects)? {
            let shard = shard?;
            if !shard.file_type()?.is_dir() {
                continue;
            }
            for entry in fs::read_dir(shard.path())? {
                let path = entry?.path();
                if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
                    if let Ok(metadata) = serde_json::from_slice::<CacheMetadata>(&fs::read(path)?)
                    {
                        referenced_packs.insert(metadata.pack_blake3);
                    }
                }
            }
        }
    }
    if packs.exists() {
        for shard in fs::read_dir(&packs)? {
            let shard = shard?;
            if !shard.file_type()?.is_dir() {
                continue;
            }
            for entry in fs::read_dir(shard.path())? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str()) != Some("ndag") {
                    continue;
                }
                let stem = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("");
                let hash = format!("{}{stem}", shard.file_name().to_string_lossy());
                if !referenced_packs.contains(&hash) {
                    removed_bytes += entry.metadata()?.len();
                    fs::remove_file(path)?;
                }
            }
            if fs::read_dir(shard.path())?.next().is_none() {
                fs::remove_dir(shard.path())?;
            }
        }
    }
    Ok((removed_objects, removed_bytes))
}

pub fn run_cli(args: &[String]) -> Result<String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err(cache_usage().into());
    };
    let cache_dir = option_value(args, "--cache-dir")
        .map(PathBuf::from)
        .ok_or_else(cache_usage)?;
    match command {
        "stats" => {
            let stats = disk_stats(&cache_dir)?;
            Ok(format!(
                "cache={} objects={} packs={} pack-bytes={} metadata-bytes={} orphan-files={}",
                cache_dir.display(),
                stats.objects,
                stats.packs,
                stats.pack_bytes,
                stats.metadata_bytes,
                stats.orphan_files
            ))
        }
        "gc" => {
            let days = option_value(args, "--max-age-days")
                .map(str::parse::<u64>)
                .transpose()?
                .unwrap_or(30);
            let (objects, bytes) =
                gc(&cache_dir, Duration::from_secs(days.saturating_mul(86_400)))?;
            Ok(format!(
                "cache={} removed-objects={} removed-bytes={}",
                cache_dir.display(),
                objects,
                bytes
            ))
        }
        "--help" | "-h" | "help" => Ok(cache_usage()),
        other => Err(format!("unknown cache command {other:?}\n{}", cache_usage()).into()),
    }
}

fn option_value<'a>(args: &'a [String], option: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == option)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

fn cache_usage() -> String {
    "Usage: honk cache stats --cache-dir <directory>\n\
     Usage: honk cache gc --cache-dir <directory> [--max-age-days <days>]"
        .to_string()
}

#[cfg(test)]
mod tests {
    use nockasm::{lift_bundle, noun, DagInput, DagMode};

    use super::*;

    fn graph(root_name: &str) -> Vec<u8> {
        let noun = noun![[1 2] [1 2]];
        lift_bundle(&[DagInput {
            name: root_name,
            noun: &noun,
            mode: DagMode::Noun,
        }])
        .unwrap()
        .to_bytes()
    }

    #[test]
    fn atomic_cache_round_trip_and_fresh_bypass() {
        let temp = tempfile::tempdir().unwrap();
        let key = blake3::hash(b"key");
        let mut cache = BuildCache::new(temp.path().to_path_buf(), false);
        assert!(cache
            .read(key, CacheObjectKind::DependencyVase)
            .unwrap()
            .is_none());
        let root_name = key.to_hex().to_string();
        let graph = graph(&root_name);
        cache
            .write_pack(
                &[CacheWrite {
                    key,
                    kind: CacheObjectKind::DependencyVase,
                    logical_source: "a.hoon",
                    dependency_keys: &[],
                    root_name: &root_name,
                }],
                &graph,
            )
            .unwrap();
        let cached = cache
            .read(key, CacheObjectKind::DependencyVase)
            .unwrap()
            .unwrap();
        assert_eq!(
            cached.bundle.lower_root(&cached.root_name),
            nockasm::NasmBundle::from_bytes(&graph)
                .unwrap()
                .lower_root(&root_name)
        );
        let mut fresh = BuildCache::new(temp.path().to_path_buf(), true);
        assert!(fresh
            .read(key, CacheObjectKind::DependencyVase)
            .unwrap()
            .is_none());
    }

    #[test]
    fn prebuilt_pack_round_trips_like_the_parsing_path() {
        let temp = tempfile::tempdir().unwrap();
        let key = blake3::hash(b"prebuilt");
        let root_name = key.to_hex().to_string();
        let noun = noun![[7 8] [7 8]];
        let bundle = lift_bundle(&[DagInput {
            name: &root_name,
            noun: &noun,
            mode: DagMode::Noun,
        }])
        .unwrap();
        let graph = bundle.to_bytes();
        let mut cache = BuildCache::new(temp.path().to_path_buf(), false);
        cache
            .write_pack_prebuilt(
                &[CacheWrite {
                    key,
                    kind: CacheObjectKind::EntryProduct,
                    logical_source: "b.hoon",
                    dependency_keys: &[],
                    root_name: &root_name,
                }],
                Rc::new(bundle),
                &graph,
            )
            .unwrap();
        assert_eq!(cache.stats().writes, 1);
        // A fresh cache instance must read the prebuilt pack from disk exactly
        // as it reads packs written through the parsing path.
        let mut fresh = BuildCache::new(temp.path().to_path_buf(), false);
        let cached = fresh
            .read(key, CacheObjectKind::EntryProduct)
            .unwrap()
            .unwrap();
        assert_eq!(cached.bundle.lower_root(&cached.root_name), Some(noun));
        assert_eq!(fresh.stats().hits, 1);
    }

    #[test]
    fn corrupt_object_is_a_miss() {
        let temp = tempfile::tempdir().unwrap();
        let key = blake3::hash(b"key");
        let root_name = key.to_hex().to_string();
        let graph = graph(&root_name);
        let mut cache = BuildCache::new(temp.path().to_path_buf(), false);
        cache
            .write_pack(
                &[CacheWrite {
                    key,
                    kind: CacheObjectKind::DependencyVase,
                    logical_source: "a.hoon",
                    dependency_keys: &[],
                    root_name: &root_name,
                }],
                &graph,
            )
            .unwrap();
        let pack = cache
            .pack_path(blake3::hash(&graph).to_hex().as_str())
            .unwrap();
        fs::write(pack, b"broken").unwrap();
        let mut cache = BuildCache::new(temp.path().to_path_buf(), false);
        assert!(cache
            .read(key, CacheObjectKind::DependencyVase)
            .unwrap()
            .is_none());
        assert_eq!(cache.stats().corrupt, 1);
    }

    #[test]
    fn stats_and_gc_remove_expired_objects_and_unreferenced_packs() {
        let temp = tempfile::tempdir().unwrap();
        let key = blake3::hash(b"key");
        let root_name = key.to_hex().to_string();
        let graph = graph(&root_name);
        let mut cache = BuildCache::new(temp.path().to_path_buf(), false);
        cache
            .write_pack(
                &[CacheWrite {
                    key,
                    kind: CacheObjectKind::DependencyVase,
                    logical_source: "a.hoon",
                    dependency_keys: &[],
                    root_name: &root_name,
                }],
                &graph,
            )
            .unwrap();
        let stats = disk_stats(temp.path()).unwrap();
        assert_eq!(stats.objects, 1);
        assert_eq!(stats.packs, 1);
        assert_eq!(stats.pack_bytes, graph.len() as u64);
        let (removed, bytes) = gc(temp.path(), Duration::ZERO).unwrap();
        assert_eq!(removed, 1);
        assert!(bytes >= graph.len() as u64);
        let stats = disk_stats(temp.path()).unwrap();
        assert_eq!(stats.objects, 0);
        assert_eq!(stats.packs, 0);
    }
}
