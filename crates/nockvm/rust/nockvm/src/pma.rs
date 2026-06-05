//! Persistent Memory Arena (PMA)
//!
//! The PMA is a file-backed memory region for storing long-lived Nouns.
//! It uses bump allocation and stores nouns in offset form.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::ptr::copy_nonoverlapping;
use std::sync::Arc;
use std::time::{Duration, Instant};

use either::Either::{Left, Right};
#[cfg(feature = "pma-assert")]
use intmap::IntMap;
#[cfg(unix)]
use libc;
use smallvec::SmallVec;
use thiserror::Error;
use tracing::{debug, info};

use crate::ext::noun_equality;
use crate::mem::{word_size_of, Arena, NewStackError, NockStack};
use crate::noun::{
    AllocLocation, Atom, Cell, CellMemory, IndirectAtom, Noun, NounAllocator, NounRepr, NounSpace,
};
use crate::offset::PmaOffsetWords;

mod stream;
pub use stream::{
    classify_pma_noun, PmaDirectJamConfig, PmaDirectJamError, PmaDirectReader, PmaRawNounKind,
};

const PMA_MAGIC: u64 = u64::from_le_bytes(*b"NOCKPMA1");
const PMA_VERSION_V1: u64 = 1;
const PMA_VERSION: u64 = 2;
const PMA_V2_TRAILER_MAGIC: u64 = u64::from_le_bytes(*b"NOCKPM2!");
const PMA_GROWTH_JOURNAL_MAGIC: u64 = u64::from_le_bytes(*b"PMAGROW1");
const PMA_MIGRATION_JOURNAL_MAGIC: u64 = u64::from_le_bytes(*b"PMAMIGR2");
const DEFAULT_PMA_RESERVED_BYTES: usize = 1 << 40; // 1 TiB virtual reservation.
const NOCK_PMA_RESERVED_WORDS_ENV: &str = "NOCK_PMA_RESERVED_WORDS";
const NOCK_PMA_GROWTH_EVENTS_PATH_ENV: &str = "NOCK_PMA_GROWTH_EVENTS_PATH";
const NOCK_PMA_RESIZE_FAIL_AT_ENV: &str = "NOCK_PMA_RESIZE_FAIL_AT";
const NOCK_PMA_MIGRATION_FAIL_AT_ENV: &str = "NOCK_PMA_MIGRATION_FAIL_AT";
const NOCK_PMA_DISABLE_RESIZE_ENV: &str = "NOCK_PMA_DISABLE_RESIZE_FOR_REGRESSION";

#[cfg(test)]
thread_local! {
    static TEST_GROWTH_FAILURE_POINT: std::cell::RefCell<Option<&'static str>> =
        const { std::cell::RefCell::new(None) };
    static TEST_MIGRATION_FAILURE_POINT: std::cell::RefCell<Option<&'static str>> =
        const { std::cell::RefCell::new(None) };
}

/// The metadata for the PMA is a trailer or footer because otherwise the base + offset pointer derivations would need
/// to account for the footer size. With this design it's just base pointer + offset.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct PmaLegacyTrailer {
    magic: u64,
    version: u64,
    data_words: u64,
    alloc_offset: u64,
}

const PMA_LEGACY_TRAILER_BYTES: usize = std::mem::size_of::<PmaLegacyTrailer>();
const PMA_TRAILER_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PmaFileMetadata {
    pub magic: u64,
    pub version: u64,
    pub data_words: u64,
    pub alloc_words: u64,
    pub capacity_words: u64,
    pub free_words: u64,
    pub reserved_words: u64,
    pub file_bytes: u64,
    pub apparent_file_bytes: u64,
    pub physical_file_bytes: Option<u64>,
}

impl PmaLegacyTrailer {
    fn to_bytes(self) -> [u8; PMA_LEGACY_TRAILER_BYTES] {
        let mut buf = [0u8; PMA_LEGACY_TRAILER_BYTES];
        buf[0..8].copy_from_slice(&self.magic.to_le_bytes());
        buf[8..16].copy_from_slice(&self.version.to_le_bytes());
        buf[16..24].copy_from_slice(&self.data_words.to_le_bytes());
        buf[24..32].copy_from_slice(&self.alloc_offset.to_le_bytes());
        buf
    }

