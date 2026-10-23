//! Read-only PMA noun access. Input files must remain immutable while mapped.
//! Metadata/mug words are never read as noun values and are never modified.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use bincode::{Decode, Encode};
use memmap2::{Mmap, MmapOptions};
use serde::Serialize;

pub type RawNoun = u64;
const LOCATION: u64 = 1 << 60;
const DIRECT_MASK: u64 = 1 << 63;
const INDIRECT_MASK: u64 = 3 << 62;
const CELL_MASK: u64 = 7 << 61;
const CELL_TAG: u64 = 3 << 62;
const PMA_MAGIC: u64 = u64::from_le_bytes(*b"NOCKPMA1");
const TRAILER_MAGIC: u64 = u64::from_le_bytes(*b"NOCKPM2!");
const MANIFEST_MAGIC: u64 = u64::from_le_bytes(*b"SNAPMAN1");
const PERSIST_MAGIC: u64 = u64::from_le_bytes(*b"PMAPERS1");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Node<'a> {
    Direct(u64),
    /// Significant little-endian bytes; no trailing zero bytes.
    Atom(&'a [u8]),
    Cell {
        head: RawNoun,
        tail: RawNoun,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct PmaMetadata {
    pub version: u64,
    pub capacity_words: u64,
    pub alloc_words: u64,
    pub reserved_words: u64,
    pub file_bytes: u64,
    pub physical_file_bytes: Option<u64>,
}

/// Common verified envelope for snapshots and operative PMAs. Operative
/// sidecars protect their fields, but do not contain a PMA used-range digest.
#[derive(Clone, Debug, Serialize)]
pub struct Manifest {
    pub source: String,
    pub version: u32,
    pub timestamp_tag: Option<String>,
    pub ker_hash: [u8; 32],
    pub event_num: u64,
    pub pma_words: u64,
    pub alloc_words: u64,
    pub kernel_root_raw: RawNoun,
    pub cold_offset: Option<u64>,
    pub used_blake3: Option<[u8; 32]>,
    pub structure_blake3: Option<[u8; 32]>,
    pub created_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Encode, Decode)]
enum SnapshotKind {
    Epoch,
    Rotating,
}

#[derive(Clone, Debug, Encode, Decode)]
struct WirePayload {
    magic: u64,
    version: u32,
    kind: SnapshotKind,
    timestamp_tag: String,
    ker_hash: [u8; 32],
    event_num: u64,
    pma_words: u64,
    alloc_words: u64,
    kernel_root_raw: u64,
    cold_offset: u64,
    used_blake3: [u8; 32],
    structure_blake3: Option<[u8; 32]>,
    created_at_ms: i64,
}

// Nested fields have no framing in bincode: this is byte-identical to the
// production flat SnapshotManifest ending in its fixed [u8;32] checksum.
#[derive(Encode, Decode)]
struct WireManifest {
    payload: WirePayload,
    checksum: [u8; 32],
}

#[derive(Encode, Decode)]
struct PersistV5 {
    magic: u64,
    version: u32,
    #[bincode(with_serde)]
    ker_hash: blake3::Hash,
    event_num: u64,
    kernel_state_raw: u64,
    pma_reserved_words: u64,
    #[bincode(with_serde)]
    checksum: blake3::Hash,
}

#[derive(Encode, Decode)]
struct PersistV4 {
    magic: u64,
    version: u32,
    #[bincode(with_serde)]
    ker_hash: blake3::Hash,
    event_num: u64,
    kernel_state_raw: u64,
    #[bincode(with_serde)]
    checksum: blake3::Hash,
}

pub struct Snapshot {
    pub manifest: Manifest,
    pub metadata: PmaMetadata,
    // Keep the read-only descriptor alive with the mapping. No writable handle
    // or writable mmap is ever created by this module.
    _file: File,
    used: Option<Mmap>,
}

impl Snapshot {
    pub fn open(pma_path: impl AsRef<Path>, manifest_path: impl AsRef<Path>) -> Result<Self> {
        let bytes = small_file(manifest_path.as_ref())?;
        let (wire, consumed): (WireManifest, usize) =
            bincode::decode_from_slice(&bytes, bincode::config::standard().with_limit::<65536>())
                .context("decode snapshot manifest")?;
        ensure!(
            consumed == bytes.len(),
            "trailing bytes in snapshot manifest"
        );
        let payload = wire.payload;
        ensure!(
            payload.magic == MANIFEST_MAGIC && payload.version == 2,
            "unsupported snapshot manifest magic/version"
        );
        let encoded = bincode::encode_to_vec(&payload, bincode::config::standard())?;
        ensure!(
            blake3::hash(&encoded).as_bytes() == &wire.checksum,
            "snapshot manifest checksum mismatch"
        );
        let file = File::open(pma_path.as_ref()).context("open PMA read-only")?;
        let metadata = read_metadata(&file)?;
        ensure!(
            payload.pma_words == metadata.capacity_words,
            "manifest/footer capacity mismatch"
        );
        ensure!(
            payload.alloc_words == metadata.alloc_words,
            "manifest/footer allocation mismatch"
        );
        ensure!(
            payload.cold_offset < metadata.alloc_words,
            "manifest cold offset outside used prefix"
        );
        let manifest = Manifest {
            source: match payload.kind {
                SnapshotKind::Epoch => "epoch",
                SnapshotKind::Rotating => "rotating",
            }
            .into(),
            version: payload.version,
            timestamp_tag: Some(payload.timestamp_tag),
            ker_hash: payload.ker_hash,
            event_num: payload.event_num,
            pma_words: payload.pma_words,
            alloc_words: payload.alloc_words,
            kernel_root_raw: payload.kernel_root_raw,
            cold_offset: Some(payload.cold_offset),
            used_blake3: Some(payload.used_blake3),
            structure_blake3: payload.structure_blake3,
            created_at_ms: Some(payload.created_at_ms),
        };
        Self::map(file, metadata, manifest)
    }

    pub fn open_operative(pma_path: impl AsRef<Path>, meta_path: impl AsRef<Path>) -> Result<Self> {
        let bytes = small_file(meta_path.as_ref())?;
        let file = File::open(pma_path.as_ref()).context("open operative PMA read-only")?;
        let metadata = read_metadata(&file)?;
        let ((magic, meta_version), _header_bytes): ((u64, u32), usize) =
            bincode::decode_from_slice(&bytes, bincode::config::standard().with_limit::<65536>())?;
        ensure!(magic == PERSIST_MAGIC, "invalid operative metadata magic");
        let (ker_hash, event_num, root) = match meta_version {
            5 => {
                let (meta, consumed): (PersistV5, usize) = bincode::decode_from_slice(
                    &bytes,
                    bincode::config::standard().with_limit::<65536>(),
                )?;
                ensure!(
                    consumed == bytes.len(),
                    "trailing bytes in operative v5 metadata"
                );
                let mut hash = blake3::Hasher::new();
                hash.update(meta.ker_hash.as_bytes());
                hash.update(&meta.event_num.to_le_bytes());
                hash.update(&meta.kernel_state_raw.to_le_bytes());
                hash.update(&meta.pma_reserved_words.to_le_bytes());
                ensure!(
                    hash.finalize() == meta.checksum,
                    "operative v5 metadata checksum mismatch"
                );
                ensure!(
                    meta.pma_reserved_words == 0
                        || meta.pma_reserved_words == metadata.reserved_words,
                    "operative reservation mismatch"
                );
                (
                    *meta.ker_hash.as_bytes(),
                    meta.event_num,
                    meta.kernel_state_raw,
                )
            }
            4 => {
                let (meta, consumed): (PersistV4, usize) = bincode::decode_from_slice(
                    &bytes,
                    bincode::config::standard().with_limit::<65536>(),
                )?;
                ensure!(
                    consumed == bytes.len(),
                    "trailing bytes in operative v4 metadata"
                );
                let mut hash = blake3::Hasher::new();
                hash.update(meta.ker_hash.as_bytes());
                hash.update(&meta.event_num.to_le_bytes());
                hash.update(&meta.kernel_state_raw.to_le_bytes());
                ensure!(
                    hash.finalize() == meta.checksum,
                    "operative v4 metadata checksum mismatch"
                );
                (
                    *meta.ker_hash.as_bytes(),
                    meta.event_num,
                    meta.kernel_state_raw,
                )
            }
            _ => bail!("unsupported operative metadata version {meta_version}"),
        };
        let manifest = Manifest {
            source: "operative".into(),
            version: meta_version,
            timestamp_tag: None,
            ker_hash,
            event_num,
            pma_words: metadata.capacity_words,
            alloc_words: metadata.alloc_words,
            kernel_root_raw: root,
            cold_offset: None,
            used_blake3: None,
            structure_blake3: None,
            created_at_ms: None,
        };
        Self::map(file, metadata, manifest)
    }

    fn map(file: File, metadata: PmaMetadata, manifest: Manifest) -> Result<Self> {
        let used_bytes = metadata
            .alloc_words
            .checked_mul(8)
            .context("used bytes overflow")?;
        let len = usize::try_from(used_bytes).context("used prefix exceeds address space")?;
        let used = if len == 0 {
            None
        } else {
            // SAFETY: file is opened read-only; caller supplies immutable backup
            // artifacts and keeps them unmodified/untruncated for this lifetime.
            Some(
                unsafe { MmapOptions::new().len(len).map(&file) }
                    .context("map used PMA prefix read-only")?,
            )
        };
        let snapshot = Self {
            manifest,
            metadata,
            _file: file,
            used,
        };
        snapshot
            .node(snapshot.root())
            .context("validate root noun")?;
        Ok(snapshot)
    }

    pub fn root(&self) -> RawNoun {
        self.manifest.kernel_root_raw
    }
    pub fn mapped_bytes(&self) -> usize {
        self.bytes().len()
    }
    fn bytes(&self) -> &[u8] {
        self.used.as_deref().unwrap_or(&[])
    }

    /// Explicit, separately timed integrity scan. This is unavailable for
    /// operative files because their sidecars contain no PMA content digest.
    pub fn verify_used_hash(&self) -> Result<()> {
        let expected = self
            .manifest
            .used_blake3
            .context("operative PMA has no stored used-range hash")?;
        ensure!(
            blake3::hash(self.bytes()).as_bytes() == &expected,
            "PMA used-range hash mismatch"
        );
        Ok(())
    }

    pub fn node(&self, raw: RawNoun) -> Result<Node<'_>> {
        if raw & DIRECT_MASK == 0 {
            return Ok(Node::Direct(raw));
        }
        ensure!(raw & LOCATION != 0, "non-offset pointer noun {raw:#018x}");
        if raw & INDIRECT_MASK == DIRECT_MASK {
            let offset = raw & !(INDIRECT_MASK | LOCATION);
            let header = self.word_range(offset, 2)?;
            let size = u64::from_le_bytes(header[8..16].try_into().unwrap());
            ensure!(size > 0, "zero-length indirect atom at {offset}");
            let start = offset.checked_add(2).context("atom offset overflow")?;
            let bytes = self.word_range(start, size)?;
            let significant = bytes
                .iter()
                .rposition(|byte| *byte != 0)
                .map_or(0, |last| last + 1);
            let bytes = &bytes[..significant];
            // Keep allocated-handle identity separate from direct raw values.
            // Value normalization is represented by trimmed atom bytes, even
            // when an indirect allocation stores zero or a small integer.
            return Ok(Node::Atom(bytes));
        }
        if raw & CELL_MASK == CELL_TAG {
            let offset = raw & !(CELL_MASK | LOCATION);
            let bytes = self.word_range(offset, 3)?;
            return Ok(Node::Cell {
                head: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
                tail: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            });
        }
        bail!("forwarding or invalid noun tag {raw:#018x}")
    }

    fn word_range(&self, offset: u64, words: u64) -> Result<&[u8]> {
        let end = offset
            .checked_add(words)
            .context("PMA word range overflow")?;
        ensure!(
            end <= self.metadata.alloc_words,
            "PMA object outside allocated prefix: offset={offset} words={words} alloc={}",
            self.metadata.alloc_words
        );
        let start = usize::try_from(offset.checked_mul(8).context("byte offset overflow")?)?;
        let end = usize::try_from(end.checked_mul(8).context("byte end overflow")?)?;
        self.bytes()
            .get(start..end)
            .context("PMA byte range outside mapping")
    }

    /// Nock axis lookup, iterative and side-effect-free. Axis 1 is identity.
    pub fn slot(&self, mut root: RawNoun, axis: u64) -> Result<RawNoun> {
        ensure!(axis != 0, "axis zero has no slot");
        let mut bit = (1u64 << (63 - axis.leading_zeros())) >> 1;
        while bit != 0 {
            let Node::Cell { head, tail } = self.node(root)? else {
                bail!("axis {axis} traverses an atom");
            };
            root = if axis & bit == 0 { head } else { tail };
            bit >>= 1;
        }
        Ok(root)
    }
}

