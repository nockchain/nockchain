//! Byte-layout delta control, not a logical noun/DAG delta. All source files
//! must remain immutable for the lifetime of the read-only mappings.
//!
//! The virtual target retains target word offsets and its verified descriptor.
//! Pieces refer either to an equal base chunk or to borrowed target bytes. A
//! persisted delta would store the latter bytes; this control only counts them.

use anyhow::{bail, ensure, Context, Result};
use memmap2::{Mmap, MmapOptions};
use nockchain_noun_delta_bench::pma::{Node, Snapshot};
use serde::Serialize;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::Path;
use std::time::Instant;

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../tests/common/mod.rs"]
mod common;

type Digest = [u8; 32];

#[derive(Clone, Copy, Debug)]
enum Piece {
    Base(usize),
    New(usize),
}

struct VirtualPrefix<'a> {
    base: &'a [u8],
    target: &'a [u8],
    chunk_bytes: usize,
    pieces: Vec<Piece>,
}

impl VirtualPrefix<'_> {
    fn chunk(&self, index: usize) -> &[u8] {
        let len = self
            .chunk_bytes
            .min(self.target.len() - index * self.chunk_bytes);
        match self.pieces[index] {
            Piece::Base(offset) => &self.base[offset..offset + len],
            Piece::New(offset) => &self.target[offset..offset + len],
        }
    }

    fn read_exact(&self, offset: usize, mut out: &mut [u8]) -> Result<()> {
        ensure!(
            offset
                .checked_add(out.len())
                .is_some_and(|end| end <= self.target.len()),
            "virtual read outside target used prefix"
        );
        let mut offset = offset;
        while !out.is_empty() {
            let index = offset / self.chunk_bytes;
            let within = offset % self.chunk_bytes;
            let source = &self.chunk(index)[within..];
            let count = out.len().min(source.len());
            out[..count].copy_from_slice(&source[..count]);
            out = &mut out[count..];
            offset += count;
        }
        Ok(())
    }

    fn word(&self, offset_words: u64) -> Result<u64> {
        let offset = usize::try_from(
            offset_words
                .checked_mul(8)
                .context("word offset overflow")?,
        )?;
        let mut bytes = [0; 8];
        self.read_exact(offset, &mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn checksum(&self) -> Digest {
        let mut hasher = blake3::Hasher::new();
        for index in 0..self.pieces.len() {
            hasher.update(self.chunk(index));
        }
        *hasher.finalize().as_bytes()
    }
}

#[derive(Debug, Default, Serialize)]
struct WalkStats {
    visited_unique_handles: usize,
    cells: usize,
    indirect_atoms: usize,
    atom_prefix_bytes_checked: usize,
    pending_handles: usize,
    limit: usize,
}

/// Bounded structural smoke check of the actual target root through virtual
/// reads. Whole-prefix checksum validation covers the rest of the bytes; this
/// deliberately does not claim a complete graph/cycle validation.
fn check_root(view: &VirtualPrefix<'_>, snapshot: &Snapshot, limit: usize) -> Result<WalkStats> {
    let mut stats = WalkStats {
        limit,
        ..Default::default()
    };
    let mut seen = HashSet::new();
    let mut pending = vec![snapshot.root()];
    while let Some(raw) = pending.pop() {
        if seen.contains(&raw) {
            continue;
        }
        if seen.len() >= limit {
            pending.push(raw);
            break;
        }
        seen.insert(raw);
        match snapshot.node(raw)? {
            Node::Direct(value) => ensure!(raw == value && raw < 1 << 63, "invalid direct noun"),
            Node::Cell { head, tail } => {
                ensure!(
                    raw & (7 << 61) == 3 << 62 && raw & (1 << 60) != 0,
                    "invalid cell tag"
                );
                let offset = raw & !((7 << 61) | (1 << 60));
                ensure!(
                    view.word(offset + 1)? == head && view.word(offset + 2)? == tail,
                    "virtual cell differs from target"
                );
                stats.cells += 1;
                pending.push(tail);
                pending.push(head);
            }
            Node::Atom(bytes) => {
                ensure!(
                    raw & (3 << 62) == 1 << 63 && raw & (1 << 60) != 0,
                    "invalid atom tag"
                );
                let offset = raw & !((3 << 62) | (1 << 60));
                let words = view.word(offset + 1)?;
                ensure!(
                    words > 0
                        && offset
                            .checked_add(2)
                            .and_then(|n| n.checked_add(words))
                            .is_some_and(|end| end <= snapshot.metadata.alloc_words),
                    "invalid virtual atom bounds"
                );
                // Avoid a second unbounded scan of large atoms in this smoke
                // check; the complete byte checksum has already been checked.
                let count = bytes.len().min(64);
                let mut prefix = [0; 64];
                view.read_exact(
                    usize::try_from(
                        (offset + 2)
                            .checked_mul(8)
                            .context("atom byte offset overflow")?,
                    )?,
                    &mut prefix[..count],
                )?;
                ensure!(
                    prefix[..count] == bytes[..count],
                    "virtual atom prefix differs from target"
                );
                stats.atom_prefix_bytes_checked += count;
                stats.indirect_atoms += 1;
            }
        }
    }
    stats.visited_unique_handles = seen.len();
    stats.pending_handles = pending.len();
    Ok(stats)
}

#[derive(Debug, Serialize)]
struct DiffStats {
    chunk_bytes: usize,
    base_bytes: usize,
    target_bytes: usize,
    base_chunks: usize,
    target_chunks: usize,
    aligned_reused_bytes: u64,
    content_addressed_reused_bytes: u64,
    relocated_reused_bytes: u64,
    new_bytes: u64,
    // One tagged offset/length record per target chunk would require at most
    // this many fixed-width bytes, excluding the small serialized envelope.
    piece_table_bytes_at_24_per_piece: u64,
    counted_delta_bytes_with_piece_table: u64,
    base_index_seconds: f64,
    target_diff_seconds: f64,
    virtual_checksum_seconds: f64,
    base_used_blake3: String,
    target_used_blake3: String,
    virtual_used_blake3: String,
}

fn hex(hash: Digest) -> String {
    blake3::Hash::from(hash).to_hex().to_string()
}

fn compare<'a>(
    base: &'a [u8],
    target: &'a [u8],
    chunk_bytes: usize,
) -> Result<(VirtualPrefix<'a>, DiffStats)> {
    ensure!(chunk_bytes > 0, "chunk size must be positive");
    let started = Instant::now();
    let mut index: HashMap<Digest, Vec<usize>> = HashMap::new();
    let mut base_hasher = blake3::Hasher::new();
    for (number, chunk) in base.chunks(chunk_bytes).enumerate() {
        base_hasher.update(chunk);
        index
            .entry(*blake3::hash(chunk).as_bytes())
            .or_default()
            .push(number * chunk_bytes);
    }
    let base_hash = *base_hasher.finalize().as_bytes();
    let base_index_seconds = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let mut pieces = Vec::with_capacity(target.len().div_ceil(chunk_bytes));
    let mut target_hasher = blake3::Hasher::new();
    let mut aligned_reused_bytes = 0;
    let mut content_addressed_reused_bytes = 0;
    let mut new_bytes = 0;
    for (number, chunk) in target.chunks(chunk_bytes).enumerate() {
        target_hasher.update(chunk);
        let offset = number * chunk_bytes;
        // An aligned hit needs no target chunk hash. Exact bytes are required
        // both here and below: digest collisions cannot create false reuse.
        let reused = if base.get(offset..offset + chunk.len()) == Some(chunk) {
            aligned_reused_bytes += chunk.len() as u64;
            Some(offset)
        } else {
            index
                .get(blake3::hash(chunk).as_bytes())
                .and_then(|candidates| {
                    candidates.iter().copied().find(|&candidate| {
                        // Fixed-size base chunk boundaries: the final short base
                        // chunk is eligible only when its length also matches.
                        let len = chunk_bytes.min(base.len() - candidate);
                        len == chunk.len() && &base[candidate..candidate + len] == chunk
                    })
                })
        };
        if let Some(source) = reused {
            pieces.push(Piece::Base(source));
            content_addressed_reused_bytes += chunk.len() as u64;
        } else {
            pieces.push(Piece::New(offset));
            new_bytes += chunk.len() as u64;
        }
    }
    let target_hash = *target_hasher.finalize().as_bytes();
    let target_diff_seconds = started.elapsed().as_secs_f64();
    let view = VirtualPrefix {
        base,
        target,
        chunk_bytes,
        pieces,
    };
    let started = Instant::now();
    let virtual_hash = view.checksum();
    ensure!(
        virtual_hash == target_hash,
        "virtual target checksum mismatch"
    );
    let virtual_checksum_seconds = started.elapsed().as_secs_f64();
    let piece_table_bytes = u64::try_from(view.pieces.len())?
        .checked_mul(24)
        .context("piece table size overflow")?;
    let stats = DiffStats {
        chunk_bytes,
        base_bytes: base.len(),
        target_bytes: target.len(),
        base_chunks: base.len().div_ceil(chunk_bytes),
        target_chunks: view.pieces.len(),
        aligned_reused_bytes,
        content_addressed_reused_bytes,
        relocated_reused_bytes: content_addressed_reused_bytes - aligned_reused_bytes,
        new_bytes,
        piece_table_bytes_at_24_per_piece: piece_table_bytes,
        counted_delta_bytes_with_piece_table: new_bytes
            .checked_add(piece_table_bytes)
            .context("delta size overflow")?,
        base_index_seconds,
        target_diff_seconds,
        virtual_checksum_seconds,
        base_used_blake3: hex(base_hash),
        target_used_blake3: hex(target_hash),
        virtual_used_blake3: hex(virtual_hash),
    };
    Ok((view, stats))
}