    fn from_bytes(buf: [u8; PMA_LEGACY_TRAILER_BYTES]) -> Self {
        let magic = u64::from_le_bytes(buf[0..8].try_into().expect("magic slice"));
        let version = u64::from_le_bytes(buf[8..16].try_into().expect("version slice"));
        let data_words = u64::from_le_bytes(buf[16..24].try_into().expect("data_words slice"));
        let alloc_offset = u64::from_le_bytes(buf[24..32].try_into().expect("alloc_offset slice"));
        Self {
            magic,
            version,
            data_words,
            alloc_offset,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PmaTrailerV2 {
    reserved_words: u64,
    footer: PmaLegacyTrailer,
}

impl PmaTrailerV2 {
    fn new(capacity_words: u64, alloc_words: u64, reserved_words: u64) -> Self {
        Self {
            reserved_words,
            footer: PmaLegacyTrailer {
                magic: PMA_MAGIC,
                version: PMA_VERSION,
                data_words: capacity_words,
                alloc_offset: alloc_words,
            },
        }
    }

    fn checksum(version: u64, reserved_words: u64, capacity_words: u64, alloc_words: u64) -> u64 {
        PMA_V2_TRAILER_MAGIC
            ^ version.rotate_left(5)
            ^ reserved_words.rotate_left(13)
            ^ capacity_words.rotate_left(29)
            ^ alloc_words.rotate_left(43)
    }

    fn to_bytes(self) -> [u8; PMA_TRAILER_BYTES] {
        let mut buf = [0u8; PMA_TRAILER_BYTES];
        let checksum = Self::checksum(
            PMA_VERSION, self.reserved_words, self.footer.data_words, self.footer.alloc_offset,
        );
        buf[0..8].copy_from_slice(&PMA_V2_TRAILER_MAGIC.to_le_bytes());
        buf[8..16].copy_from_slice(&PMA_VERSION.to_le_bytes());
        buf[16..24].copy_from_slice(&self.reserved_words.to_le_bytes());
        buf[24..32].copy_from_slice(&checksum.to_le_bytes());
        buf[32..64].copy_from_slice(&self.footer.to_bytes());
        buf
    }

    fn from_bytes(buf: [u8; PMA_TRAILER_BYTES]) -> Option<Self> {
        let extension_magic = u64::from_le_bytes(buf[0..8].try_into().ok()?);
        let version = u64::from_le_bytes(buf[8..16].try_into().ok()?);
        let reserved_words = u64::from_le_bytes(buf[16..24].try_into().ok()?);
        let checksum = u64::from_le_bytes(buf[24..32].try_into().ok()?);
        let mut footer_buf = [0u8; PMA_LEGACY_TRAILER_BYTES];
        footer_buf.copy_from_slice(&buf[32..64]);
        let footer = PmaLegacyTrailer::from_bytes(footer_buf);
        let expected_checksum = Self::checksum(
            version, reserved_words, footer.data_words, footer.alloc_offset,
        );
        (extension_magic == PMA_V2_TRAILER_MAGIC
            && version == PMA_VERSION
            && footer.magic == PMA_MAGIC
            && footer.version == PMA_VERSION
            && footer.alloc_offset <= footer.data_words
            && footer.data_words <= reserved_words
            && checksum == expected_checksum)
            .then_some(Self {
                reserved_words,
                footer,
            })
    }
}

#[derive(Clone, Copy, Debug)]
struct PmaGrowthJournal {
    magic: u64,
    old_data_words: u64,
    old_alloc_words: u64,
    new_data_words: u64,
    reserved_words: u64,
    checksum: u64,
}

#[derive(Clone, Copy, Debug)]
struct PmaGrowthEvent {
    old_words: usize,
    new_words: usize,
    alloc_words: usize,
    required_words: usize,
    request_words: usize,
    context: &'static str,
    elapsed: Duration,
}

impl PmaGrowthJournal {
    const BYTES: usize = 48;
    const LEGACY_BYTES: usize = 40;

    fn new(
        old_data_words: u64,
        old_alloc_words: u64,
        new_data_words: u64,
        reserved_words: u64,
    ) -> Self {
        let checksum = Self::checksum(
            old_data_words, old_alloc_words, new_data_words, reserved_words,
        );
        Self {
            magic: PMA_GROWTH_JOURNAL_MAGIC,
            old_data_words,
            old_alloc_words,
            new_data_words,
            reserved_words,
            checksum,
        }
    }

    fn checksum(
        old_data_words: u64,
        old_alloc_words: u64,
        new_data_words: u64,
        reserved_words: u64,
    ) -> u64 {
        PMA_GROWTH_JOURNAL_MAGIC
            ^ old_data_words.rotate_left(7)
            ^ old_alloc_words.rotate_left(17)
            ^ new_data_words.rotate_left(31)
            ^ reserved_words.rotate_left(47)
    }

    fn to_bytes(self) -> [u8; Self::BYTES] {
        let mut buf = [0u8; Self::BYTES];
        buf[0..8].copy_from_slice(&self.magic.to_le_bytes());
        buf[8..16].copy_from_slice(&self.old_data_words.to_le_bytes());
        buf[16..24].copy_from_slice(&self.old_alloc_words.to_le_bytes());
        buf[24..32].copy_from_slice(&self.new_data_words.to_le_bytes());
        buf[32..40].copy_from_slice(&self.reserved_words.to_le_bytes());
        buf[40..48].copy_from_slice(&self.checksum.to_le_bytes());
        buf
    }

    fn from_bytes(buf: [u8; Self::BYTES]) -> Option<Self> {
        let magic = u64::from_le_bytes(buf[0..8].try_into().ok()?);
        let old_data_words = u64::from_le_bytes(buf[8..16].try_into().ok()?);
        let old_alloc_words = u64::from_le_bytes(buf[16..24].try_into().ok()?);
        let new_data_words = u64::from_le_bytes(buf[24..32].try_into().ok()?);
        let reserved_words = u64::from_le_bytes(buf[32..40].try_into().ok()?);
        let checksum = u64::from_le_bytes(buf[40..48].try_into().ok()?);
        let journal = Self {
            magic,
            old_data_words,
            old_alloc_words,
            new_data_words,
            reserved_words,
            checksum,
        };
        journal.validate().then_some(journal)
    }

    fn from_legacy_bytes(buf: [u8; Self::LEGACY_BYTES]) -> Option<Self> {
        let magic = u64::from_le_bytes(buf[0..8].try_into().ok()?);
        let old_data_words = u64::from_le_bytes(buf[8..16].try_into().ok()?);
        let old_alloc_words = u64::from_le_bytes(buf[16..24].try_into().ok()?);
        let new_data_words = u64::from_le_bytes(buf[24..32].try_into().ok()?);
        let legacy_checksum = u64::from_le_bytes(buf[32..40].try_into().ok()?);
        let expected_legacy_checksum = PMA_GROWTH_JOURNAL_MAGIC
            ^ old_data_words.rotate_left(7)
            ^ old_alloc_words.rotate_left(17)
            ^ new_data_words.rotate_left(31);
        if magic != PMA_GROWTH_JOURNAL_MAGIC
            || old_alloc_words > old_data_words
            || old_data_words > new_data_words
            || legacy_checksum != expected_legacy_checksum
        {
            return None;
        }
        Some(Self::new(
            old_data_words, old_alloc_words, new_data_words, new_data_words,
        ))
    }

    fn validate(self) -> bool {
        self.magic == PMA_GROWTH_JOURNAL_MAGIC
            && self.old_alloc_words <= self.old_data_words
            && self.old_data_words <= self.new_data_words
            && self.new_data_words <= self.reserved_words
            && self.checksum
                == Self::checksum(
                    self.old_data_words, self.old_alloc_words, self.new_data_words,
                    self.reserved_words,
                )
    }
}

#[derive(Clone, Copy, Debug)]
struct PmaMigrationJournal {
    magic: u64,
    capacity_words: u64,
    alloc_words: u64,
    reserved_words: u64,
    checksum: u64,
}

impl PmaMigrationJournal {
    const BYTES: usize = 40;

    fn new(capacity_words: u64, alloc_words: u64, reserved_words: u64) -> Self {
        let checksum = Self::checksum(capacity_words, alloc_words, reserved_words);
        Self {
            magic: PMA_MIGRATION_JOURNAL_MAGIC,
            capacity_words,
            alloc_words,
            reserved_words,
            checksum,
        }
    }

    fn checksum(capacity_words: u64, alloc_words: u64, reserved_words: u64) -> u64 {
        PMA_MIGRATION_JOURNAL_MAGIC
            ^ capacity_words.rotate_left(11)
            ^ alloc_words.rotate_left(23)
            ^ reserved_words.rotate_left(37)
    }

    fn to_bytes(self) -> [u8; Self::BYTES] {
        let mut buf = [0u8; Self::BYTES];
        buf[0..8].copy_from_slice(&self.magic.to_le_bytes());
        buf[8..16].copy_from_slice(&self.capacity_words.to_le_bytes());
        buf[16..24].copy_from_slice(&self.alloc_words.to_le_bytes());
        buf[24..32].copy_from_slice(&self.reserved_words.to_le_bytes());
        buf[32..40].copy_from_slice(&self.checksum.to_le_bytes());
        buf
    }

    fn from_bytes(buf: [u8; Self::BYTES]) -> Option<Self> {
        let magic = u64::from_le_bytes(buf[0..8].try_into().ok()?);
        let capacity_words = u64::from_le_bytes(buf[8..16].try_into().ok()?);
        let alloc_words = u64::from_le_bytes(buf[16..24].try_into().ok()?);
        let reserved_words = u64::from_le_bytes(buf[24..32].try_into().ok()?);
        let checksum = u64::from_le_bytes(buf[32..40].try_into().ok()?);
        let journal = Self {
            magic,
            capacity_words,
            alloc_words,
            reserved_words,
            checksum,
        };
        journal.validate().then_some(journal)
    }

    fn validate(self) -> bool {
        self.magic == PMA_MIGRATION_JOURNAL_MAGIC
            && self.alloc_words <= self.capacity_words
            && self.capacity_words <= self.reserved_words
            && self.checksum
                == Self::checksum(self.capacity_words, self.alloc_words, self.reserved_words)
    }
}

/// Errors that can occur during PMA operations
#[derive(Debug, Error)]
pub enum PmaError {
    #[error("PMA is full, cannot allocate {requested} words (available: {available})")]
    OutOfMemory { requested: usize, available: usize },

    #[error("Failed to create arena: {0}")]
    ArenaError(#[from] NewStackError),

    #[error("PMA metadata IO failed: {0}")]
    MetadataIo(#[from] std::io::Error),

    #[error("Invalid PMA metadata: {0}")]
    InvalidMetadata(String),

    #[error("PMA growth failed: {0}")]
    GrowthFailed(String),

    #[error("PMA metadata migration failed: {0}")]
    MigrationFailed(String),
}

/// The Persistent Memory Arena
///
/// A bump-allocated memory region for storing nouns in offset form.
/// The PMA is backed by a file and can persist across program restarts.
///
/// "Bump-allocated" means allocation simply increments the `alloc_offset`
/// pointer by the requested size—there is no free list, no compaction, and
/// no mechanism to reclaim memory once allocated. This makes allocation
/// extremely fast (just a pointer bump) but means the PMA grows monotonically
/// until explicitly reset.
///
/// When a Noun that lives in the PMA needs to be modified, the workflow is:
/// 1. The Noun is read from the PMA (already in offset form)
/// 2. Modifications happen in the NockStack (ephemeral working memory)
/// 3. The modified Noun is copied back to the PMA via `copy_to_pma()`
///
/// Step 3 only allocates space for the Allocated subtrees that changed. For
/// example, if `[2 3]` becomes `[4 3]`:
/// - The Cell is Allocated, so a NEW cell is allocated in the PMA with head=4,
///   tail=3 with new DirectAtoms for the 4 and 3 since they are not Allocated.
/// - The old `[2 3]` cell remains in the PMA, untouched but now unreachable
///
/// For Allocated structures, unchanged subtrees that are already in PMA (offset
/// form) are reused without copying. If `[[1 2] 3]` becomes `[[1 2] 4]`:
/// - A NEW outer cell is allocated with tail=4
/// - The head still points to the existing `[1 2]` in PMA (no copy needed)
/// - Only the old outer cell becomes garbage; `[1 2]` is shared
///
/// This copy allocates fresh space in the PMA for the new version—the old
/// version is not overwritten or freed, it simply becomes unreachable garbage.
/// Garbage collection (Milestone 4) will eventually reclaim this dead space.
///
/// Currently Pma is only suitable for a single reader/writer. In the future,
/// `alloc_offset` will be changed to `AtomicUsize` to allow multiple readers.
///
/// For more information, see `open/docs/pma/DESIGN.md`.
pub struct Pma {
    /// The underlying arena for memory management and pointer resolution
    arena: Arc<Arena>,
    /// Current allocation offset in words (bump pointer)
    alloc_offset: usize,
    /// Path to the backing file (for future file-backed persistence)
    path: PathBuf,
    /// Set after a growth failure may have partially changed the file/mapping.
    growth_poisoned: bool,
}

impl Pma {
    pub fn read_file_metadata(path: &Path) -> Result<PmaFileMetadata, PmaError> {
        let mut file = std::fs::File::open(path)?;
        match Self::read_file_metadata_from_reader(&mut file) {
            Ok(metadata) => Ok(metadata),
            Err(err) => {
                if let Some(metadata) = Self::recover_metadata_from_migration_journal(path)? {
                    return Ok(metadata);
                }
                if let Some(metadata) = Self::recover_metadata_from_growth_journal(path)? {
                    return Ok(metadata);
                }
                Err(err)
            }
        }
    }

    fn read_file_metadata_from_reader(
        file: &mut std::fs::File,
    ) -> Result<PmaFileMetadata, PmaError> {
        let os_metadata = file.metadata()?;
        let file_len_u64 = os_metadata.len();
        let file_len = usize::try_from(file_len_u64)
            .map_err(|_| PmaError::InvalidMetadata("file is too large to map".to_string()))?;
        if file_len < PMA_LEGACY_TRAILER_BYTES {
            return Err(PmaError::InvalidMetadata(format!(
                "file too small: {file_len} bytes"
            )));
        }

        if file_len >= PMA_TRAILER_BYTES {
            let data_bytes = file_len - PMA_TRAILER_BYTES;
            if data_bytes.is_multiple_of(8) {
                let mut trailer_bytes = [0u8; PMA_TRAILER_BYTES];
                file.seek(SeekFrom::End(-(PMA_TRAILER_BYTES as i64)))?;
                file.read_exact(&mut trailer_bytes)?;
                if let Some(trailer) = PmaTrailerV2::from_bytes(trailer_bytes) {
                    let capacity_words = data_bytes >> 3;
                    return Self::metadata_from_v2_trailer(
                        trailer, capacity_words, file_len_u64, &os_metadata,
                    );
                }
            }
        }

        let data_bytes = file_len - PMA_LEGACY_TRAILER_BYTES;
        if data_bytes.is_multiple_of(8) {
            let mut trailer_bytes = [0u8; PMA_LEGACY_TRAILER_BYTES];
            file.seek(SeekFrom::End(-(PMA_LEGACY_TRAILER_BYTES as i64)))?;
            file.read_exact(&mut trailer_bytes)?;
            let trailer = PmaLegacyTrailer::from_bytes(trailer_bytes);
            if let Ok(metadata) = Self::metadata_from_legacy_trailer(
                trailer,
                data_bytes >> 3,
                file_len_u64,
                &os_metadata,
            ) {
                return Ok(metadata);
            }
        }

        // If a v1-to-v2 migration extended the file but crashed before publishing a valid
        // v2 trailer, the old v1 trailer is still at the old EOF: file_len - PMA_TRAILER_BYTES.
        if file_len >= PMA_TRAILER_BYTES {
            let data_bytes = file_len - PMA_TRAILER_BYTES;
            if data_bytes.is_multiple_of(8) {
                let mut trailer_bytes = [0u8; PMA_LEGACY_TRAILER_BYTES];
                file.seek(SeekFrom::Start(data_bytes as u64))?;
                file.read_exact(&mut trailer_bytes)?;
                let trailer = PmaLegacyTrailer::from_bytes(trailer_bytes);
                if let Ok(metadata) = Self::metadata_from_legacy_trailer(
                    trailer,
                    data_bytes >> 3,
                    file_len_u64,
                    &os_metadata,
                ) {
                    return Ok(metadata);
                }
            }
        }

        Err(PmaError::InvalidMetadata(
            "no valid PMA v2 or legacy trailer found".to_string(),
        ))
    }

    fn metadata_from_v2_trailer(
        trailer: PmaTrailerV2,
        data_words: usize,
        file_len_u64: u64,
        os_metadata: &fs::Metadata,
    ) -> Result<PmaFileMetadata, PmaError> {
        if trailer.footer.data_words as usize != data_words {
            return Err(PmaError::InvalidMetadata(format!(
                "metadata capacity_words {} does not match file ({data_words})",
                trailer.footer.data_words
            )));
        }
        Ok(PmaFileMetadata {
            magic: trailer.footer.magic,
            version: trailer.footer.version,
            data_words: trailer.footer.data_words,
            alloc_words: trailer.footer.alloc_offset,
            capacity_words: trailer.footer.data_words,
            free_words: trailer
                .footer
                .data_words
                .saturating_sub(trailer.footer.alloc_offset),
            reserved_words: trailer.reserved_words,
            file_bytes: file_len_u64,
            apparent_file_bytes: file_len_u64,
            physical_file_bytes: physical_file_bytes(os_metadata),
        })
    }

    fn metadata_from_legacy_trailer(
        trailer: PmaLegacyTrailer,
        data_words: usize,
        file_len_u64: u64,
        os_metadata: &fs::Metadata,
    ) -> Result<PmaFileMetadata, PmaError> {
        if trailer.magic != PMA_MAGIC {
            return Err(PmaError::InvalidMetadata("bad PMA magic".to_string()));
        }
        if trailer.version != PMA_VERSION_V1 {
            return Err(PmaError::InvalidMetadata(format!(
                "unsupported PMA version {}",
                trailer.version
            )));
        }
        if trailer.data_words as usize != data_words {
            return Err(PmaError::InvalidMetadata(format!(
                "metadata data_words {} does not match file ({data_words})",
                trailer.data_words
            )));
        }
        if trailer.alloc_offset > trailer.data_words {
            return Err(PmaError::InvalidMetadata(format!(
                "alloc_offset {} exceeds data_words {}",
                trailer.alloc_offset, trailer.data_words
            )));
        }
        Ok(PmaFileMetadata {
            magic: trailer.magic,
            version: trailer.version,
            data_words: trailer.data_words,
            alloc_words: trailer.alloc_offset,
            capacity_words: trailer.data_words,
            free_words: trailer.data_words.saturating_sub(trailer.alloc_offset),
            reserved_words: trailer.data_words,
            file_bytes: file_len_u64,
            apparent_file_bytes: file_len_u64,
            physical_file_bytes: physical_file_bytes(os_metadata),
        })
    }

    /// Return the default virtual reservation for a PMA with `capacity_words` current capacity.
    pub fn default_reserved_words_for_capacity(capacity_words: usize) -> usize {
        default_reserved_words(capacity_words)
    }

    /// Create a new PMA with the given size in words
    pub fn new(size_words: usize, path: PathBuf) -> Result<Self, PmaError> {
        let reserved_words = Self::default_reserved_words_for_capacity(size_words);
        Self::new_with_reserved(size_words, reserved_words, path)
    }

    /// Create a new PMA with explicit current capacity and virtual reservation.
    pub fn new_with_reserved(
        capacity_words: usize,
        reserved_words: usize,
        path: PathBuf,
    ) -> Result<Self, PmaError> {
        let reserved_words = reserved_words.max(capacity_words);
        let arena = Arena::allocate_growable_file(
            &path, capacity_words, reserved_words, PMA_TRAILER_BYTES,
        )?;
        let pma = Self {
            arena,
            alloc_offset: 0,
            path,
            growth_poisoned: false,
        };
        pma.persist_metadata();
        Ok(pma)
    }

    /// Open an existing PMA file without truncating it or raising its virtual reservation.
    pub fn open(path: PathBuf) -> Result<Self, PmaError> {
        Self::open_with_min_preserving_reservation(path, 0)
    }

    /// Open an existing PMA and ensure its current capacity is at least `min_words`, preserving
    /// the existing virtual reservation unless `min_words` requires raising it.
    pub fn open_with_min(path: PathBuf, min_words: usize) -> Result<Self, PmaError> {
        Self::open_with_min_preserving_reservation(path, min_words)
    }

    /// Open an existing PMA with an explicit minimum current capacity and virtual reservation.
    pub fn open_with_min_and_reserved(
        path: PathBuf,
        min_words: usize,
        reserved_words: usize,
    ) -> Result<Self, PmaError> {
        Self::open_with_min_inner(path, min_words, Some(reserved_words))
    }

    fn open_with_min_preserving_reservation(
        path: PathBuf,
        min_words: usize,
    ) -> Result<Self, PmaError> {
        Self::open_with_min_inner(path, min_words, None)
    }

    fn open_with_min_inner(
        path: PathBuf,
        min_words: usize,
        requested_reserved_words: Option<usize>,
    ) -> Result<Self, PmaError> {
        let metadata = Self::read_file_metadata(&path)?;
        let data_words = usize::try_from(metadata.data_words).map_err(|_| {
            PmaError::InvalidMetadata("PMA data_words exceeds usize addressable range".to_string())
        })?;
        let alloc_offset = usize::try_from(metadata.alloc_words).map_err(|_| {
            PmaError::InvalidMetadata(
                "PMA alloc_offset exceeds usize addressable range".to_string(),
            )
        })?;
        let existing_reserved_words =
            usize::try_from(metadata.reserved_words).unwrap_or(usize::MAX);
        let reserved_words = requested_reserved_words
            .unwrap_or(existing_reserved_words)
            .max(existing_reserved_words)
            .max(data_words)
            .max(min_words);
        if metadata.version == PMA_VERSION_V1 {
            Self::migrate_legacy_metadata(&path, data_words, alloc_offset, reserved_words)?;
        }
        let arena = Arena::open_growable_file(
            &path,
            data_words,
            reserved_words.max(data_words),
            PMA_TRAILER_BYTES,
        )?;
        let mut pma = Self {
            arena,
            alloc_offset,
            path,
            growth_poisoned: false,
        };
        if pma.size_words() < min_words && !pma_growth_disabled_for_regression() {
            pma.grow_to_capacity(min_words)?;
        } else {
            pma.persist_metadata();
        }
        pma.clear_growth_journal_best_effort();
        pma.clear_migration_journal_best_effort();
        Ok(pma)
    }

    fn recover_metadata_from_growth_journal(
        path: &Path,
    ) -> Result<Option<PmaFileMetadata>, PmaError> {
        let Some(journal) = read_growth_journal(path)? else {
            return Ok(None);
        };
        let os_metadata = fs::metadata(path)?;
        let file_len = os_metadata.len();
        let new_len = file_len_for_words_u64(journal.new_data_words)?;
        let old_len = file_len_for_words_u64(journal.old_data_words)?;
        if file_len == new_len {
            return Ok(Some(PmaFileMetadata {
                magic: PMA_MAGIC,
                version: PMA_VERSION,
                data_words: journal.new_data_words,
                alloc_words: journal.old_alloc_words,
                capacity_words: journal.new_data_words,
                free_words: journal
                    .new_data_words
                    .saturating_sub(journal.old_alloc_words),
                reserved_words: journal.reserved_words,
                file_bytes: file_len,
                apparent_file_bytes: file_len,
                physical_file_bytes: physical_file_bytes(&os_metadata),
            }));
        }
        if file_len == old_len {
            return Ok(Some(PmaFileMetadata {
                magic: PMA_MAGIC,
                version: PMA_VERSION,
                data_words: journal.old_data_words,
                alloc_words: journal.old_alloc_words,
                capacity_words: journal.old_data_words,
                free_words: journal
                    .old_data_words
                    .saturating_sub(journal.old_alloc_words),
                reserved_words: journal.reserved_words,
                file_bytes: file_len,
                apparent_file_bytes: file_len,
                physical_file_bytes: physical_file_bytes(&os_metadata),
            }));
        }
        Ok(None)
    }

    fn recover_metadata_from_migration_journal(
        path: &Path,
    ) -> Result<Option<PmaFileMetadata>, PmaError> {
        let Some(journal) = read_migration_journal(path)? else {
            return Ok(None);
        };
        let os_metadata = fs::metadata(path)?;
        let file_len = os_metadata.len();
        let old_len =
            file_len_for_words_u64_with_tail(journal.capacity_words, PMA_LEGACY_TRAILER_BYTES)?;
        let new_len = file_len_for_words_u64(journal.capacity_words)?;
        if file_len != old_len && file_len != new_len {
            return Ok(None);
        }
        Ok(Some(PmaFileMetadata {
            magic: PMA_MAGIC,
            version: PMA_VERSION_V1,
            data_words: journal.capacity_words,
            alloc_words: journal.alloc_words,
            capacity_words: journal.capacity_words,
            free_words: journal.capacity_words.saturating_sub(journal.alloc_words),
            reserved_words: journal.reserved_words,
            file_bytes: file_len,
            apparent_file_bytes: file_len,
            physical_file_bytes: physical_file_bytes(&os_metadata),
        }))
    }

    fn migrate_legacy_metadata(
        path: &Path,
        capacity_words: usize,
        alloc_words: usize,
        reserved_words: usize,
    ) -> Result<(), PmaError> {
        let capacity_words_u64 = u64::try_from(capacity_words)
            .map_err(|_| PmaError::InvalidMetadata("PMA capacity exceeds u64".to_string()))?;
        let alloc_words_u64 = u64::try_from(alloc_words)
            .map_err(|_| PmaError::InvalidMetadata("PMA allocation exceeds u64".to_string()))?;
        let reserved_words_u64 = u64::try_from(reserved_words)
            .map_err(|_| PmaError::InvalidMetadata("PMA reservation exceeds u64".to_string()))?;
        let journal =
            PmaMigrationJournal::new(capacity_words_u64, alloc_words_u64, reserved_words_u64);
        write_migration_journal(path, journal)?;
        if should_inject_migration_failure("before_new_metadata_write") {
            return Err(PmaError::MigrationFailed(
                "injected PMA migration failure before new metadata write".to_string(),
            ));
        }

        let mut file = fs::OpenOptions::new().read(true).write(true).open(path)?;
        file.set_len(file_len_for_words_u64(capacity_words_u64)?)?;
        file.seek(SeekFrom::Start(capacity_words_u64 * 8))?;
        let trailer = PmaTrailerV2::new(capacity_words_u64, alloc_words_u64, reserved_words_u64);
        file.write_all(&trailer.to_bytes())?;
        if should_inject_migration_failure("after_new_metadata_write_before_fsync") {
            return Err(PmaError::MigrationFailed(
                "injected PMA migration failure after new metadata write before fsync".to_string(),
            ));
        }
        file.sync_data()?;
        if should_inject_migration_failure("after_metadata_fsync_before_parent_directory_sync") {
            return Err(PmaError::MigrationFailed(
                "injected PMA migration failure after metadata fsync before parent directory sync"
                    .to_string(),
            ));
        }
        if let Some(parent) = path.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
        if should_inject_migration_failure("after_parent_directory_sync") {
            return Err(PmaError::MigrationFailed(
                "injected PMA migration failure after parent directory sync".to_string(),
            ));
        }
        fs::remove_file(migration_journal_path(path))?;
        if let Some(parent) = path.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
        if should_inject_migration_failure("after_marker_declares_upgraded") {
            return Err(PmaError::MigrationFailed(
                "injected PMA migration failure after marker declares upgraded".to_string(),
            ));
        }
        info!(
            path = %path.display(),
            capacity_words,
            alloc_words,
            reserved_words,
            "PMA legacy metadata migrated to v2"
        );
        Ok(())
    }

    fn clear_growth_journal_best_effort(&self) {
        let _ = fs::remove_file(growth_journal_path(&self.path));
    }

    fn clear_migration_journal_best_effort(&self) {
        let _ = fs::remove_file(migration_journal_path(&self.path));
    }

    fn growth_poisoned_error() -> PmaError {
        PmaError::GrowthFailed(
            "previous PMA growth failed after possibly changing the backing file; restart the node or reopen the PMA so growth journal recovery can complete".to_string(),
        )
    }

    fn ensure_not_growth_poisoned(&self) -> Result<(), PmaError> {
        if self.growth_poisoned {
            Err(Self::growth_poisoned_error())
        } else {
            Ok(())
        }
    }

    fn panic_if_growth_poisoned(&self) {
        if let Err(err) = self.ensure_not_growth_poisoned() {
            panic!("{err}");
        }
    }

    fn poison_growth<T>(&mut self, message: String) -> Result<T, PmaError> {
        self.growth_poisoned = true;
        Err(PmaError::GrowthFailed(format!(
            "{message}; PMA handle is poisoned, restart the node or reopen the PMA so growth journal recovery can complete"
        )))
    }

    pub fn is_growth_poisoned(&self) -> bool {
        self.growth_poisoned
    }

    pub fn file_metadata(&self) -> Result<PmaFileMetadata, PmaError> {
        let mut metadata = Self::read_file_metadata(&self.path)?;
        metadata.reserved_words =
            u64::try_from(self.reserved_words()).expect("PMA reserved words exceed u64");
        metadata.capacity_words =
            u64::try_from(self.size_words()).expect("PMA capacity words exceed u64");
        metadata.data_words = metadata.capacity_words;
        metadata.alloc_words = self.alloc_offset_words().into();
        metadata.free_words = metadata.capacity_words.saturating_sub(metadata.alloc_words);
        Ok(metadata)
    }

    /// Flush the entire PMA mapping to storage.
    pub fn sync_all(&self) -> io::Result<()> {
        #[cfg(unix)]
        {
            let len_bytes = self.arena.mapped_len_bytes();
            if len_bytes == 0 {
                return Ok(());
            }
            let ret = unsafe {
                libc::msync(
                    self.arena.base_ptr() as *mut libc::c_void,
                    len_bytes,
                    libc::MS_SYNC,
                )
            };
            if ret != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    pub fn sync_used_data(&self) -> io::Result<()> {
        let used_words = self.alloc_offset().min(self.size_words());
        let used_bytes = PmaOffsetWords::try_from_usize(used_words)
            .expect("PMA used word count exceeds u64 addressable range")
            .checked_bytes_usize()
            .expect("PMA used byte count exceeds usize addressable range");
        self.sync_mapped_range(0, used_bytes)
    }

    pub fn sync_trailer(&self) -> io::Result<()> {
        self.sync_mapped_range(self.arena.len_bytes(), PMA_TRAILER_BYTES)
    }

    pub fn sync_file(&self) -> io::Result<()> {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.path)?
            .sync_data()
    }

    fn sync_mapped_range(&self, offset_bytes: usize, len_bytes: usize) -> io::Result<()> {
        #[cfg(unix)]
        {
            if len_bytes == 0 {
                return Ok(());
            }
            let mapped_len = self.arena.mapped_len_bytes();
            if mapped_len == 0 {
                return Ok(());
            }
            let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            if page <= 0 {
                return Err(io::Error::last_os_error());
            }
            let page = page as usize;
            let start = offset_bytes.min(mapped_len);
            let end = offset_bytes.saturating_add(len_bytes).min(mapped_len);
            if start >= end {
                return Ok(());
            }
            let start_aligned = (start / page) * page;
            let end_aligned = end
                .checked_add(page - 1)
                .map(|value| (value / page) * page)
                .unwrap_or(mapped_len)
                .min(mapped_len);
            let sync_len = end_aligned.saturating_sub(start_aligned);
            if sync_len == 0 {
                return Ok(());
            }
            let ret = unsafe {
                libc::msync(
                    self.arena.base_ptr().add(start_aligned) as *mut libc::c_void,
                    sync_len,
                    libc::MS_SYNC,
                )
            };
            if ret != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    /// Get the backing file path.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Get the underlying arena
    pub fn arena(&self) -> &Arc<Arena> {
        &self.arena
    }

    /// Get the current allocation offset in words
    pub fn alloc_offset(&self) -> usize {
        self.alloc_offset
    }

    /// Get the current allocation offset as a typed PMA word offset.
    pub fn alloc_offset_words(&self) -> PmaOffsetWords {
        PmaOffsetWords::try_from_usize(self.alloc_offset)
            .expect("PMA allocation offset exceeds u64 addressable range")
    }

    /// Get the total size of the PMA in words
    pub fn size_words(&self) -> usize {
        self.arena.words()
    }

    /// Get the maximum reserved PMA offset range in words.
    pub fn reserved_words(&self) -> usize {
        self.arena.reserved_words()
    }

    /// Get the number of free words remaining
    pub fn free_words(&self) -> usize {
        self.size_words().saturating_sub(self.alloc_offset())
    }

    /// Convert a pointer within the PMA to an offset in words
    pub fn offset_from_ptr(&self, ptr: *const u8) -> PmaOffsetWords {
        self.arena.offset_from_ptr(ptr)
    }

    /// Convert an offset in words to a pointer
    pub fn ptr_from_offset(&self, offset_words: PmaOffsetWords) -> *mut u8 {
        self.arena.ptr_from_offset(offset_words)
    }

    /// Check if a pointer is within the PMA's memory region
    pub fn contains_ptr(&self, ptr: *const u8) -> bool {
        let base = self.arena.base_ptr() as usize;
        let end = base
            .checked_add(self.arena.reserved_len_bytes())
            .expect("PMA bounds exceed usize address space");
        let ptr_addr = ptr as usize;
        ptr_addr >= base && ptr_addr < end
    }

    /// Reset the allocation pointer to zero
    pub fn reset(&mut self) {
        self.panic_if_growth_poisoned();
        self.alloc_offset = 0;
        self.persist_metadata();
    }

    /// Reset the allocation pointer to a specific offset
    ///
    /// # Panics
    /// Panics if `offset` is greater than the PMA size.
    pub fn reset_to(&mut self, offset: PmaOffsetWords) {
        self.panic_if_growth_poisoned();
        let offset = offset
            .try_into_usize()
            .expect("PMA reset offset exceeds usize addressable range");
        assert!(
            offset <= self.size_words(),
            "reset_to offset {} exceeds PMA size {}",
            offset,
            self.size_words()
        );
        self.alloc_offset = offset;
        self.persist_metadata();
    }

    /// Check if an allocation of `words` would exceed available space.
    ///
    /// # Panics
    /// Panics with `PmaError::OutOfMemory` if there isn't enough space.
    pub fn alloc_would_oom(&self, words: usize) {
        if words > self.free_words() {
            panic!(
                "{}",
                PmaError::OutOfMemory {
                    requested: words,
                    available: self.free_words(),
                }
            );
        }
    }

    /// Ensure at least `min_free_words` are available in the current PMA capacity.
    pub fn ensure_free_words(&mut self, min_free_words: usize) -> Result<(), PmaError> {
        self.ensure_not_growth_poisoned()?;
        let required = self
            .alloc_offset
            .checked_add(min_free_words)
            .ok_or_else(|| PmaError::GrowthFailed("required PMA words overflowed".to_string()))?;
        self.grow_to_fit_with_context(required, min_free_words, "ensure_free_words")
    }

    /// Allocate `words` from the PMA, returning a pointer to the allocation.
    ///
    /// # Panics
    /// Panics if there isn't enough space in the PMA.
    unsafe fn raw_alloc(&mut self, words: usize) -> *mut u64 {
        self.panic_if_growth_poisoned();
        let required = self
            .alloc_offset
            .checked_add(words)
            .unwrap_or_else(|| panic!("PMA allocation offset overflow for {words} words"));
        if required > self.size_words() {
            if pma_growth_disabled_for_regression() {
                self.alloc_would_oom(words);
            }
            if let Err(err) = self.grow_to_fit_with_context(required, words, "raw_alloc") {
                panic!("{err}");
            }
        }
        self.alloc_would_oom(words);
        let ptr = self.arena.ptr_from_offset(self.alloc_offset_words()) as *mut u64;
        self.alloc_offset += words;
        self.persist_metadata();
        ptr
    }

    pub fn grow_to_fit(&mut self, required_words: usize) -> Result<(), PmaError> {
        self.ensure_not_growth_poisoned()?;
        let request_words = required_words.saturating_sub(self.alloc_offset);
        self.grow_to_fit_with_context(required_words, request_words, "grow_to_fit")
    }

    fn grow_to_fit_with_context(
        &mut self,
        required_words: usize,
        request_words: usize,
        context: &'static str,
    ) -> Result<(), PmaError> {
        self.ensure_not_growth_poisoned()?;
        if required_words <= self.size_words() {
            return Ok(());
        }
        let mut new_words = self.size_words().max(1);
        while new_words < required_words {
            new_words = new_words.checked_mul(2).ok_or_else(|| {
                PmaError::GrowthFailed("PMA growth capacity overflowed".to_string())
            })?;
        }
        new_words = new_words.min(self.reserved_words());
        if new_words < required_words {
            return Err(PmaError::OutOfMemory {
                requested: request_words,
                available: self.free_words(),
            });
        }
        self.grow_to_capacity_with_context(new_words, required_words, request_words, context)
    }

    pub fn grow_to_capacity(&mut self, new_words: usize) -> Result<(), PmaError> {
        self.ensure_not_growth_poisoned()?;
        self.grow_to_capacity_with_context(new_words, new_words, 0, "grow_to_capacity")
    }

    fn grow_to_capacity_with_context(
        &mut self,
        new_words: usize,
        required_words: usize,
        request_words: usize,
        context: &'static str,
    ) -> Result<(), PmaError> {
        self.ensure_not_growth_poisoned()?;
        let old_words = self.size_words();
        if new_words <= old_words {
            return Ok(());
        }
        if should_inject_growth_failure("create_destination")
            || should_inject_growth_failure("before_file_extension")
        {
            return Err(PmaError::GrowthFailed(
                "injected PMA growth failure before file extension".to_string(),
            ));
        }
        if new_words > self.reserved_words() {
            return Err(PmaError::OutOfMemory {
                requested: request_words.max(new_words.saturating_sub(self.alloc_offset)),
                available: self.free_words(),
            });
        }
        let started = Instant::now();
        let old_alloc = self.alloc_offset;
        let journal = PmaGrowthJournal::new(
            u64::try_from(old_words).expect("PMA capacity exceeds u64"),
            u64::try_from(old_alloc).expect("PMA alloc exceeds u64"),
            u64::try_from(new_words).expect("PMA capacity exceeds u64"),
            u64::try_from(self.reserved_words()).expect("PMA reservation exceeds u64"),
        );
        write_growth_journal(&self.path, journal)?;
        if should_inject_growth_failure("after_journal") {
            return Err(PmaError::GrowthFailed(
                "injected PMA growth failure after journal".to_string(),
            ));
        }
        if let Err(err) = self.arena.grow_file_capacity(new_words, PMA_TRAILER_BYTES) {
            return self.poison_growth(format!("PMA growth failed during file extension: {err}"));
        }
        if should_inject_growth_failure("after_file_extension") {
            return self
                .poison_growth("injected PMA growth failure after file extension".to_string());
        }
        let old_trailer = unsafe { self.arena.base_ptr().add(old_words * 8) };
        unsafe {
            std::ptr::write_bytes(old_trailer, 0, PMA_TRAILER_BYTES);
        }
        if should_inject_growth_failure("after_zero_old_trailer") {
            return self.poison_growth(
                "injected PMA growth failure after zeroing old trailer".to_string(),
            );
        }
        self.persist_metadata();
        if should_inject_growth_failure("after_new_trailer_write") {
            return self
                .poison_growth("injected PMA growth failure after new trailer write".to_string());
        }
        if let Err(err) = self.sync_trailer() {
            return self.poison_growth(format!("PMA growth failed syncing metadata: {err}"));
        }
        if should_inject_growth_failure("after_metadata_fsync") {
            return self
                .poison_growth("injected PMA growth failure after metadata sync".to_string());
        }
        if let Err(err) = self.sync_file() {
            return self.poison_growth(format!("PMA growth failed syncing backing file: {err}"));
        }
        if should_inject_growth_failure("after_file_fsync") {
            return self.poison_growth("injected PMA growth failure after file fsync".to_string());
        }
        self.clear_growth_journal_best_effort();
        if should_inject_growth_failure("after_metadata_publication") {
            info!(
                path = %self.path.display(),
                "ignoring injected PMA growth failure after metadata publication because growth is already durable"
            );
        }
        self.record_growth_event(PmaGrowthEvent {
            old_words,
            new_words,
            alloc_words: old_alloc,
            required_words,
            request_words,
            context,
            elapsed: started.elapsed(),
        });
        Ok(())
    }

    fn record_growth_event(&self, event: PmaGrowthEvent) {
        info!(
            path = %self.path.display(),
            old_capacity_words = event.old_words,
            new_capacity_words = event.new_words,
            required_words = event.required_words,
            allocation_request_words = event.request_words,
            alloc_words = event.alloc_words,
            reserved_words = self.reserved_words(),
            context = event.context,
            elapsed_ms = event.elapsed.as_secs_f64() * 1000.0,
            "PMA automatic growth complete"
        );
        let Some(events_path) = std::env::var_os(NOCK_PMA_GROWTH_EVENTS_PATH_ENV) else {
            return;
        };
        let line = format!(
            "path={} old_capacity_words={} new_capacity_words={} required_words={} allocation_request_words={} alloc_words={} reserved_words={} context={} elapsed_ms={:.3}\n",
            self.path.display(),
            event.old_words,
            event.new_words,
            event.required_words,
            event.request_words,
            event.alloc_words,
            self.reserved_words(),
            event.context,
            event.elapsed.as_secs_f64() * 1000.0
        );
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(events_path)
        {
            let _ = file.write_all(line.as_bytes());
            let _ = file.sync_data();
        }
    }

    pub fn persist_metadata(&self) {
        self.panic_if_growth_poisoned();
        debug_assert!(
            self.arena.mapped_len_bytes()
                >= self
                    .arena
                    .len_bytes()
                    .checked_add(PMA_TRAILER_BYTES)
                    .expect("PMA trailer exceeds usize address space"),
            "PMA arena mapping is too small for metadata trailer"
        );
        let trailer = PmaTrailerV2::new(
            u64::try_from(self.arena.words())
                .expect("PMA arena size exceeds u64 addressable range"),
            self.alloc_offset_words().into(),
            u64::try_from(self.reserved_words()).expect("PMA reservation exceeds u64"),
        );
        let bytes = trailer.to_bytes();
        let dst = unsafe { self.arena.base_ptr().add(self.arena.len_bytes()) };
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        }
    }

    /// Hint the OS to drop the first `numerator/denominator` of allocated PMA data.
    pub fn advise_drop_allocated_prefix(
        &self,
        numerator: usize,
        denominator: usize,
    ) -> io::Result<usize> {
        if denominator == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "denominator must be non-zero",
            ));
        }

        let alloc_words = self.alloc_offset().min(self.size_words());
        let advise_words = alloc_words.saturating_mul(numerator) / denominator;
        if advise_words == 0 {
            return Ok(0);
        }
        let mut len_bytes = advise_words.saturating_mul(8);
        len_bytes = len_bytes.min(self.arena.len_bytes());

        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if page <= 0 {
            return Err(io::Error::other("failed to read page size"));
        }
        let page = page as usize;
        let len_aligned = (len_bytes / page) * page;
        if len_aligned == 0 {
            return Ok(0);
        }
        madvise_drop_file_backed_pages(self.arena.base_ptr() as *mut libc::c_void, len_aligned)?;
        Ok(len_aligned)
    }
}

fn default_reserved_words(capacity_words: usize) -> usize {
    if let Ok(value) = std::env::var(NOCK_PMA_RESERVED_WORDS_ENV) {
        if let Ok(words) = value.parse::<usize>() {
            return words.max(capacity_words);
        }
    }
    let default_reserved_words = DEFAULT_PMA_RESERVED_BYTES / std::mem::size_of::<u64>();
    default_reserved_words.max(capacity_words)
}

#[cfg(unix)]
fn physical_file_bytes(metadata: &fs::Metadata) -> Option<u64> {
    metadata.blocks().checked_mul(512)
}

#[cfg(not(unix))]
fn physical_file_bytes(_metadata: &fs::Metadata) -> Option<u64> {
    None
}

fn pma_growth_disabled_for_regression() -> bool {
    std::env::var_os(NOCK_PMA_DISABLE_RESIZE_ENV).is_some()
}

fn should_inject_growth_failure(point: &str) -> bool {
    #[cfg(test)]
    if TEST_GROWTH_FAILURE_POINT.with(|configured| {
        matches!(*configured.borrow(), Some(configured) if configured == point || configured == "any")
    }) {
        return true;
    }
    std::env::var(NOCK_PMA_RESIZE_FAIL_AT_ENV)
        .map(|value| value == point || value == "any")
        .unwrap_or(false)
}

fn should_inject_migration_failure(point: &str) -> bool {
    #[cfg(test)]
    if TEST_MIGRATION_FAILURE_POINT.with(|configured| {
        matches!(*configured.borrow(), Some(configured) if configured == point || configured == "any")
    }) {
        return true;
    }
    std::env::var(NOCK_PMA_MIGRATION_FAIL_AT_ENV)
        .map(|value| value == point || value == "any")
        .unwrap_or(false)
}

#[cfg(test)]
fn set_test_growth_failure_point(point: Option<&'static str>) {
    TEST_GROWTH_FAILURE_POINT.with(|configured| *configured.borrow_mut() = point);
}

#[cfg(test)]
fn set_test_migration_failure_point(point: Option<&'static str>) {
    TEST_MIGRATION_FAILURE_POINT.with(|configured| *configured.borrow_mut() = point);
}

fn file_len_for_words_u64(words: u64) -> Result<u64, PmaError> {
    file_len_for_words_u64_with_tail(words, PMA_TRAILER_BYTES)
}

fn file_len_for_words_u64_with_tail(words: u64, tail_bytes: usize) -> Result<u64, PmaError> {
    words
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(tail_bytes as u64))
        .ok_or_else(|| PmaError::InvalidMetadata("PMA file length overflowed".to_string()))
}

fn growth_journal_path(path: &Path) -> PathBuf {
    path.with_extension("grow")
}

fn migration_journal_path(path: &Path) -> PathBuf {
    path.with_extension("migrate")
}

fn read_growth_journal(path: &Path) -> Result<Option<PmaGrowthJournal>, PmaError> {
    let path = growth_journal_path(path);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(PmaError::MetadataIo(err)),
    };
    if bytes.len() == PmaGrowthJournal::BYTES {
        let mut buf = [0u8; PmaGrowthJournal::BYTES];
        buf.copy_from_slice(&bytes);
        return Ok(PmaGrowthJournal::from_bytes(buf));
    }
    if bytes.len() == PmaGrowthJournal::LEGACY_BYTES {
        let mut buf = [0u8; PmaGrowthJournal::LEGACY_BYTES];
        buf.copy_from_slice(&bytes);
        return Ok(PmaGrowthJournal::from_legacy_bytes(buf));
    }
    Ok(None)
}

fn write_growth_journal(path: &Path, journal: PmaGrowthJournal) -> Result<(), PmaError> {
    let journal_path = growth_journal_path(path);
    let tmp_path = journal_path.with_extension("grow.tmp");
    {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp_path)?;
        file.write_all(&journal.to_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp_path, &journal_path)?;
    if let Some(parent) = journal_path.parent() {
        let dir = fs::File::open(parent)?;
        dir.sync_all()?;
    }
    Ok(())
}

fn read_migration_journal(path: &Path) -> Result<Option<PmaMigrationJournal>, PmaError> {
    let path = migration_journal_path(path);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(PmaError::MetadataIo(err)),
    };
    if bytes.len() != PmaMigrationJournal::BYTES {
        return Ok(None);
    }
    let mut buf = [0u8; PmaMigrationJournal::BYTES];
    buf.copy_from_slice(&bytes);
    Ok(PmaMigrationJournal::from_bytes(buf))
}

fn write_migration_journal(path: &Path, journal: PmaMigrationJournal) -> Result<(), PmaError> {
    let journal_path = migration_journal_path(path);
    let tmp_path = journal_path.with_extension("migrate.tmp");
    {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp_path)?;
        file.write_all(&journal.to_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp_path, &journal_path)?;
    if let Some(parent) = journal_path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(unix)]
fn madvise_drop_file_backed_pages(ptr: *mut libc::c_void, len: usize) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let ret = unsafe { libc::madvise(ptr, len, libc::MADV_PAGEOUT) };
        if ret == 0 {
            return Ok(());
        }

        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EINVAL) | Some(libc::ENOSYS) => {
                let fallback = unsafe { libc::madvise(ptr, len, libc::MADV_DONTNEED) };
                if fallback == 0 {
                    return Ok(());
                }
                return Err(io::Error::last_os_error());
            }
            _ => return Err(err),
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let ret = unsafe { libc::madvise(ptr, len, libc::MADV_DONTNEED) };
        if ret != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl ibig::Stack for Pma {
    unsafe fn alloc_layout(&mut self, layout: std::alloc::Layout) -> *mut u64 {
        // Convert bytes to words, rounding up
        let words = (layout.size() + 7) >> 3;
        self.raw_alloc(words)
    }
}

