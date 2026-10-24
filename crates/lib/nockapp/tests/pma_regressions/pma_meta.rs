use std::error::Error;
use std::fs;
use std::path::Path;

use bincode::{config, Decode, Encode};
use blake3::{Hash, Hasher};

const PMA_PERSIST_MAGIC: u64 = u64::from_le_bytes(*b"PMAPERS1");
const PMA_PERSIST_VERSION: u32 = 5;
const PMA_PERSIST_VERSION_V4: u32 = 4;

#[derive(Clone, Debug)]
pub(crate) struct PmaPersistMetadataForTest {
    pub(crate) ker_hash: Hash,
    pub(crate) event_num: u64,
    pub(crate) kernel_state_raw: u64,
    pub(crate) pma_reserved_words: Option<u64>,
}

#[derive(Clone, Encode, Decode, Debug)]
struct PmaPersistMetadataV5ForTest {
    magic: u64,
    version: u32,
    #[bincode(with_serde)]
    ker_hash: Hash,
    event_num: u64,
    kernel_state_raw: u64,
    pma_reserved_words: u64,
    #[bincode(with_serde)]
    checksum: Hash,
}

#[derive(Clone, Encode, Decode, Debug)]
struct PmaPersistMetadataV4ForTest {
    magic: u64,
    version: u32,
    #[bincode(with_serde)]
    ker_hash: Hash,
    event_num: u64,
    kernel_state_raw: u64,
    #[bincode(with_serde)]
    checksum: Hash,
}

impl PmaPersistMetadataForTest {
    pub(crate) fn load(path: &Path) -> Result<Self, Box<dyn Error>> {
        let bytes = fs::read(path)?;
        if let Ok((meta, _)) = bincode::decode_from_slice::<
            PmaPersistMetadataV5ForTest,
            config::Configuration,
        >(&bytes, config::standard())
        {
            if meta.validate() {
                return Ok(meta.into_current());
            }
        }
        if let Ok((meta, _)) = bincode::decode_from_slice::<
            PmaPersistMetadataV4ForTest,
            config::Configuration,
        >(&bytes, config::standard())
        {
            if meta.validate() {
                return Ok(meta.into_current());
            }
        }
        Err(std::io::Error::other(format!(
            "invalid PMA persist metadata at {}",
            path.display()
        ))
        .into())
    }

    pub(crate) fn save_v4_to_path(&self, path: &Path) -> Result<(), Box<dyn Error>> {
        let meta =
            PmaPersistMetadataV4ForTest::new(self.ker_hash, self.event_num, self.kernel_state_raw);
        let bytes = bincode::encode_to_vec(meta, config::standard())?;
        fs::write(path, bytes)?;
        Ok(())
    }
}

impl PmaPersistMetadataV5ForTest {
    fn validate(&self) -> bool {
        self.magic == PMA_PERSIST_MAGIC
            && self.version == PMA_PERSIST_VERSION
            && self.checksum
                == Self::checksum(
                    self.ker_hash, self.event_num, self.kernel_state_raw, self.pma_reserved_words,
                )
    }

    fn checksum(
        ker_hash: Hash,
        event_num: u64,
        kernel_state_raw: u64,
        pma_reserved_words: u64,
    ) -> Hash {
        let mut hasher = Hasher::new();
        hasher.update(ker_hash.as_bytes());
        hasher.update(&event_num.to_le_bytes());
        hasher.update(&kernel_state_raw.to_le_bytes());
        hasher.update(&pma_reserved_words.to_le_bytes());
        hasher.finalize()
    }

    fn into_current(self) -> PmaPersistMetadataForTest {
        PmaPersistMetadataForTest {
            ker_hash: self.ker_hash,
            event_num: self.event_num,
            kernel_state_raw: self.kernel_state_raw,
            pma_reserved_words: Some(self.pma_reserved_words),
        }
    }
}

impl PmaPersistMetadataV4ForTest {
    fn new(ker_hash: Hash, event_num: u64, kernel_state_raw: u64) -> Self {
        let checksum = Self::checksum(ker_hash, event_num, kernel_state_raw);
        Self {
            magic: PMA_PERSIST_MAGIC,
            version: PMA_PERSIST_VERSION_V4,
            ker_hash,
            event_num,
            kernel_state_raw,
            checksum,
        }
    }

    fn validate(&self) -> bool {
        self.magic == PMA_PERSIST_MAGIC
            && self.version == PMA_PERSIST_VERSION_V4
            && self.checksum == Self::checksum(self.ker_hash, self.event_num, self.kernel_state_raw)
    }

    fn checksum(ker_hash: Hash, event_num: u64, kernel_state_raw: u64) -> Hash {
        let mut hasher = Hasher::new();
        hasher.update(ker_hash.as_bytes());
        hasher.update(&event_num.to_le_bytes());
        hasher.update(&kernel_state_raw.to_le_bytes());
        hasher.finalize()
    }

    fn into_current(self) -> PmaPersistMetadataForTest {
        PmaPersistMetadataForTest {
            ker_hash: self.ker_hash,
            event_num: self.event_num,
            kernel_state_raw: self.kernel_state_raw,
            pma_reserved_words: None,
        }
    }
}