fn small_file(path: &Path) -> Result<Vec<u8>> {
    ensure!(
        fs::metadata(path)?.len() <= 65536,
        "metadata file exceeds 64 KiB"
    );
    Ok(fs::read(path).with_context(|| format!("read {}", path.display()))?)
}

fn read_metadata(file: &File) -> Result<PmaMetadata> {
    let stat = file.metadata()?;
    let len = stat.len();
    ensure!(stat.is_file() && len >= 32, "not a PMA regular file");
    let mut reader = file;
    let mut trailer = [0u8; 64];
    if len >= 64 {
        reader.seek(SeekFrom::End(-64))?;
        reader.read_exact(&mut trailer)?;
    }
    let word =
        |index: usize| u64::from_le_bytes(trailer[index * 8..index * 8 + 8].try_into().unwrap());
    let (version, capacity_words, alloc_words, reserved_words, trailer_len) = if len >= 64
        && word(0) == TRAILER_MAGIC
    {
        let version = word(1);
        let reserved = word(2);
        let capacity = word(6);
        let alloc = word(7);
        let checksum = TRAILER_MAGIC
            ^ version.rotate_left(5)
            ^ reserved.rotate_left(13)
            ^ capacity.rotate_left(29)
            ^ alloc.rotate_left(43);
        ensure!(
            version == 2 && word(4) == PMA_MAGIC && word(5) == 2 && word(3) == checksum,
            "invalid PMA v2 trailer/checksum"
        );
        (version, capacity, alloc, reserved, 64)
    } else {
        reader.seek(SeekFrom::End(-32))?;
        let mut footer = [0u8; 32];
        reader.read_exact(&mut footer)?;
        let word =
            |index: usize| u64::from_le_bytes(footer[index * 8..index * 8 + 8].try_into().unwrap());
        ensure!(
            word(0) == PMA_MAGIC && word(1) == 1,
            "no valid PMA v1/v2 footer"
        );
        (1, word(2), word(3), word(2), 32)
    };
    ensure!(
        alloc_words <= capacity_words && capacity_words <= reserved_words,
        "invalid PMA allocation/capacity/reservation"
    );
    ensure!(
        capacity_words
            .checked_mul(8)
            .and_then(|bytes| bytes.checked_add(trailer_len))
            == Some(len),
        "PMA capacity disagrees with physical EOF"
    );
    #[cfg(unix)]
    let physical_file_bytes = {
        use std::os::unix::fs::MetadataExt;
        stat.blocks().checked_mul(512)
    };
    #[cfg(not(unix))]
    let physical_file_bytes = None;
    Ok(PmaMetadata {
        version,
        capacity_words,
        alloc_words,
        reserved_words,
        file_bytes: len,
        physical_file_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "pma-reader-unit-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn v2_pma(path: &Path) -> Vec<u8> {
        let (capacity, allocated, reserved) = (1024u64, 1u64, 2048u64);
        let checksum = TRAILER_MAGIC
            ^ 2u64.rotate_left(5)
            ^ reserved.rotate_left(13)
            ^ capacity.rotate_left(29)
            ^ allocated.rotate_left(43);
        let mut bytes = vec![0; capacity as usize * 8];
        for word in [
            TRAILER_MAGIC,
            2,
            reserved,
            checksum,
            PMA_MAGIC,
            2,
            capacity,
            allocated,
        ] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        fs::write(path, &bytes).unwrap();
        bytes
    }

    #[test]
    fn operative_v4_v5_validate_sidecar_and_preserve_source_bytes() {
        let scratch = Scratch::new();
        let pma = scratch.0.join("1.pma");
        let original = v2_pma(&pma);
        let meta = scratch.0.join("1.meta");
        let ker_hash = blake3::hash(b"kernel");
        let event_num = 7_433_651u64;
        let root = 42u64;
        for version in [4, 5] {
            let mut checksum = blake3::Hasher::new();
            checksum.update(ker_hash.as_bytes());
            checksum.update(&event_num.to_le_bytes());
            checksum.update(&root.to_le_bytes());
            let encoded = if version == 5 {
                checksum.update(&2048u64.to_le_bytes());
                bincode::encode_to_vec(
                    PersistV5 {
                        magic: PERSIST_MAGIC,
                        version,
                        ker_hash,
                        event_num,
                        kernel_state_raw: root,
                        pma_reserved_words: 2048,
                        checksum: checksum.finalize(),
                    },
                    bincode::config::standard(),
                )
                .unwrap()
            } else {
                bincode::encode_to_vec(
                    PersistV4 {
                        magic: PERSIST_MAGIC,
                        version,
                        ker_hash,
                        event_num,
                        kernel_state_raw: root,
                        checksum: checksum.finalize(),
                    },
                    bincode::config::standard(),
                )
                .unwrap()
            };
            fs::write(&meta, &encoded).unwrap();
            let snapshot = Snapshot::open_operative(&pma, &meta).unwrap();
            assert_eq!(snapshot.root(), root);
            assert_eq!(snapshot.node(root).unwrap(), Node::Direct(42));
            assert_eq!(snapshot.manifest.event_num, event_num);
            assert_eq!(snapshot.manifest.ker_hash, *ker_hash.as_bytes());
            assert_eq!(snapshot.mapped_bytes(), 8);
            assert!(snapshot.verify_used_hash().is_err());
            assert!(snapshot.node(CELL_TAG | LOCATION).is_err());
            assert!(snapshot.node(CELL_TAG).is_err());
            assert!(snapshot.node(CELL_MASK | LOCATION).is_err());
            assert_eq!(fs::read(&pma).unwrap(), original);
            assert_eq!(fs::read(&meta).unwrap(), encoded);
            let mut corrupt = encoded;
            *corrupt.last_mut().unwrap() ^= 1;
            fs::write(&meta, corrupt).unwrap();
            assert!(Snapshot::open_operative(&pma, &meta).is_err());
        }
    }

    #[test]
    fn v2_footer_checksum_and_capacity_are_checked() {
        let scratch = Scratch::new();
        let path = scratch.0.join("snapshot.pma");
        let mut bytes = v2_pma(&path);
        let metadata = read_metadata(&File::open(&path).unwrap()).unwrap();
        assert_eq!(metadata.alloc_words, 1);
        assert_eq!(metadata.capacity_words, 1024);
        let checksum_byte = bytes.len() - 64 + 24;
        bytes[checksum_byte] ^= 1;
        fs::write(&path, &bytes).unwrap();
        assert!(read_metadata(&File::open(&path).unwrap()).is_err());
        bytes[checksum_byte] ^= 1;
        bytes.remove(0);
        fs::write(&path, bytes).unwrap();
        assert!(read_metadata(&File::open(&path).unwrap()).is_err());
    }
}