impl NounAllocator for Pma {
    unsafe fn alloc_indirect(&mut self, words: usize) -> *mut u64 {
        self.raw_alloc(words + 2)
    }

    unsafe fn alloc_cell(&mut self) -> *mut CellMemory {
        self.raw_alloc(word_size_of::<CellMemory>()) as *mut CellMemory
    }

    unsafe fn alloc_struct<T>(&mut self, count: usize) -> *mut T {
        self.raw_alloc(word_size_of::<T>() * count) as *mut T
    }

    unsafe fn equals(&mut self, a: *mut Noun, b: *mut Noun) -> bool {
        let a = &*a;
        let b = &*b;
        let space = NounSpace::pma_only(self);
        noun_equality(a.in_space(&space), b.in_space(&space))
    }

    fn noun_space(&self) -> NounSpace {
        NounSpace::pma_only(self)
    }
}

/// Trait for types that can be copied into the PMA.
///
/// This is used to evacuate nouns from the NockStack to the PMA for persistence.
pub trait PmaCopy {
    /// Copy this value into the PMA.
    ///
    /// For nouns, this evacuates allocated data (indirect atoms, cells) to the PMA
    /// and converts pointers to offset form. Direct atoms are unchanged since they
    /// fit in a single word.
    ///
    /// # Safety
    /// The caller must ensure `stack` and `pma` describe the arenas that own the
    /// nouns being copied; pointer-form nouns are resolved via `NounSpace::new`.
    unsafe fn copy_to_pma(&mut self, stack: &NockStack, pma: &mut Pma);