struct Input {
    snapshot: Snapshot,
    _file: File,
    mapping: Option<Mmap>,
}
impl Input {
    fn open(pma: &str, descriptor: &str) -> Result<Self> {
        let snapshot = if descriptor.ends_with(".meta") {
            Snapshot::open_operative(Path::new(pma), Path::new(descriptor))?
        } else {
            Snapshot::open(Path::new(pma), Path::new(descriptor))?
        };
        let file = File::open(pma)?;
        let len = snapshot.mapped_bytes();
        // SAFETY: the caller supplies frozen copies. This mapping is read-only
        // and includes only the footer-validated used prefix, never the tail.
        let mapping = if len == 0 {
            None
        } else {
            Some(unsafe { MmapOptions::new().len(len).map(&file)? })
        };
        Ok(Self {
            snapshot,
            _file: file,
            mapping,
        })
    }
    fn bytes(&self) -> &[u8] {
        self.mapping.as_deref().unwrap_or(&[])
    }
}

fn check_expected(actual: &str, snapshot: &Snapshot) -> Result<()> {
    if let Some(expected) = snapshot.manifest.used_blake3 {
        ensure!(
            actual == hex(expected),
            "used-prefix hash disagrees with snapshot manifest"
        );
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(4..=7).contains(&args.len()) {
        bail!("usage: page_delta BASE_PMA BASE_DESC TARGET_PMA TARGET_DESC [CHUNK_BYTES ... (max 3)]; defaults: 65536 1048576");
    }
    let chunks: Vec<usize> = if args.len() == 4 {
        vec![65536, 1048576]
    } else {
        args[4..]
            .iter()
            .map(|s| s.parse().context("parse chunk bytes"))
            .collect::<Result<_>>()?
    };
    for &size in &chunks {
        ensure!(
            size >= 8 && size % 8 == 0,
            "chunk size must be a positive multiple of 8 bytes"
        );
    }
    let base = Input::open(&args[0], &args[1])?;
    let target = Input::open(&args[2], &args[3])?;
    println!(
        "{}",
        json!({
            "stage":"byte_layout_control_open",
            "semantics":"Complete target used-prefix bytes plus target descriptor; raw pointer offsets preserved. Not a logical DAG delta.",
            "artifact":"No delta payload written; new-byte pieces borrow immutable target for measurement.",
            "base_manifest":base.snapshot.manifest, "base_footer":base.snapshot.metadata,
            "target_manifest":target.snapshot.manifest, "target_footer":target.snapshot.metadata,
            "chunks":chunks,
        })
    );
    for chunk_bytes in chunks {
        let started = Instant::now();
        let (view, stats) = compare(base.bytes(), target.bytes(), chunk_bytes)?;
        check_expected(&stats.base_used_blake3, &base.snapshot)?;
        check_expected(&stats.target_used_blake3, &target.snapshot)?;
        let walk_started = Instant::now();
        let walk = check_root(&view, &target.snapshot, 4096)?;
        println!(
            "{}",
            json!({
                "stage":"byte_layout_control_complete", "seconds":started.elapsed().as_secs_f64(),
                "stats":stats, "bounded_root_walk":walk,
                "root_walk_seconds":walk_started.elapsed().as_secs_f64(),
                "manifest_hashes_checked":{"base":base.snapshot.manifest.used_blake3.is_some(),"target":target.snapshot.manifest.used_blake3.is_some()},
                "virtual_target_checksum_valid":true,
            })
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligned_relocation_new_and_partial_chunks_reconstruct_exactly() {
        let base = b"AAAABBBBCCCCxy";
        let target = b"AAAACCCCZZZZBBBBxy";
        let (view, stats) = compare(base, target, 4).unwrap();
        assert_eq!(stats.aligned_reused_bytes, 4);
        assert_eq!(stats.content_addressed_reused_bytes, 14);
        assert_eq!(stats.new_bytes, 4);
        assert_eq!(stats.relocated_reused_bytes, 10);
        let mut all = vec![0; target.len()];
        view.read_exact(0, &mut all).unwrap();
        assert_eq!(all, target);
        let mut spanning = [0; 11];
        view.read_exact(3, &mut spanning).unwrap();
        assert_eq!(&spanning, &target[3..14]);
        assert!(view.read_exact(target.len(), &mut [0]).is_err());
        assert_eq!(view.checksum(), *blake3::hash(target).as_bytes());
    }

    #[test]
    fn empty_and_shorter_targets_preserve_exact_length() {
        let (empty, stats) = compare(b"base", b"", 4).unwrap();
        assert_eq!(stats.new_bytes, 0);
        assert!(empty.pieces.is_empty());
        assert_eq!(empty.checksum(), *blake3::hash(b"").as_bytes());
        let (short, stats) = compare(b"AAAABBBB", b"AAA", 4).unwrap();
        assert_eq!(stats.aligned_reused_bytes, 3);
        assert_eq!(short.chunk(0), b"AAA");
        assert!(compare(b"", b"", 0).is_err());
    }

    #[test]
    fn real_noun_root_is_read_through_mixed_virtual_chunks_without_mutation() {
        use common::{Fixture, Layout, Model, Scratch};
        let dir = Scratch::new();
        let mut model = Model::default();
        let number = model.number(42);
        let atom = model.atom(&[0x81; 137]);
        let root = model.cell(number, atom);
        let a = Fixture::write(dir.path(), "a", &model, root, Layout::default());
        let b = Fixture::write(
            dir.path(),
            "b",
            &model,
            root,
            Layout {
                padding_words: 19,
                metadata: 123,
                ..Default::default()
            },
        );
        let base = Input::open(a.pma.to_str().unwrap(), a.manifest.to_str().unwrap()).unwrap();
        let target = Input::open(b.pma.to_str().unwrap(), b.manifest.to_str().unwrap()).unwrap();
        for size in [8, 64, 1024] {
            let (view, stats) = compare(base.bytes(), target.bytes(), size).unwrap();
            check_expected(&stats.base_used_blake3, &base.snapshot).unwrap();
            check_expected(&stats.target_used_blake3, &target.snapshot).unwrap();
            let walk = check_root(&view, &target.snapshot, 4096).unwrap();
            assert_eq!(
                (walk.cells, walk.indirect_atoms, walk.pending_handles),
                (1, 1, 0)
            );
            assert!(check_expected(&hex([0; 32]), &target.snapshot).is_err());
        }
        a.assert_unchanged();
        b.assert_unchanged();
    }
}