    /// Assert that this value is fully contained within the PMA.
    ///
    /// For nouns, this verifies that all allocated data (indirect atoms, cells)
    /// resides in the PMA. Direct atoms always pass since they have no allocations.
    ///
    /// # Panics
    /// Panics if any part of this value is not in the PMA.
    fn assert_in_pma(&self, pma: &Pma);
}

/// Trait for types that can be copied from one PMA to another.
///
/// This is used for PMA compaction, copying reachable data from a from-space
/// PMA into a to-space PMA.
pub trait PmaCopyFrom {
    /// Copy this value from `from_pma` into `to_pma`, updating any internal
    /// pointers to reference the new PMA.
    ///
    /// # Safety
    /// The caller must ensure the value currently resides in `from_pma`.
    unsafe fn copy_from_pma(&mut self, from_pma: &Pma, to_pma: &mut Pma);
}

impl PmaCopy for () {
    unsafe fn copy_to_pma(&mut self, _stack: &NockStack, _pma: &mut Pma) {}

    fn assert_in_pma(&self, _pma: &Pma) {}
}

impl PmaCopyFrom for () {
    unsafe fn copy_from_pma(&mut self, _from_pma: &Pma, _to_pma: &mut Pma) {}
}

impl PmaCopy for Atom {
    unsafe fn copy_to_pma(&mut self, stack: &NockStack, pma: &mut Pma) {
        let mut noun = self.as_noun();
        noun.copy_to_pma(stack, pma);
        *self = noun.as_atom().expect("Atom remains atom after copy_to_pma");
    }

    #[cfg(feature = "pma-assert")]
    fn assert_in_pma(&self, pma: &Pma) {
        self.as_noun().assert_in_pma(pma);
    }

    #[cfg(not(feature = "pma-assert"))]
    #[inline(always)]
    fn assert_in_pma(&self, _pma: &Pma) {}
}

impl PmaCopyFrom for Atom {
    unsafe fn copy_from_pma(&mut self, from_pma: &Pma, to_pma: &mut Pma) {
        let mut noun = self.as_noun();
        noun.copy_from_pma(from_pma, to_pma);
        if let Ok(atom) = noun.as_atom() {
            *self = atom;
        }
    }
}

impl PmaCopy for Noun {
    /// Copy a noun and all its allocated substructure to the PMA.
    ///
    /// Uses a worklist algorithm to avoid stack overflow on deep structures.
    /// Structural sharing is preserved via forwarding pointers: if the same
    /// substructure is referenced multiple times, it's only copied once.
    ///
    /// # Algorithm
    /// 1. Push (noun, destination_ptr) onto worklist
    /// 2. Pop and process each item:
    ///    - Direct atoms: write directly to destination
    ///    - Already in PMA (offset form): write directly to destination
    ///    - Has forwarding pointer: write forwarded offset-form to destination
    ///    - Indirect atom: copy to PMA, set forwarding pointer, write offset-form
    ///    - Cell: copy metadata to PMA, set forwarding pointer, queue head/tail
    ///
    /// # Safety
    /// - Source nouns will have forwarding pointers set (corrupting the stack data)
    unsafe fn copy_to_pma(&mut self, stack: &NockStack, pma: &mut Pma) {
        if self.is_direct() {
            return;
        }

        let trace_noun = std::env::var_os("NOCK_PMA_TRACE_NOUN").is_some();
        let trace_start = Instant::now();
        let mut last_progress = trace_start;
        let mut steps = 0usize;

        let space = NounSpace::new(stack, &*pma);
        let root_repr = self.repr(&space);
        match root_repr {
            NounRepr::Indirect(AllocLocation::PmaOffset)
            | NounRepr::Cell(AllocLocation::PmaOffset) => {
                self.assert_in_pma(pma);
                return;
            }
            NounRepr::Indirect(AllocLocation::PmaPtr) | NounRepr::Cell(AllocLocation::PmaPtr) => {
                let offset_noun = {
                    let allocated = self.as_allocated().expect("repr said allocated");
                    let ptr = allocated.to_raw_pointer(&space);
                    assert!(
                        pma.contains_ptr(ptr as *const u8),
                        "noun claims PMA pointer but is outside PMA"
                    );
                    let offset = pma.offset_from_ptr(ptr as *const u8);
                    if allocated.is_indirect() {
                        IndirectAtom::from_offset_words(offset).as_noun()
                    } else {
                        Cell::from_offset_words(offset).as_noun()
                    }
                };
                *self = offset_noun;
                self.assert_in_pma(pma);
                return;
            }
            NounRepr::Forwarding(_) => {
                panic!("forwarding-pointer noun encountered during PMA copy");
            }
            _ => {}
        }

        let mut work: SmallVec<[(Noun, *mut Noun); 64]> = SmallVec::new();
        work.push((*self, self as *mut Noun));

        while let Some((noun, dest_ptr)) = work.pop() {
            steps += 1;
            if trace_noun && (steps & 0x3fff == 0) {
                let now = Instant::now();
                if now.duration_since(last_progress).as_millis() >= 2000 {
                    debug!(
                        "pma-copy: noun progress: steps={}, elapsed_ms={}",
                        steps,
                        trace_start.elapsed().as_millis()
                    );
                    last_progress = now;
                }
            }
            match noun.as_either_direct_allocated() {
                Left(_direct) => {
                    *dest_ptr = noun;
                }
                Right(allocated) => {
                    let forwarded = allocated.forwarding_pointer(&space);
                    if let Some(forwarded) = forwarded {
                        let offset_noun = {
                            let ptr = forwarded.to_raw_pointer(&space);
                            assert!(
                                pma.contains_ptr(ptr as *const u8),
                                "forwarding pointer escapes PMA"
                            );
                            let offset = pma.offset_from_ptr(ptr as *const u8);
                            if forwarded.is_indirect() {
                                IndirectAtom::from_offset_words(offset).as_noun()
                            } else {
                                Cell::from_offset_words(offset).as_noun()
                            }
                        };
                        *dest_ptr = offset_noun;
                        continue;
                    }

                    let repr = noun.repr(&space);

                    match repr {
                        NounRepr::Indirect(AllocLocation::PmaOffset)
                        | NounRepr::Cell(AllocLocation::PmaOffset) => {
                            noun.assert_in_pma(pma);
                            *dest_ptr = noun;
                            continue;
                        }
                        NounRepr::Indirect(AllocLocation::PmaPtr)
                        | NounRepr::Cell(AllocLocation::PmaPtr) => {
                            let offset_noun = {
                                let ptr = allocated.to_raw_pointer(&space);
                                assert!(
                                    pma.contains_ptr(ptr as *const u8),
                                    "noun claims PMA pointer but is outside PMA"
                                );
                                let offset = pma.offset_from_ptr(ptr as *const u8);
                                if allocated.is_indirect() {
                                    IndirectAtom::from_offset_words(offset).as_noun()
                                } else {
                                    Cell::from_offset_words(offset).as_noun()
                                }
                            };
                            noun.assert_in_pma(pma);
                            *dest_ptr = offset_noun;
                            continue;
                        }
                        NounRepr::Forwarding(_) => {
                            panic!("forwarding-pointer noun encountered during PMA copy");
                        }
                        NounRepr::Direct => {
                            *dest_ptr = noun;
                            continue;
                        }
                        NounRepr::Indirect(AllocLocation::Stack)
                        | NounRepr::Cell(AllocLocation::Stack) => {}
                    }

                    match allocated.as_either() {
                        Left(mut indirect) => {
                            let (raw_size, src_ptr) =
                                { (indirect.raw_size(&space), indirect.to_raw_pointer(&space)) };

                            let pma_ptr = pma.raw_alloc(raw_size);
                            copy_nonoverlapping(src_ptr, pma_ptr, raw_size);

                            indirect.set_forwarding_pointer(pma_ptr, &space);

                            let offset = pma.offset_from_ptr(pma_ptr as *const u8);
                            *dest_ptr = IndirectAtom::from_offset_words(offset).as_noun();
                        }
                        Right(mut cell) => {
                            let (src_cell, head, tail) = {
                                let src_cell = cell.to_raw_pointer(&space);
                                let head = (*src_cell).head;
                                let tail = (*src_cell).tail;
                                (src_cell, head, tail)
                            };

                            let pma_ptr = pma.raw_alloc(word_size_of::<CellMemory>());
                            let pma_cell = pma_ptr as *mut CellMemory;
                            (*pma_cell).metadata = (*src_cell).metadata;

                            cell.set_forwarding_pointer(pma_cell, &space);

                            work.push((tail, &mut (*pma_cell).tail));
                            work.push((head, &mut (*pma_cell).head));

                            let offset = pma.offset_from_ptr(pma_ptr as *const u8);
                            *dest_ptr = Cell::from_offset_words(offset).as_noun();
                        }
                    }
                }
            }
        }

        if trace_noun {
            debug!(
                "pma-copy: noun done: steps={}, elapsed_ms={}",
                steps,
                trace_start.elapsed().as_millis()
            );
        }
    }

    /// Assert that this noun and all its substructure is in the PMA.
    ///
    #[cfg(feature = "pma-assert")]
    fn assert_in_pma(&self, pma: &Pma) {
        if self.is_direct() {
            return;
        }

        let space = NounSpace::pma_only(pma);
        let mut seen = IntMap::new();
        let mut work = vec![*self];

        while let Some(noun) = work.pop() {
            if noun.is_direct() {
                continue;
            }

            match noun.repr(&space) {
                NounRepr::Indirect(AllocLocation::Stack) | NounRepr::Cell(AllocLocation::Stack) => {
                    panic!("noun is stack-allocated, not in PMA");
                }
                NounRepr::Forwarding(_) => {
                    panic!("forwarding pointer is not valid PMA state");
                }
                NounRepr::Indirect(_) | NounRepr::Direct => {}
                NounRepr::Cell(_) => {
                    let cell = noun.in_space(&space).as_cell().expect("checked is_cell");
                    let ptr = unsafe { cell.raw_pointer() } as usize as u64;
                    if seen.get(ptr).is_some() {
                        continue;
                    }
                    seen.insert(ptr, ());
                    work.push(cell.head().noun());
                    work.push(cell.tail().noun());
                }
            }
        }
    }

    #[cfg(not(feature = "pma-assert"))]
    #[inline(always)]
    fn assert_in_pma(&self, _pma: &Pma) {}
}

impl PmaCopyFrom for Noun {
    unsafe fn copy_from_pma(&mut self, from_pma: &Pma, to_pma: &mut Pma) {
        if self.is_direct() {
            return;
        }
        let to_base = to_pma.arena().base_ptr() as usize;
        let to_end = to_base
            .checked_add(to_pma.arena().reserved_len_bytes())
            .expect("PMA bounds exceed usize address space");
        let space = NounSpace::pma_only(from_pma).with_extra_ptr_ranges(vec![(to_base, to_end)]);
        let mut work: SmallVec<[(Noun, *mut Noun); 64]> = SmallVec::new();
        work.push((*self, self as *mut Noun));

        while let Some((noun, dest_ptr)) = work.pop() {
            match noun.as_either_direct_allocated() {
                Left(_direct) => {
                    *dest_ptr = noun;
                }
                Right(allocated) => {
                    if let Some(forwarded) = allocated.forwarding_pointer(&space) {
                        let ptr = forwarded.to_raw_pointer(&space) as *const u8;
                        let offset = to_pma.offset_from_ptr(ptr);
                        *dest_ptr = if forwarded.is_indirect() {
                            IndirectAtom::from_offset_words(offset).as_noun()
                        } else {
                            Cell::from_offset_words(offset).as_noun()
                        };
                        continue;
                    }

                    match allocated.as_either() {
                        Left(mut indirect) => {
                            let raw_size = indirect.raw_size(&space);
                            let src_ptr = indirect.to_raw_pointer(&space);
                            let pma_ptr = to_pma.raw_alloc(raw_size);
                            copy_nonoverlapping(src_ptr, pma_ptr, raw_size);

                            indirect.set_forwarding_pointer(pma_ptr, &space);

                            let offset = to_pma.offset_from_ptr(pma_ptr as *const u8);
                            *dest_ptr = IndirectAtom::from_offset_words(offset).as_noun();
                        }
                        Right(mut cell) => {
                            let src_cell = cell.to_raw_pointer(&space);
                            let head = (*src_cell).head;
                            let tail = (*src_cell).tail;

                            let pma_ptr = to_pma.raw_alloc(word_size_of::<CellMemory>());
                            let pma_cell = pma_ptr as *mut CellMemory;
                            (*pma_cell).metadata = (*src_cell).metadata;

                            cell.set_forwarding_pointer(pma_cell, &space);

                            work.push((tail, &mut (*pma_cell).tail));
                            work.push((head, &mut (*pma_cell).head));

                            let offset = to_pma.offset_from_ptr(pma_ptr as *const u8);
                            *dest_ptr = Cell::from_offset_words(offset).as_noun();
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn test_pma_path(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut path = std::env::temp_dir();
    path.push(format!("nockvm_pma_{label}_{pid}_{id}.mmap"));
    path
}

#[cfg(test)]
mod tests {
    use std::alloc::Layout;
    use std::fs;
    use std::sync::Mutex;

    use ibig::Stack;

    use super::*;
    use crate::hamt::Hamt;
    use crate::jets::cold::NounListMem;
    use crate::mem::{word_size_of, NockStack, NOCK_STACK_SIZE_TINY};
    use crate::noun::{AllocLocation, D, DIRECT_MAX};

    static PMA_ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Helper to create a test PMA with a given size
    fn test_pma(size_words: usize) -> Pma {
        let path = test_pma_path("pma");
        Pma::new(size_words, path).expect("Failed to create test PMA")
    }

    /// Verifies bump allocation returns sequential offsets and correctly tracks free space.
    ///
    /// This test exercises:
    /// - Pma::new creates a valid PMA
    /// - alloc_offset() starts at 0
    /// - free_words() equals size initially
    /// - NounAllocator::alloc_indirect bumps the offset correctly
    /// - NounAllocator::alloc_cell allocates CellMemory
    /// - NounAllocator::alloc_struct allocates arbitrary structs
    /// - Sequential allocations don't overlap
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_pma_allocation() {
        let mut pma = test_pma(1000);

        // Initial state: nothing allocated yet
        assert_eq!(pma.alloc_offset(), 0, "Initial alloc_offset should be 0");
        assert_eq!(
            pma.free_words(),
            1000,
            "Initial free_words should equal size"
        );

        // First allocation: alloc_indirect(10) allocates 10 + 2 = 12 words (data + metadata + size)
        let ptr1 = unsafe { pma.alloc_indirect(10) };
        assert!(
            !ptr1.is_null(),
            "First allocation should return non-null pointer"
        );
        assert_eq!(
            pma.alloc_offset(),
            12,
            "After alloc_indirect(10), offset should be 12"
        );
        assert_eq!(
            pma.free_words(),
            988,
            "After alloc_indirect(10), free should be 988"
        );

        // Second allocation: alloc_indirect(20) allocates 20 + 2 = 22 words
        let ptr2 = unsafe { pma.alloc_indirect(20) };
        assert!(
            !ptr2.is_null(),
            "Second allocation should return non-null pointer"
        );
        assert_eq!(
            pma.alloc_offset(),
            34,
            "After second alloc, offset should be 34"
        );
        assert_eq!(
            pma.free_words(),
            966,
            "After second alloc, free should be 966"
        );

        // Third allocation: alloc_cell allocates word_size_of::<CellMemory>() words
        let ptr3 = unsafe { pma.alloc_cell() };
        assert!(
            !ptr3.is_null(),
            "Cell allocation should return non-null pointer"
        );
        let cell_words = word_size_of::<CellMemory>();
        let offset_after_cell = 34 + cell_words;
        assert_eq!(
            pma.alloc_offset(),
            offset_after_cell,
            "After cell alloc, offset should increase by CellMemory size"
        );

        // Fourth allocation: alloc_struct for NounListMem
        let struct_words = word_size_of::<NounListMem>();
        let ptr4: *mut NounListMem = unsafe { pma.alloc_struct(1) };
        assert!(
            !ptr4.is_null(),
            "Struct allocation should return non-null pointer"
        );
        let offset_after_struct = offset_after_cell + struct_words;
        assert_eq!(
            pma.alloc_offset(),
            offset_after_struct,
            "After struct alloc, offset should increase by struct size in words"
        );

        // Fifth allocation: alloc_struct with count > 1 (allocate array of 3 NounListMem)
        let ptr5: *mut NounListMem = unsafe { pma.alloc_struct(3) };
        assert!(
            !ptr5.is_null(),
            "Array struct allocation should return non-null pointer"
        );
        let offset_after_array = offset_after_struct + (struct_words * 3);
        assert_eq!(
            pma.alloc_offset(),
            offset_after_array,
            "After array alloc, offset should increase by struct_size * count"
        );

        // Sixth allocation: alloc_layout for ibig::Stack trait (allocate 8 u64s)
        let layout_words = 8usize;
        let layout = Layout::array::<u64>(layout_words).expect("valid layout");
        let ptr6 = unsafe { pma.alloc_layout(layout) };
        assert!(
            !ptr6.is_null(),
            "Layout allocation should return non-null pointer"
        );
        assert_eq!(
            pma.alloc_offset(),
            offset_after_array + layout_words,
            "After layout alloc, offset should increase by layout size in words"
        );

        // Verify all allocations are sequential and non-overlapping
        // For a bump allocator, each pointer should be at or after the end of the previous allocation
        let ptr1_end = unsafe { ptr1.add(12) }; // 12 words for alloc_indirect(10)
        let ptr2_end = unsafe { ptr2.add(22) }; // 22 words for alloc_indirect(20)
        let ptr3_end = unsafe { (ptr3 as *mut u64).add(cell_words) };
        let ptr4_end = unsafe { (ptr4 as *mut u64).add(struct_words) };
        let ptr5_end = unsafe { (ptr5 as *mut u64).add(struct_words * 3) };

        assert!(ptr2 >= ptr1_end, "ptr2 should start at or after ptr1's end");
        assert!(
            ptr3 as *mut u64 >= ptr2_end,
            "ptr3 should start at or after ptr2's end"
        );
        assert!(
            ptr4 as *mut u64 >= ptr3_end,
            "ptr4 should start at or after ptr3's end"
        );
        assert!(
            ptr5 as *mut u64 >= ptr4_end,
            "ptr5 should start at or after ptr4's end"
        );
        assert!(ptr6 >= ptr5_end, "ptr6 should start at or after ptr5's end");
    }

    /// Verifies offset-to-pointer and pointer-to-offset conversions are inverses.
    ///
    /// This test exercises:
    /// - ptr_from_offset converts word offset to pointer
    /// - offset_from_ptr converts pointer back to word offset
    /// - Round-trip: offset -> ptr -> offset gives same offset
    /// - Round-trip: ptr -> offset -> ptr gives same ptr
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_pma_offset_round_trip() {
        let mut pma = test_pma(1000);

        // Test with offset 0 (base of PMA)
        let ptr_at_0 = pma.ptr_from_offset(PmaOffsetWords::from_words(0));
        let offset_from_0 = pma.offset_from_ptr(ptr_at_0);
        assert_eq!(offset_from_0.words(), 0, "Offset at base should be 0");

        // Test with a known offset
        let test_offset = PmaOffsetWords::from_words(42);
        let ptr = pma.ptr_from_offset(test_offset);
        let recovered_offset = pma.offset_from_ptr(ptr);
        assert_eq!(
            recovered_offset, test_offset,
            "Round-trip offset -> ptr -> offset should return same offset"
        );

        // Test with pointer from an allocation
        let alloc_ptr = unsafe { pma.alloc_indirect(10) };
        let alloc_offset = pma.offset_from_ptr(alloc_ptr as *const u8);
        let recovered_ptr = pma.ptr_from_offset(alloc_offset);
        assert_eq!(
            recovered_ptr, alloc_ptr as *mut u8,
            "Round-trip ptr -> offset -> ptr should return same pointer"
        );

        // Test multiple allocations have distinct offsets
        let ptr1 = unsafe { pma.alloc_indirect(5) };
        let ptr2 = unsafe { pma.alloc_indirect(5) };
        let offset1 = pma.offset_from_ptr(ptr1 as *const u8);
        let offset2 = pma.offset_from_ptr(ptr2 as *const u8);
        assert_ne!(
            offset1, offset2,
            "Different allocations should have different offsets"
        );

        // Verify the offsets differ by the expected amount (5 + 2 = 7 words)
        assert_eq!(
            offset2
                .checked_sub(offset1)
                .expect("offsets should be ordered")
                .words(),
            7,
            "Second allocation offset should be 7 words after first"
        );
    }

    /// Verifies contains_ptr correctly identifies pointers inside vs outside the PMA.
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_pma_contains_ptr() {
        let path = test_pma_path("pma_contains_ptr");
        let mut pma = Pma::new_with_reserved(1000, 2000, path).expect("Failed to create test PMA");

        // Get base pointer and compute some test pointers
        let base = pma.arena().base_ptr();
        let len_bytes = pma.arena().len_bytes();
        let reserved_len_bytes = pma.arena().reserved_len_bytes();

        // Base pointer should be in PMA
        assert!(pma.contains_ptr(base), "Base pointer should be in PMA");

        // Pointer at offset 0 should be in PMA
        let ptr_at_0 = pma.ptr_from_offset(PmaOffsetWords::from_words(0));
        assert!(
            pma.contains_ptr(ptr_at_0),
            "Pointer at offset 0 should be in PMA"
        );

        // Pointer in the middle should be in PMA
        let middle_offset = PmaOffsetWords::from_words(500);
        let ptr_middle = pma.ptr_from_offset(middle_offset);
        assert!(
            pma.contains_ptr(ptr_middle),
            "Pointer in middle should be in PMA"
        );

        // Last valid byte should be in PMA
        let last_byte = unsafe { base.add(len_bytes - 1) };
        assert!(pma.contains_ptr(last_byte), "Last byte should be in PMA");

        // Pointer just past the current file capacity should still be in the reserved PMA range
        let past_current_capacity = unsafe { base.add(len_bytes) };
        assert!(
            pma.contains_ptr(past_current_capacity),
            "Pointer past current capacity should be in reserved PMA range"
        );

        // Last reserved byte should be in PMA
        let last_reserved_byte = unsafe { base.add(reserved_len_bytes - 1) };
        assert!(
            pma.contains_ptr(last_reserved_byte),
            "Last reserved byte should be in PMA"
        );

        // Pointer just past the reserved range should NOT be in PMA
        let past_reserved_end = unsafe { base.add(reserved_len_bytes) };
        assert!(
            !pma.contains_ptr(past_reserved_end),
            "Pointer past reserved range should not be in PMA"
        );

        // Pointer well past the reserved range should NOT be in PMA
        let way_past_end = unsafe { base.add(reserved_len_bytes + 1000) };
        assert!(
            !pma.contains_ptr(way_past_end),
            "Pointer way past reserved range should not be in PMA"
        );

        // Pointer before the base should NOT be in PMA (if base > 0)
        if base as usize > 0 {
            let before_base = unsafe { base.sub(1) };
            assert!(
                !pma.contains_ptr(before_base),
                "Pointer before base should not be in PMA"
            );
        }

        // Null pointer should NOT be in PMA
        assert!(
            !pma.contains_ptr(std::ptr::null()),
            "Null pointer should not be in PMA"
        );

        // Allocated pointer should be in PMA
        let alloc_ptr = unsafe { pma.alloc_indirect(10) };
        assert!(
            pma.contains_ptr(alloc_ptr as *const u8),
            "Allocated pointer should be in PMA"
        );
    }

    /// Verifies allocation fails gracefully when PMA is full.
    ///
    /// This test exercises:
    /// - alloc_would_oom() does not panic when there's space
    /// - alloc_would_oom() panics when there isn't enough space
    /// - Exact-fit allocations succeed
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_pma_out_of_memory() {
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let mut pma = test_pma(100); // Small PMA: 100 words

        // alloc_would_oom should not panic when there's space
        pma.alloc_would_oom(50); // Should not panic
        pma.alloc_would_oom(100); // Should not panic (exact fit)

        // alloc_would_oom should panic when there isn't space
        let result = catch_unwind(AssertUnwindSafe(|| {
            pma.alloc_would_oom(101);
        }));
        assert!(
            result.is_err(),
            "alloc_would_oom(101) should panic with 100 free"
        );

        // Allocate some space
        unsafe { pma.alloc_indirect(10) }; // 12 words (10 + 2 for metadata/size)
        assert_eq!(pma.alloc_offset(), 12);
        assert_eq!(pma.free_words(), 88);

        // alloc_would_oom should reflect remaining space
        pma.alloc_would_oom(88); // Should not panic
        let result = catch_unwind(AssertUnwindSafe(|| {
            pma.alloc_would_oom(89);
        }));
        assert!(
            result.is_err(),
            "alloc_would_oom(89) should panic with 88 free"
        );

        // Fill the rest
        unsafe { pma.alloc_struct::<u64>(88) };
        assert_eq!(pma.alloc_offset(), 100);
        assert_eq!(pma.free_words(), 0);

        // alloc_would_oom should panic for any non-zero allocation when full
        let result = catch_unwind(AssertUnwindSafe(|| {
            pma.alloc_would_oom(1);
        }));
        assert!(result.is_err(), "alloc_would_oom(1) should panic when full");

        // But 0 words should not panic
        pma.alloc_would_oom(0); // Should not panic

        // Reset and verify we can allocate again
        pma.reset();
        assert_eq!(pma.free_words(), 100);
        pma.alloc_would_oom(100); // Should not panic after reset

        // Verify exact-fit allocation works
        unsafe { pma.alloc_struct::<u64>(100) };
        assert_eq!(pma.alloc_offset(), 100);
        assert_eq!(pma.free_words(), 0);
    }

    /// Verifies reset() and reset_to() correctly manage the allocation pointer.
    ///
    /// This test exercises:
    /// - reset() sets alloc_offset back to 0
    /// - reset_to(offset) sets alloc_offset to a specific value
    /// - After reset, free_words equals size again
    /// - Allocations after reset start from the reset point
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_pma_reset() {
        let mut pma = test_pma(1000);

        // Allocate some space
        unsafe { pma.alloc_indirect(10) }; // 12 words
        unsafe { pma.alloc_indirect(20) }; // 22 words
        assert_eq!(pma.alloc_offset(), 34);
        assert_eq!(pma.free_words(), 966);

        // Reset to zero
        pma.reset();
        assert_eq!(pma.alloc_offset(), 0, "reset() should set offset to 0");
        assert_eq!(
            pma.free_words(),
            1000,
            "reset() should restore all free space"
        );

        // Allocations after reset should start from 0
        let ptr_after_reset = unsafe { pma.alloc_indirect(5) }; // 7 words
        assert_eq!(pma.alloc_offset(), 7);
        let offset_after_reset = pma.offset_from_ptr(ptr_after_reset as *const u8);
        assert_eq!(
            offset_after_reset.words(),
            0,
            "First allocation after reset should be at offset 0"
        );

        // Allocate more to create a checkpoint
        unsafe { pma.alloc_indirect(10) }; // 12 more words
        let checkpoint = pma.alloc_offset();
        assert_eq!(checkpoint, 19); // 7 + 12

        // Allocate even more
        unsafe { pma.alloc_indirect(30) }; // 32 more words
        assert_eq!(pma.alloc_offset(), 51); // 19 + 32

        // Reset to checkpoint
        pma.reset_to(PmaOffsetWords::try_from_usize(checkpoint).expect("checkpoint fits"));
        assert_eq!(
            pma.alloc_offset(),
            19,
            "reset_to() should set offset to checkpoint"
        );
        assert_eq!(
            pma.free_words(),
            981,
            "reset_to() should restore free space from checkpoint"
        );

        // Next allocation should start at the checkpoint
        let ptr_after_reset_to = unsafe { pma.alloc_indirect(3) }; // 5 words
        let offset_after_reset_to = pma.offset_from_ptr(ptr_after_reset_to as *const u8);
        assert_eq!(
            offset_after_reset_to.words(),
            19,
            "Allocation after reset_to should start at checkpoint"
        );
        assert_eq!(pma.alloc_offset(), 24); // 19 + 5
    }

    /// Verifies reset_to panics when given an offset outside the PMA bounds.
    #[test]
    #[should_panic(expected = "reset_to offset")]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_pma_reset_to_out_of_bounds() {
        let mut pma = test_pma(1000);
        pma.reset_to(PmaOffsetWords::from_words(1001)); // Should panic: offset exceeds PMA size
    }

    #[test]
    fn test_pma_open_restores_alloc_offset() {
        let path = test_pma_path("open_restore");
        {
            let mut pma = Pma::new(1000, path.clone()).expect("Failed to create test PMA");
            unsafe { pma.alloc_indirect(10) };
            unsafe { pma.alloc_cell() };
            assert!(
                pma.alloc_offset() > 0,
                "Expected allocations to advance offset"
            );
        }

        let pma = Pma::open(path).expect("Failed to open PMA");
        assert!(
            pma.alloc_offset() > 0,
            "alloc_offset should be restored on open"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "file-backed PMA unsupported in Miri")]
    fn test_growable_pma_reports_capacity_separately_from_reservation() {
        let path = test_pma_path("reporting");
        let pma =
            Pma::new_with_reserved(1024, 16 * 1024, path.clone()).expect("create growable PMA");
        let file_len = fs::metadata(&path).expect("pma metadata").len();
        let expected_len = 1024_u64 * 8 + PMA_TRAILER_BYTES as u64;
        assert_eq!(
            file_len, expected_len,
            "apparent file size should track current capacity, not reserved words"
        );

        let metadata = pma.file_metadata().expect("runtime PMA metadata");
        assert_eq!(metadata.capacity_words, 1024);
        assert_eq!(metadata.data_words, 1024);
        assert_eq!(metadata.alloc_words, 0);
        assert_eq!(metadata.free_words, 1024);
        assert_eq!(metadata.reserved_words, 16 * 1024);
        assert_eq!(metadata.apparent_file_bytes, expected_len);
        assert_eq!(metadata.file_bytes, expected_len);
        #[cfg(unix)]
        assert!(
            metadata.physical_file_bytes.is_some(),
            "unix metadata should expose physical allocated bytes"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "file-backed PMA unsupported in Miri")]
    fn test_legacy_pma_migrates_to_v2_and_preserves_content() {
        let path = test_pma_path("legacy_migration");
        let mut stack = NockStack::new(128, 0);
        let mut root = Cell::new(&mut stack, D(30), D(31)).as_noun();
        let alloc_words;
        {
            let mut pma = Pma::new_with_reserved(32, 128, path.clone()).expect("create PMA");
            unsafe {
                root.copy_to_pma(&stack, &mut pma);
            }
            alloc_words = pma.alloc_offset();
            pma.sync_all().expect("sync PMA");
            pma.sync_file().expect("sync PMA file");
        }
        {
            let mut file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .expect("open PMA file");
            file.set_len(32 * 8 + PMA_LEGACY_TRAILER_BYTES as u64)
                .expect("truncate to legacy trailer");
            file.seek(SeekFrom::Start(32 * 8)).expect("seek trailer");
            let trailer = PmaLegacyTrailer {
                magic: PMA_MAGIC,
                version: PMA_VERSION_V1,
                data_words: 32,
                alloc_offset: alloc_words as u64,
            };
            file.write_all(&trailer.to_bytes())
                .expect("write legacy trailer");
            file.sync_all().expect("sync legacy PMA file");
        }

        let legacy_metadata = Pma::read_file_metadata(&path).expect("read legacy metadata");
        assert_eq!(legacy_metadata.version, PMA_VERSION_V1);
        assert_eq!(legacy_metadata.capacity_words, 32);
        assert_eq!(legacy_metadata.alloc_words, alloc_words as u64);

        let reopened = Pma::open_with_min(path.clone(), 64).expect("migrate and grow legacy PMA");
        assert_eq!(reopened.size_words(), 64);
        assert!(reopened.reserved_words() >= 64);
        assert_eq!(reopened.alloc_offset(), alloc_words);
        let migrated_metadata = Pma::read_file_metadata(&path).expect("read migrated metadata");
        assert_eq!(migrated_metadata.version, PMA_VERSION);
        assert_eq!(migrated_metadata.capacity_words, 64);
        assert!(migrated_metadata.reserved_words >= 64);
        assert_eq!(migrated_metadata.alloc_words, alloc_words as u64);

        let space = NounSpace::pma_only(&reopened);
        let cell = root.in_space(&space).as_cell().expect("root cell");
        assert_eq!(
            cell.head()
                .as_atom()
                .expect("head atom")
                .as_u64()
                .expect("head atom should fit in u64"),
            30
        );
        assert_eq!(
            cell.tail()
                .as_atom()
                .expect("tail atom")
                .as_u64()
                .expect("tail atom should fit in u64"),
            31
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "file-backed PMA unsupported in Miri")]
    fn test_legacy_migration_journal_recovers_after_injected_failures() {
        let _env_lock = PMA_ENV_LOCK.lock().expect("PMA env lock");
        let fail_points = [
            "before_new_metadata_write", "after_new_metadata_write_before_fsync",
            "after_metadata_fsync_before_parent_directory_sync", "after_parent_directory_sync",
            "after_marker_declares_upgraded",
        ];
        for fail_point in fail_points {
            let path = test_pma_path(&format!("legacy_migration_{fail_point}"));
            let mut stack = NockStack::new(128, 0);
            let mut root = Cell::new(&mut stack, D(40), D(41)).as_noun();
            let alloc_words;
            {
                let mut pma = Pma::new_with_reserved(32, 128, path.clone()).expect("create PMA");
                unsafe {
                    root.copy_to_pma(&stack, &mut pma);
                }
                alloc_words = pma.alloc_offset();
                pma.sync_all().expect("sync PMA");
                pma.sync_file().expect("sync PMA file");
            }
            {
                let mut file = fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&path)
                    .expect("open PMA file");
                file.set_len(32 * 8 + PMA_LEGACY_TRAILER_BYTES as u64)
                    .expect("truncate to legacy trailer");
                file.seek(SeekFrom::Start(32 * 8)).expect("seek trailer");
                let trailer = PmaLegacyTrailer {
                    magic: PMA_MAGIC,
                    version: PMA_VERSION_V1,
                    data_words: 32,
                    alloc_offset: alloc_words as u64,
                };
                file.write_all(&trailer.to_bytes())
                    .expect("write legacy trailer");
                file.sync_all().expect("sync legacy PMA file");
            }

            set_test_migration_failure_point(Some(fail_point));
            let err = match Pma::open(path.clone()) {
                Ok(_) => panic!("injected migration failure should fail at {fail_point}"),
                Err(err) => err,
            };
            set_test_migration_failure_point(None);
            assert!(
                err.to_string().contains("injected PMA migration failure"),
                "unexpected injected migration error at {fail_point}: {err}"
            );

            let reopened = Pma::open(path.clone()).expect("reopen should complete migration");
            assert_eq!(reopened.alloc_offset(), alloc_words);
            let migrated_metadata = Pma::read_file_metadata(&path).expect("read migrated metadata");
            assert_eq!(migrated_metadata.version, PMA_VERSION);
            assert_eq!(migrated_metadata.capacity_words, 32);
            assert!(
                !migration_journal_path(&path).exists(),
                "successful reopen should clear migration journal for {fail_point}"
            );
            let space = NounSpace::pma_only(&reopened);
            let cell = root.in_space(&space).as_cell().expect("root cell");
            assert_eq!(
                cell.head()
                    .as_atom()
                    .expect("head atom")
                    .as_u64()
                    .expect("head atom should fit in u64"),
                40
            );
            assert_eq!(
                cell.tail()
                    .as_atom()
                    .expect("tail atom")
                    .as_u64()
                    .expect("tail atom should fit in u64"),
                41
            );
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "file-backed PMA unsupported in Miri")]
    fn test_growth_journal_recovers_after_injected_failures() {
        let _env_lock = PMA_ENV_LOCK.lock().expect("PMA env lock");
        let fail_points = [
            "after_journal", "after_file_extension", "after_zero_old_trailer",
            "after_new_trailer_write", "after_metadata_fsync", "after_file_fsync",
        ];
        for fail_point in fail_points {
            let path = test_pma_path(&format!("growth_journal_recovery_{fail_point}"));
            let mut stack = NockStack::new(128, 0);
            let mut root = Cell::new(&mut stack, D(10), D(11)).as_noun();
            {
                let mut pma = Pma::new_with_reserved(32, 1024, path.clone()).expect("create PMA");
                unsafe {
                    root.copy_to_pma(&stack, &mut pma);
                }
                assert_eq!(pma.alloc_offset(), word_size_of::<CellMemory>());
                set_test_growth_failure_point(Some(fail_point));
                let err = pma
                    .grow_to_capacity(64)
                    .expect_err("injected growth failure should fail");
                set_test_growth_failure_point(None);
                assert!(
                    err.to_string().contains("injected PMA growth failure"),
                    "unexpected injected failure error at {fail_point}: {err}"
                );
                let should_poison = fail_point != "after_journal";
                assert_eq!(
                    pma.is_growth_poisoned(),
                    should_poison,
                    "PMA poison state mismatch after {fail_point}"
                );
                if should_poison {
                    let followup_err = pma
                        .ensure_free_words(1)
                        .expect_err("poisoned PMA handle should reject further mutation");
                    assert!(
                        followup_err
                            .to_string()
                            .contains("previous PMA growth failed"),
                        "unexpected poisoned-handle error after {fail_point}: {followup_err}"
                    );
                }
            }

            let mut reopened = Pma::open(path.clone()).unwrap_or_else(|err| {
                panic!("reopen should recover growth failure at {fail_point}: {err}")
            });
            assert!(
                (32..=64).contains(&reopened.size_words()),
                "recovered PMA capacity should be old or new after {fail_point}, got {}",
                reopened.size_words()
            );
            assert_eq!(
                reopened.reserved_words(),
                1024,
                "growth journal recovery should preserve virtual reservation after {fail_point}"
            );
            reopened.grow_to_capacity(512).unwrap_or_else(|err| {
                panic!("reserved PMA should grow again after {fail_point}: {err}")
            });
            assert_eq!(reopened.reserved_words(), 1024);
            assert_eq!(reopened.alloc_offset(), word_size_of::<CellMemory>());
            assert!(
                !growth_journal_path(&path).exists(),
                "successful reopen should clear growth journal after {fail_point}"
            );
            let space = NounSpace::pma_only(&reopened);
            let cell = root.in_space(&space).as_cell().expect("root cell");
            assert_eq!(
                cell.head()
                    .as_atom()
                    .expect("head atom")
                    .as_u64()
                    .expect("head atom should fit in u64"),
                10
            );
            assert_eq!(
                cell.tail()
                    .as_atom()
                    .expect("tail atom")
                    .as_u64()
                    .expect("tail atom should fit in u64"),
                11
            );
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "file-backed PMA unsupported in Miri")]
    fn test_growth_failure_after_publication_is_committed_success() {
        let _env_lock = PMA_ENV_LOCK.lock().expect("PMA env lock");
        let path = test_pma_path("growth_after_publication_success");
        let mut pma = Pma::new_with_reserved(32, 1024, path.clone()).expect("create PMA");

        set_test_growth_failure_point(Some("after_metadata_publication"));
        pma.grow_to_capacity(64)
            .expect("post-publication injection should not report failed growth");
        set_test_growth_failure_point(None);

        assert_eq!(pma.size_words(), 64);
        assert_eq!(pma.reserved_words(), 1024);
        assert!(!pma.is_growth_poisoned());
        assert!(
            !growth_journal_path(&path).exists(),
            "successful published growth should clear growth journal"
        );

        let reopened = Pma::open(path).expect("reopen published growth");
        assert_eq!(reopened.size_words(), 64);
        assert_eq!(reopened.reserved_words(), 1024);
    }

    /// Verifies direct atoms are unchanged by evacuation since they fit in a single word.
    ///
    /// Direct atoms don't require any allocation - they're just 64-bit values with
    /// the MSB = 0. Evacuation should leave them completely unchanged.
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_evacuate_direct_atom() {
        let stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let mut pma = test_pma(1000);

        // Test several direct atom values
        let test_values: [u64; 5] = [0, 1, 42, 12345, DIRECT_MAX];

        for &val in &test_values {
            let mut noun = D(val);
            let original_raw = unsafe { noun.as_raw() };

            // Evacuate to PMA
            unsafe { noun.copy_to_pma(&stack, &mut pma) };

            // Direct atoms should be completely unchanged
            let after_raw = unsafe { noun.as_raw() };
            assert_eq!(
                original_raw, after_raw,
                "Direct atom {} should be unchanged after evacuation",
                val
            );

            // Verify it's still a direct atom
            assert!(
                noun.is_direct(),
                "Should still be a direct atom after evacuation"
            );

            // Direct atoms should trivially pass assert_in_pma (no allocations to check)
            noun.assert_in_pma(&pma);
        }

        // PMA should have no allocations (direct atoms don't need space)
        assert_eq!(
            pma.alloc_offset(),
            0,
            "No allocations should be made for direct atoms"
        );
    }

    /// Verifies indirect atoms (too large for direct representation) are copied to PMA
    /// and converted to offset form.
    ///
    /// This test exercises:
    /// - Creating an indirect atom on the NockStack
    /// - Evacuating it to the PMA via copy_to_pma
    /// - Verifying the atom is now in offset form (LOCATION_BIT set)
    /// - Verifying the data can be read correctly via the PMA arena
    /// - Verifying PMA allocations were made
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_evacuate_indirect_atom() {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let mut pma = test_pma(1000);
        let space = NounSpace::new(&stack, &pma);

        // Create an indirect atom on the stack (value > DIRECT_MAX requires indirect storage)
        // We'll use a 2-word value to ensure it's indirect
        let data: [u64; 2] = [0xDEADBEEF_CAFEBABE, 0x12345678_9ABCDEF0];
        let indirect = unsafe { IndirectAtom::new_raw(&mut stack, 2, data.as_ptr()) };
        let mut noun = indirect.as_noun();

        // Verify it's an indirect atom on the stack
        assert!(noun.is_indirect(), "Should be an indirect atom");
        assert!(!noun.is_direct(), "Should not be a direct atom");
        assert!(
            matches!(
                noun.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "Should be stack-allocated before evacuation"
        );

        // Record the initial PMA offset
        let initial_offset = pma.alloc_offset();
        assert_eq!(initial_offset, 0, "PMA should start empty");

        // Evacuate to PMA
        unsafe { noun.copy_to_pma(&stack, &mut pma) };

        // Verify PMA allocation was made
        // Indirect atom needs: metadata (1) + size (1) + data (2) = 4 words
        assert!(
            pma.alloc_offset() > initial_offset,
            "PMA should have allocations after evacuation"
        );
        assert_eq!(
            pma.alloc_offset(),
            4, // metadata + size + 2 data words
            "Indirect atom should allocate 4 words in PMA"
        );

        // Verify the noun is now in offset form (not stack-allocated)
        assert!(
            !matches!(
                noun.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "Should be in offset form after evacuation"
        );
        assert!(noun.is_indirect(), "Should still be an indirect atom");

        // Verify data is readable and correct via PMA arena
        let atom = noun.as_atom().expect("Should be an atom");
        let read_indirect = atom.as_indirect().expect("Should be indirect");

        // Read the size - should be 2 words
        let read_handle = read_indirect.as_atom().in_space(&space);
        let size = read_handle.size();
        assert_eq!(size, 2, "Indirect atom should have size 2");

        // Read the data back and verify it matches
        let data_ptr = read_handle.data_pointer();
        let read_data = unsafe { std::slice::from_raw_parts(data_ptr, 2) };
        assert_eq!(read_data[0], data[0], "First data word should match");
        assert_eq!(read_data[1], data[1], "Second data word should match");

        // Verify assert_in_pma passes
        noun.assert_in_pma(&pma);
    }

    /// Verifies a simple cell with direct atom contents is evacuated and readable from PMA.
    ///
    /// This test exercises:
    /// - Creating a cell [head tail] on the NockStack
    /// - Evacuating it to the PMA
    /// - Verifying the cell is in offset form
    /// - Verifying head and tail are readable and correct
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_evacuate_simple_cell() {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let mut pma = test_pma(1000);
        let space = NounSpace::new(&stack, &pma);

        // Create a simple cell [42 123] with direct atoms
        let mut noun = Cell::new(&mut stack, D(42), D(123)).as_noun();

        // Verify it's a cell on the stack
        assert!(noun.is_cell(), "Should be a cell");
        assert!(
            matches!(
                noun.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "Should be stack-allocated before evacuation"
        );

        unsafe { noun.copy_to_pma(&stack, &mut pma) };

        // Verify PMA allocation was made (CellMemory size)
        let cell_words = word_size_of::<CellMemory>();
        assert_eq!(
            pma.alloc_offset(),
            cell_words,
            "Cell should allocate {} words",
            cell_words
        );

        // Verify the noun is now in offset form
        assert!(
            !matches!(
                noun.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "Should be in offset form after evacuation"
        );
        assert!(noun.is_cell(), "Should still be a cell");

        // Read head and tail
        let cell = noun.in_space(&space).as_cell().expect("Should be a cell");
        let head = cell.head().noun();
        let tail = cell.tail().noun();

        // Verify head and tail are correct direct atoms
        assert!(head.is_direct(), "Head should be direct");
        assert!(tail.is_direct(), "Tail should be direct");
        assert_eq!(
            head.as_direct().expect("head is direct").data(),
            42,
            "Head should be 42"
        );
        assert_eq!(
            tail.as_direct().expect("tail is direct").data(),
            123,
            "Tail should be 123"
        );

        // Verify assert_in_pma passes
        noun.assert_in_pma(&pma);
    }

    /// Verifies nested cell structures are fully evacuated with all sub-cells in offset form.
    ///
    /// This test exercises:
    /// - Creating nested cells [[1 2] [3 4]]
    /// - Evacuating the entire structure
    /// - Verifying all cells are in offset form
    /// - Verifying all values are readable
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_evacuate_nested_cells() {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let mut pma = test_pma(1000);
        let space = NounSpace::new(&stack, &pma);

        // Create nested cells: [[1 2] [3 4]]
        let left = Cell::new(&mut stack, D(1), D(2)).as_noun();
        let right = Cell::new(&mut stack, D(3), D(4)).as_noun();
        let mut noun = Cell::new(&mut stack, left, right).as_noun();

        // Verify structure before evacuation
        assert!(noun.is_cell(), "Root should be a cell");
        assert!(
            matches!(
                noun.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "Root should be stack-allocated"
        );

        unsafe { noun.copy_to_pma(&stack, &mut pma) };

        // Should allocate 3 cells worth of space
        let cell_words = word_size_of::<CellMemory>();
        assert_eq!(
            pma.alloc_offset(),
            cell_words * 3,
            "Should allocate 3 cells"
        );

        // Verify root is in offset form
        assert!(
            !matches!(
                noun.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "Root should be in offset form"
        );

        // Navigate and verify structure
        let root = noun.in_space(&space).as_cell().expect("root is cell");
        let left_cell = root.head().as_cell().expect("left is cell");
        let right_cell = root.tail().as_cell().expect("right is cell");

        // Verify left cell [1 2]
        assert!(
            !matches!(root.head().allocated_location(), Some(AllocLocation::Stack)),
            "Left should be in offset form"
        );
        assert_eq!(left_cell.head().noun().as_direct().expect("1").data(), 1);
        assert_eq!(left_cell.tail().noun().as_direct().expect("2").data(), 2);

        // Verify right cell [3 4]
        assert!(
            !matches!(root.tail().allocated_location(), Some(AllocLocation::Stack)),
            "Right should be in offset form"
        );
        assert_eq!(right_cell.head().noun().as_direct().expect("3").data(), 3);
        assert_eq!(right_cell.tail().noun().as_direct().expect("4").data(), 4);

        // Verify assert_in_pma passes for entire structure
        noun.assert_in_pma(&pma);
    }

    /// Verifies cells containing indirect atoms have both the cell and atoms correctly evacuated.
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_evacuate_cell_with_indirect_atoms() {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let mut pma = test_pma(1000);
        let space = NounSpace::new(&stack, &pma);

        // Create indirect atoms
        let data1: [u64; 2] = [0xAAAAAAAA_BBBBBBBB, 0xCCCCCCCC_DDDDDDDD];
        let data2: [u64; 2] = [0x11111111_22222222, 0x33333333_44444444];
        let indirect1 = unsafe { IndirectAtom::new_raw(&mut stack, 2, data1.as_ptr()) };
        let indirect2 = unsafe { IndirectAtom::new_raw(&mut stack, 2, data2.as_ptr()) };

        // Create cell with indirect atoms
        let mut noun = Cell::new(&mut stack, indirect1.as_noun(), indirect2.as_noun()).as_noun();

        assert!(
            matches!(
                noun.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "Should be stack-allocated"
        );

        unsafe { noun.copy_to_pma(&stack, &mut pma) };

        // Should allocate: 1 cell + 2 indirect atoms (4 words each)
        let cell_words = word_size_of::<CellMemory>();
        let indirect_words = 4; // metadata + size + 2 data words
        assert_eq!(
            pma.alloc_offset(),
            cell_words + indirect_words * 2,
            "Should allocate cell + 2 indirect atoms"
        );

        // Verify structure
        assert!(
            !matches!(
                noun.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "Root should be in offset form"
        );

        let cell = noun.in_space(&space).as_cell().expect("is cell");
        let head = cell.head().noun();
        let tail = cell.tail().noun();

        // Verify head is indirect atom with correct data
        assert!(head.is_indirect(), "Head should be indirect");
        assert!(
            !matches!(
                head.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "Head should be in offset form"
        );
        let head_indirect = head.as_indirect().expect("head indirect");
        let head_handle = head_indirect.as_atom().in_space(&space);
        let head_data = unsafe { std::slice::from_raw_parts(head_handle.data_pointer(), 2) };
        assert_eq!(head_data[0], data1[0]);
        assert_eq!(head_data[1], data1[1]);

        // Verify tail is indirect atom with correct data
        assert!(tail.is_indirect(), "Tail should be indirect");
        assert!(
            !matches!(
                tail.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "Tail should be in offset form"
        );
        let tail_indirect = tail.as_indirect().expect("tail indirect");
        let tail_handle = tail_indirect.as_atom().in_space(&space);
        let tail_data = unsafe { std::slice::from_raw_parts(tail_handle.data_pointer(), 2) };
        assert_eq!(tail_data[0], data2[0]);
        assert_eq!(tail_data[1], data2[1]);

        noun.assert_in_pma(&pma);
    }

    /// Verifies structural sharing is preserved: [x x] evacuates x only once.
    ///
    /// When the same noun is referenced multiple times, the forwarding pointer
    /// mechanism ensures it's only copied once, and both references point to
    /// the same PMA location.
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_evacuate_shared_structure() {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let mut pma = test_pma(1000);
        let space = NounSpace::new(&stack, &pma);

        // Create a shared subcell
        let shared = Cell::new(&mut stack, D(1), D(2)).as_noun();

        // Create [shared shared] - both head and tail point to same cell
        let mut noun = Cell::new(&mut stack, shared, shared).as_noun();

        unsafe { noun.copy_to_pma(&stack, &mut pma) };

        // Should allocate only 2 cells: the root and the shared subcell (not 3!)
        let cell_words = word_size_of::<CellMemory>();
        assert_eq!(
            pma.alloc_offset(),
            cell_words * 2,
            "Should allocate only 2 cells due to sharing"
        );

        // Verify both head and tail point to the same PMA location
        let root = noun.in_space(&space).as_cell().expect("is cell");
        let head_raw = unsafe { root.head().noun().as_raw() };
        let tail_raw = unsafe { root.tail().noun().as_raw() };
        assert_eq!(
            head_raw, tail_raw,
            "Head and tail should point to same location (sharing preserved)"
        );

        // Verify the shared cell is correct
        let shared_cell = root.head().as_cell().expect("shared is cell");
        assert_eq!(shared_cell.head().noun().as_direct().expect("1").data(), 1);
        assert_eq!(shared_cell.tail().noun().as_direct().expect("2").data(), 2);

        noun.assert_in_pma(&pma);
    }

    /// Verifies evacuating an already-evacuated noun is a no-op that allocates nothing.
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_evacuate_already_evacuated() {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let mut pma = test_pma(1000);

        // Create and evacuate a cell
        let mut noun = Cell::new(&mut stack, D(1), D(2)).as_noun();
        unsafe { noun.copy_to_pma(&stack, &mut pma) };

        let offset_after_first = pma.alloc_offset();
        assert!(offset_after_first > 0, "Should have allocated something");

        // Evacuate again - should be a no-op
        unsafe { noun.copy_to_pma(&stack, &mut pma) };

        assert_eq!(
            pma.alloc_offset(),
            offset_after_first,
            "Second evacuation should not allocate anything"
        );

        noun.assert_in_pma(&pma);
    }

    /// Verifies deeply nested structures are fully evacuated and traversable after evacuation.
    ///
    /// This test exercises the worklist algorithm's ability to handle deep trees
    /// without stack overflow (since we use iteration, not recursion).
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_evacuate_deep_tree() {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let mut pma = test_pma(10000);
        let space = NounSpace::new(&stack, &pma);

        // Create a deeply nested structure: [1 [2 [3 [4 ... [999 1000]]]]]
        const DEPTH: u64 = 500;

        // Build from the inside out
        let mut noun = D(DEPTH);
        for i in (1..DEPTH).rev() {
            noun = Cell::new(&mut stack, D(i), noun).as_noun();
        }

        // Verify it's deeply nested and stack-allocated
        assert!(noun.is_cell(), "Root should be a cell");
        assert!(
            matches!(
                noun.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "Should be stack-allocated"
        );

        // Count the depth before evacuation
        let mut depth_before = 0u64;
        let mut current = noun;
        while current.is_cell() {
            depth_before += 1;
            current = current
                .in_space(&space)
                .as_cell()
                .expect("depth walk should find a cell")
                .tail()
                .noun();
        }
        assert_eq!(
            depth_before,
            DEPTH - 1,
            "Should have correct depth before evacuation"
        );

        // Evacuate
        unsafe { noun.copy_to_pma(&stack, &mut pma) };

        // Should allocate (DEPTH - 1) cells
        let cell_words = word_size_of::<CellMemory>();
        assert_eq!(
            pma.alloc_offset(),
            cell_words * (DEPTH as usize - 1),
            "Should allocate {} cells",
            DEPTH - 1
        );

        // Verify root is in offset form
        assert!(
            !matches!(
                noun.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "Root should be in offset form"
        );

        // Traverse the entire structure and verify values
        let mut current = noun;
        for expected in 1..DEPTH {
            assert!(current.is_cell(), "Should be cell at depth {}", expected);
            let cell = current.in_space(&space).as_cell().expect("is cell");

            // Verify head value
            let head = cell.head().noun();
            assert!(
                head.is_direct(),
                "Head at depth {} should be direct",
                expected
            );
            assert_eq!(
                head.as_direct().expect("direct").data(),
                expected,
                "Head at depth {} should be {}",
                expected,
                expected
            );

            // Verify this cell is in offset form
            assert!(
                !matches!(
                    current.in_space(&space).allocated_location(),
                    Some(AllocLocation::Stack)
                ),
                "Cell at depth {} should be in offset form",
                expected
            );

            current = cell.tail().noun();
        }

        // Final element should be direct atom DEPTH
        assert!(current.is_direct(), "Leaf should be direct atom");
        assert_eq!(
            current.as_direct().expect("direct").data(),
            DEPTH,
            "Leaf should be {}",
            DEPTH
        );

        // Verify assert_in_pma passes for entire structure
        noun.assert_in_pma(&pma);
    }

    /// Verifies deeply nested structures with variable-sized indirect atoms are fully evacuated.
    ///
    /// Similar to test_evacuate_deep_tree, but each value is an IndirectAtom with
    /// data size varying from 2 to 10 words. This tests the evacuation of mixed
    /// cell/indirect-atom structures with variable allocation sizes.
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_evacuate_deep_tree_indirect_atoms() {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let mut pma = test_pma(100000); // Larger PMA for indirect atoms
        let space = NounSpace::new(&stack, &pma);

        const DEPTH: usize = 200;

        // Helper to create an indirect atom with `word_count` words of data
        // Data pattern: first word is the index, remaining words are index + word_position
        let make_indirect = |stack: &mut NockStack, index: usize, word_count: usize| -> Noun {
            let mut data = vec![0u64; word_count];
            for (i, word) in data.iter_mut().enumerate() {
                *word = (index as u64) << 32 | (i as u64);
            }
            unsafe { IndirectAtom::new_raw(stack, word_count, data.as_ptr()).as_noun() }
        };

        // Helper to compute word count for index (varies 2-10)
        let word_count_for_index = |index: usize| -> usize { (index % 9) + 2 };

        // Build from inside out: [indirect_1 [indirect_2 [indirect_3 ... indirect_DEPTH]]]
        let mut noun = make_indirect(&mut stack, DEPTH, word_count_for_index(DEPTH));
        for i in (1..DEPTH).rev() {
            let head = make_indirect(&mut stack, i, word_count_for_index(i));
            noun = Cell::new(&mut stack, head, noun).as_noun();
        }

        // Verify structure before evacuation
        assert!(noun.is_cell(), "Root should be a cell");
        assert!(
            matches!(
                noun.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "Should be stack-allocated"
        );

        // Count expected allocations:
        // - (DEPTH - 1) cells
        // - DEPTH indirect atoms, each with (word_count + 2) words (metadata + size + data)
        let cell_words = word_size_of::<CellMemory>();
        let mut expected_indirect_words = 0usize;
        for i in 1..=DEPTH {
            expected_indirect_words += word_count_for_index(i) + 2; // +2 for metadata and size
        }
        let expected_total = (cell_words * (DEPTH - 1)) + expected_indirect_words;

        // Evacuate
        unsafe { noun.copy_to_pma(&stack, &mut pma) };

        // Verify allocation size
        assert_eq!(
            pma.alloc_offset(),
            expected_total,
            "Should allocate {} words total ({} cells + {} indirect atom words)",
            expected_total,
            DEPTH - 1,
            expected_indirect_words
        );

        // Verify root is in offset form
        assert!(
            !matches!(
                noun.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "Root should be in offset form"
        );

        // Traverse and verify all values
        let mut current = noun;
        for expected_index in 1..DEPTH {
            assert!(
                current.is_cell(),
                "Should be cell at depth {}",
                expected_index
            );
            let cell = current.in_space(&space).as_cell().expect("is cell");

            // Verify head is an indirect atom with correct data
            let head = cell.head().noun();
            assert!(
                head.is_indirect(),
                "Head at depth {} should be indirect",
                expected_index
            );
            assert!(
                !matches!(
                    head.in_space(&space).allocated_location(),
                    Some(AllocLocation::Stack)
                ),
                "Head at depth {} should be in offset form",
                expected_index
            );

            let head_indirect = head.as_indirect().expect("indirect");
            let head_handle = head_indirect.as_atom().in_space(&space);
            let expected_word_count = word_count_for_index(expected_index);
            assert_eq!(
                head_handle.size(),
                expected_word_count,
                "Indirect atom at depth {} should have {} words",
                expected_index,
                expected_word_count
            );

            // Verify data pattern
            let data_ptr = head_handle.data_pointer();
            for word_idx in 0..expected_word_count {
                let expected_value = (expected_index as u64) << 32 | (word_idx as u64);
                let actual_value = unsafe { *data_ptr.add(word_idx) };
                assert_eq!(
                    actual_value, expected_value,
                    "Data mismatch at depth {}, word {}",
                    expected_index, word_idx
                );
            }

            current = cell.tail().noun();
        }

        // Final element should be indirect atom for index DEPTH
        assert!(current.is_indirect(), "Leaf should be indirect atom");
        assert!(
            !matches!(
                current.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "Leaf should be in offset form"
        );

        let leaf_indirect = current.as_indirect().expect("indirect");
        let leaf_handle = leaf_indirect.as_atom().in_space(&space);
        let expected_leaf_words = word_count_for_index(DEPTH);
        assert_eq!(
            leaf_handle.size(),
            expected_leaf_words,
            "Leaf indirect atom should have {} words",
            expected_leaf_words
        );

        // Verify leaf data pattern
        let leaf_data_ptr = leaf_handle.data_pointer();
        for word_idx in 0..expected_leaf_words {
            let expected_value = (DEPTH as u64) << 32 | (word_idx as u64);
            let actual_value = unsafe { *leaf_data_ptr.add(word_idx) };
            assert_eq!(
                actual_value, expected_value,
                "Leaf data mismatch at word {}",
                word_idx
            );
        }

        // Verify assert_in_pma passes for entire structure
        noun.assert_in_pma(&pma);
    }

    /// Verifies NounAllocator::equals works through the Pma interface.
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_pma_noun_allocator_equals() {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let mut pma = test_pma(1000);

        let mut noun1 = Cell::new(&mut stack, D(1), D(2)).as_noun();
        let mut noun2 = Cell::new(&mut stack, D(1), D(2)).as_noun();
        let mut noun3 = Cell::new(&mut stack, D(1), D(3)).as_noun();

        unsafe {
            noun1.copy_to_pma(&stack, &mut pma);
            noun2.copy_to_pma(&stack, &mut pma);
            noun3.copy_to_pma(&stack, &mut pma);
        }

        // Test through NounAllocator trait
        assert!(
            unsafe { pma.equals(&mut noun1 as *mut Noun, &mut noun2 as *mut Noun) },
            "NounAllocator::equals should return true for equal nouns"
        );
        assert!(
            !unsafe { pma.equals(&mut noun1 as *mut Noun, &mut noun3 as *mut Noun) },
            "NounAllocator::equals should return false for unequal nouns"
        );
    }

    /// Verifies that a HAMT can be evacuated to PMA and lookups still work.
    ///
    /// This test exercises:
    /// - Creating a HAMT with multiple entries (direct atoms as keys/values)
    /// - Evacuating the entire HAMT structure to PMA
    /// - Verifying all entries are still retrievable via lookup
    /// - Verifying all internal pointers are in offset form (not stack-allocated)
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_evacuate_hamt_round_trip() {
        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let mut pma = test_pma(10000);
        let space = NounSpace::new(&stack, &pma);

        // Create a HAMT with several entries
        let mut hamt: Hamt<Noun> = Hamt::new(&mut stack);

        // Insert 10 key-value pairs
        for i in 0u64..10 {
            let mut key = D(i);
            let value = D(i * 100);
            hamt = hamt.insert(&mut stack, &mut key, value);
        }

        // Verify lookups work before evacuation
        for i in 0u64..10 {
            let mut key = D(i);
            let result = hamt.lookup(&mut stack, &mut key);
            assert!(
                result.is_some(),
                "Lookup for key {} should succeed before evacuation",
                i
            );
            let value = result.expect("lookup should return a value before evacuation");
            assert!(value.is_direct(), "Value should be direct atom");
            assert_eq!(
                value
                    .as_direct()
                    .expect("lookup value should be a direct atom")
                    .data(),
                i * 100,
                "Value for key {} should be {}",
                i,
                i * 100
            );
        }

        // Evacuate the HAMT to PMA
        unsafe {
            hamt.copy_to_pma(&stack, &mut pma);
        }

        // Verify entries are still present after evacuation
        let mut found = [false; 10];
        for entries in hamt.iter() {
            for (key, value) in entries {
                let key_direct = key.as_direct().expect("key should be direct");
                let value_direct = value.as_direct().expect("value should be direct");
                let idx = key_direct.data() as usize;
                assert!(
                    idx < found.len(),
                    "Key {} should be within expected range",
                    idx
                );
                assert_eq!(
                    value_direct.data(),
                    (idx as u64) * 100,
                    "Value for key {} should still be {} after evacuation",
                    idx,
                    (idx as u64) * 100
                );
                found[idx] = true;
            }
        }
        assert!(
            found.iter().all(|present| *present),
            "All keys should be present after evacuation"
        );

        // Verify internal structure is in PMA (offset form)
        // Iterate over the HAMT and check all nouns are not stack-allocated
        for entries in hamt.iter() {
            for (key, value) in entries {
                if !key.is_direct() {
                    assert!(
                        !matches!(
                            key.in_space(&space).allocated_location(),
                            Some(AllocLocation::Stack)
                        ),
                        "HAMT key should be in offset form after evacuation"
                    );
                }
                if !value.is_direct() {
                    assert!(
                        !matches!(
                            value.in_space(&space).allocated_location(),
                            Some(AllocLocation::Stack)
                        ),
                        "HAMT value should be in offset form after evacuation"
                    );
                }
            }
        }
    }

    /// Test that copy_to_pma correctly copies nouns to PMA and produces valid offset-form nouns.
    ///
    /// Note: copy_to_pma sets forwarding pointers in the source nouns, which corrupts
    /// them for normal use. This is by design for structural sharing. Therefore, we
    /// cannot compare source vs PMA copy directly. Instead, we verify the PMA copy
    /// contains the expected data.
    ///
    /// This test may look superfluous, but it helped debug test_evacuate_hamt_complex_nouns so
    /// that's why its in here.
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_copy_to_pma_preserves_data() {
        use crate::noun::{Cell, IndirectAtom};

        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let mut pma = test_pma(10000);
        let space = NounSpace::new(&stack, &pma);

        // Test with indirect atom
        let data: [u64; 2] = [0xDEADBEEF_CAFEBABE, 0x12345678_9ABCDEF0];
        let stack_indirect =
            unsafe { IndirectAtom::new_raw(&mut stack, 2, data.as_ptr()) }.as_noun();

        // Copy to PMA
        let mut pma_indirect = stack_indirect;
        unsafe { pma_indirect.copy_to_pma(&stack, &mut pma) };

        // Verify the PMA copy is in offset form
        assert!(
            !matches!(
                pma_indirect.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "PMA copy should be in offset form"
        );

        // Verify the PMA copy contains correct data
        let pma_ia = pma_indirect
            .as_indirect()
            .expect("PMA copy should be an indirect atom");
        let pma_handle = pma_ia.as_atom().in_space(&space);
        let pma_size = pma_handle.size();
        assert_eq!(pma_size, 2, "PMA indirect atom should have size 2");

        let pma_bytes = pma_handle.as_ne_bytes();
        assert_eq!(
            pma_bytes.len(),
            16,
            "PMA indirect should have 16 bytes of data"
        );

        // Verify actual data values
        let pma_slice =
            unsafe { std::slice::from_raw_parts(pma_handle.data_pointer(), pma_handle.size()) };
        assert_eq!(pma_slice[0], 0xDEADBEEF_CAFEBABE, "First word should match");
        assert_eq!(
            pma_slice[1], 0x12345678_9ABCDEF0,
            "Second word should match"
        );

        // Test with cell containing direct atoms
        let stack_cell = Cell::new(&mut stack, D(42), D(99)).as_noun();
        let mut pma_cell = stack_cell;
        unsafe { pma_cell.copy_to_pma(&stack, &mut pma) };

        assert!(
            !matches!(
                pma_cell.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "PMA cell should be in offset form"
        );
        let cell = pma_cell
            .in_space(&space)
            .as_cell()
            .expect("PMA noun should be a cell");
        assert_eq!(
            cell.head()
                .noun()
                .as_direct()
                .expect("cell head should be direct")
                .data(),
            42,
            "Cell head should be 42"
        );
        assert_eq!(
            cell.tail()
                .noun()
                .as_direct()
                .expect("cell tail should be direct")
                .data(),
            99,
            "Cell tail should be 99"
        );

        // Test with nested structure
        let inner = Cell::new(&mut stack, D(1), D(2)).as_noun();
        let stack_nested = Cell::new(&mut stack, inner, D(3)).as_noun();
        let mut pma_nested = stack_nested;
        unsafe { pma_nested.copy_to_pma(&stack, &mut pma) };

        assert!(
            !matches!(
                pma_nested.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ),
            "PMA nested should be in offset form"
        );
        let outer = pma_nested
            .in_space(&space)
            .as_cell()
            .expect("PMA nested noun should be an outer cell");
        assert_eq!(
            outer
                .tail()
                .noun()
                .as_direct()
                .expect("outer tail should be direct")
                .data(),
            3,
            "Outer tail should be 3"
        );
        let inner_cell = outer.head().as_cell().expect("outer head should be a cell");
        assert_eq!(
            inner_cell
                .head()
                .noun()
                .as_direct()
                .expect("inner head should be direct")
                .data(),
            1,
            "Inner head should be 1"
        );
        assert_eq!(
            inner_cell
                .tail()
                .noun()
                .as_direct()
                .expect("inner tail should be direct")
                .data(),
            2,
            "Inner tail should be 2"
        );
    }

    /// Test HAMT evacuation with complex noun types: Cells and IndirectAtoms.
    ///
    /// This test exercises:
    /// - HAMT with indirect atoms as keys (large numbers)
    /// - HAMT with cells as values (nested structures)
    /// - Deep cell nesting to test recursive evacuation
    /// - Structural equality verification using a reference copy on a separate stack
    ///
    /// Note: copy_to_pma sets forwarding pointers in source nouns, corrupting them.
    /// To verify values, we create a second NockStack with fresh copies of the same
    /// data and compare those against the PMA copy using noun_equality.
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_evacuate_hamt_complex_nouns() {
        use crate::ext::noun_equality;
        use crate::noun::{Cell, IndirectAtom};

        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let mut pma = test_pma(100000);

        // Create a second stack with reference copies of keys/values for comparison
        // This stack won't be corrupted by forwarding pointers
        let mut ref_stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let space = NounSpace::new(&stack, &pma);
        let ref_space = NounSpace::new(&ref_stack, &pma);

        let mut hamt: Hamt<Noun> = Hamt::new(&mut stack);

        // Store reference keys/values on the separate stack
        let mut ref_keys: Vec<Noun> = Vec::new();
        let mut ref_values: Vec<Noun> = Vec::new();

        // Insert entries with indirect atom keys and cell values
        for i in 0u64..5 {
            let key_data: [u64; 2] = [0xDEADBEEF_CAFEBABE + i, 0x12345678_9ABCDEF0 + i];

            // Create on main stack for HAMT
            let key_atom =
                unsafe { IndirectAtom::new_raw(&mut stack, 2, key_data.as_ptr()) }.as_noun();
            let inner = Cell::new(&mut stack, D(i + 100), D(i + 200)).as_noun();
            let value = Cell::new(&mut stack, D(i), inner).as_noun();

            // Create identical copies on reference stack
            let ref_key =
                unsafe { IndirectAtom::new_raw(&mut ref_stack, 2, key_data.as_ptr()) }.as_noun();
            let ref_inner = Cell::new(&mut ref_stack, D(i + 100), D(i + 200)).as_noun();
            let ref_value = Cell::new(&mut ref_stack, D(i), ref_inner).as_noun();
            ref_keys.push(ref_key);
            ref_values.push(ref_value);

            let mut key_copy = key_atom;
            hamt = hamt.insert(&mut stack, &mut key_copy, value);
        }

        // Insert entries with cell keys and indirect atom values
        for i in 5u64..10 {
            let val_data: [u64; 2] = [i * 1000, i * 2000];

            // Create on main stack for HAMT
            let key = Cell::new(&mut stack, D(i), D(i + 1)).as_noun();
            let value =
                unsafe { IndirectAtom::new_raw(&mut stack, 2, val_data.as_ptr()) }.as_noun();

            // Create identical copies on reference stack
            let ref_key = Cell::new(&mut ref_stack, D(i), D(i + 1)).as_noun();
            let ref_value =
                unsafe { IndirectAtom::new_raw(&mut ref_stack, 2, val_data.as_ptr()) }.as_noun();
            ref_keys.push(ref_key);
            ref_values.push(ref_value);

            let mut key_copy = key;
            hamt = hamt.insert(&mut stack, &mut key_copy, value);
        }

        // Insert entries with deeply nested cells
        for i in 10u64..12 {
            // Create on main stack for HAMT
            let ab = Cell::new(&mut stack, D(i), D(i + 1)).as_noun();
            let abc = Cell::new(&mut stack, ab, D(i + 2)).as_noun();
            let key = Cell::new(&mut stack, abc, D(i + 3)).as_noun();
            let zw = Cell::new(&mut stack, D(i + 10), D(i + 11)).as_noun();
            let yzw = Cell::new(&mut stack, D(i + 9), zw).as_noun();
            let value = Cell::new(&mut stack, D(i + 8), yzw).as_noun();

            // Create identical copies on reference stack
            let ref_ab = Cell::new(&mut ref_stack, D(i), D(i + 1)).as_noun();
            let ref_abc = Cell::new(&mut ref_stack, ref_ab, D(i + 2)).as_noun();
            let ref_key = Cell::new(&mut ref_stack, ref_abc, D(i + 3)).as_noun();
            let ref_zw = Cell::new(&mut ref_stack, D(i + 10), D(i + 11)).as_noun();
            let ref_yzw = Cell::new(&mut ref_stack, D(i + 9), ref_zw).as_noun();
            let ref_value = Cell::new(&mut ref_stack, D(i + 8), ref_yzw).as_noun();
            ref_keys.push(ref_key);
            ref_values.push(ref_value);

            let mut key_copy = key;
            hamt = hamt.insert(&mut stack, &mut key_copy, value);
        }

        // Count entries before evacuation
        let count_before: usize = hamt.iter().map(|entries| entries.len()).sum();
        assert_eq!(count_before, 12, "Should have 12 entries before evacuation");

        // Evacuate the HAMT to PMA
        unsafe {
            hamt.copy_to_pma(&stack, &mut pma);
        }

        // Count entries after evacuation
        let count_after: usize = hamt.iter().map(|entries| entries.len()).sum();
        assert_eq!(
            count_after, count_before,
            "Entry count should be preserved after evacuation"
        );

        // Verify all values match by comparing PMA nouns to reference stack nouns
        let mut found_count = 0;
        for entries in hamt.iter() {
            for (pma_key, pma_value) in entries {
                // Find matching reference key and verify value matches
                let mut found = false;
                for (idx, ref_key) in ref_keys.iter().enumerate() {
                    if noun_equality(
                        (*pma_key).in_space(&ref_space),
                        (*ref_key).in_space(&ref_space),
                    ) {
                        assert!(
                            noun_equality(
                                (*pma_value).in_space(&ref_space),
                                ref_values[idx].in_space(&ref_space),
                            ),
                            "Value for key {} should match reference after evacuation",
                            idx
                        );
                        found = true;
                        found_count += 1;
                        break;
                    }
                }
                assert!(found, "Every PMA key should match a reference key");
            }
        }
        assert_eq!(
            found_count,
            ref_keys.len(),
            "Should find all {} entries in HAMT after evacuation",
            ref_keys.len()
        );

        // Verify all nouns in the HAMT are in offset form
        for entries in hamt.iter() {
            for (key, value) in entries {
                verify_noun_not_stack_allocated(*key, &space, "HAMT key");
                verify_noun_not_stack_allocated(*value, &space, "HAMT value");
            }
        }

        // Verify the HAMT structure itself is in PMA
        hamt.assert_in_pma(&pma);
    }

    /// Helper to recursively verify a noun is not stack-allocated
    fn verify_noun_not_stack_allocated(noun: Noun, space: &NounSpace, context: &str) {
        if noun.is_direct() {
            return;
        }

        let location = noun.in_space(space).allocated_location();
        assert!(
            !matches!(location, Some(AllocLocation::Stack)),
            "{} should be in offset form after evacuation",
            context
        );

        if let Ok(cell) = noun.in_space(space).as_cell() {
            verify_noun_not_stack_allocated(cell.head().noun(), space, context);
            verify_noun_not_stack_allocated(cell.tail().noun(), space, context);
        }
    }

    /// Verifies that PmaCopy for () is a no-op that allocates nothing.
    ///
    /// The unit type has no data, so copy_to_pma should not allocate anything
    /// and assert_in_pma should trivially pass.
    #[test]
    #[cfg_attr(miri, ignore = "memfd_create unsupported in Miri")]
    fn test_evacuate_unit() {
        let stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let mut pma = test_pma(1000);

        let mut unit = ();
        let initial_offset = pma.alloc_offset();

        // Copy to PMA - should be a no-op
        unsafe { unit.copy_to_pma(&stack, &mut pma) };

        // Verify no allocations were made
        assert_eq!(
            pma.alloc_offset(),
            initial_offset,
            "No allocations should be made for unit type"
        );

        // assert_in_pma should not panic
        unit.assert_in_pma(&pma);
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod paging_tests {
    use crate::pma::{madvise_drop_file_backed_pages, test_pma_path, Pma};

    const SLAB_BYTES: usize = 64 * 1024 * 1024;
    const TOUCH_PAGES: usize = 64;

    #[test]
    #[cfg_attr(miri, ignore = "mincore/madvise unsupported in Miri")]
    fn pma_file_backed_pages_out_and_faults_lazily() {
        let words = SLAB_BYTES >> 3;
        let path = test_pma_path("paging");
        let pma = Pma::new(words, path).expect("failed to create PMA");
        let base = pma.arena().base_ptr();
        let len = pma.arena().len_bytes();
        let page = page_size();

        assert_eq!(len, SLAB_BYTES, "unexpected PMA length");
        assert_eq!(
            len % page,
            0,
            "PMA length must be page sized, len={len}, page={page}"
        );

        touch_entire_region(base, len, page);
        let resident_bitmap = mincore_bitmap(base, len);
        let initial_ratio = residency_ratio(&resident_bitmap);
        println!("[pma-paging] initial residency ratio {:.3}", initial_ratio);
        assert!(
            resident_bitmap.iter().all(|b| b & 1 == 1),
            "expected fully resident slab after touching every page"
        );

        drop_all_pages(base, len);
        let after_drop = mincore_bitmap(base, len);
        let post_drop_ratio = residency_ratio(&after_drop);
        let dropped_pages = nonresident_page_indices(&after_drop);
        println!(
            "[pma-paging] post-drop residency ratio {:.3}; dropped {} pages",
            post_drop_ratio,
            dropped_pages.len()
        );
        if dropped_pages.is_empty() {
            println!(
                "[pma-paging] paging did not drop pages; skipping remainder (ratio={post_drop_ratio:.3})"
            );
            return;
        }

        let total_pages = len / page;
        let touched_pages = sparse_page_indices(&dropped_pages, TOUCH_PAGES);
        assert!(
            !touched_pages.is_empty(),
            "expected to fault at least one dropped page"
        );
        fault_pages(base, page, &touched_pages);

        let post_fault = mincore_bitmap(base, len);
        let post_fault_ratio = residency_ratio(&post_fault);
        let mut touched_page_bitmap = vec![false; total_pages];
        for page_idx in &touched_pages {
            touched_page_bitmap[*page_idx] = true;
        }
        let touched_resident = touched_pages
            .iter()
            .filter(|page_idx| post_fault[**page_idx] & 1 == 1)
            .count();
        let extra_refaulted = dropped_pages
            .iter()
            .filter(|page_idx| !touched_page_bitmap[**page_idx] && post_fault[**page_idx] & 1 == 1)
            .count();
        let untouched_dropped_pages = dropped_pages.len().saturating_sub(touched_pages.len());
        let max_extra_refaulted = (touched_pages.len() * 16)
            .max(8)
            .min(untouched_dropped_pages);
        println!(
            "[pma-paging] post-fault residency ratio {:.4}; touched {} dropped pages; extra refaulted {}",
            post_fault_ratio,
            touched_pages.len(),
            extra_refaulted
        );
        assert_eq!(
            touched_resident,
            touched_pages.len(),
            "all touched dropped pages should become resident after sparse faults"
        );
        assert!(
            extra_refaulted <= max_extra_refaulted,
            "dropped pages refaulted too broadly: extra={} allowed={} dropped={} touched={}",
            extra_refaulted,
            max_extra_refaulted,
            dropped_pages.len(),
            touched_pages.len()
        );
    }

    fn touch_entire_region(ptr: *mut u8, len: usize, page: usize) {
        for offset in (0..len).step_by(page) {
            unsafe {
                std::ptr::write_volatile(ptr.add(offset), (offset / page % 255) as u8);
            }
        }
    }

    fn sparse_page_indices(candidates: &[usize], desired_pages: usize) -> Vec<usize> {
        if candidates.is_empty() || desired_pages == 0 {
            return Vec::new();
        }
        let touches = desired_pages.min(candidates.len());
        let mut pages = Vec::with_capacity(touches);
        for touch_idx in 0..touches {
            let candidate_idx = touch_idx * candidates.len() / touches;
            pages.push(candidates[candidate_idx]);
        }
        pages
    }

    fn fault_pages(ptr: *mut u8, page: usize, page_indices: &[usize]) {
        for page_idx in page_indices {
            unsafe {
                std::ptr::read_volatile(ptr.add(*page_idx * page));
            }
        }
    }

    fn drop_all_pages(ptr: *mut u8, len: usize) {
        madvise_drop_file_backed_pages(ptr as *mut libc::c_void, len)
            .expect("failed to advise file-backed PMA pages out");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    fn mincore_bitmap(ptr: *mut u8, len: usize) -> Vec<u8> {
        let page = page_size();
        assert_eq!(
            len % page,
            0,
            "mincore requires len to be page sized, len={len}, page={page}"
        );
        let pages = len / page;
        let mut vec = vec![0u8; pages];
        let ret = unsafe { libc::mincore(ptr as *mut libc::c_void, len, vec.as_mut_ptr().cast()) };
        if ret != 0 {
            panic!("mincore failed: {}", std::io::Error::last_os_error());
        }
        vec
    }

    fn residency_ratio(bitmap: &[u8]) -> f64 {
        if bitmap.is_empty() {
            return 0.0;
        }
        let resident = bitmap.iter().filter(|b| **b & 1 == 1).count();
        resident as f64 / bitmap.len() as f64
    }

    fn nonresident_page_indices(bitmap: &[u8]) -> Vec<usize> {
        bitmap
            .iter()
            .enumerate()
            .filter_map(|(page_idx, byte)| (byte & 1 == 0).then_some(page_idx))
            .collect()
    }

    fn page_size() -> usize {
        unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
    }
}
