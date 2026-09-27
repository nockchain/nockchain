//! Independent logical model and hand-encoded PMA fixtures for delta tests.
//! No benchmark engine or PMA reader code is used to build these fixtures.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use bincode::Encode;

#[derive(Clone, Debug)]
pub enum Value {
    Atom(Vec<u8>),
    Cell(usize, usize),
}

#[derive(Clone, Debug, Default)]
pub struct Model {
    pub nodes: Vec<Value>,
}

impl Model {
    pub fn atom(&mut self, bytes: &[u8]) -> usize {
        let mut bytes = bytes.to_vec();
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        let id = self.nodes.len();
        self.nodes.push(Value::Atom(bytes));
        id
    }

    pub fn number(&mut self, value: u64) -> usize {
        self.atom(&value.to_le_bytes())
    }

    pub fn cell(&mut self, head: usize, tail: usize) -> usize {
        assert!(head < self.nodes.len() && tail < self.nodes.len());
        let id = self.nodes.len();
        self.nodes.push(Value::Cell(head, tail));
        id
    }
}

pub struct Scratch(PathBuf);
impl Scratch {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            "noun-delta-oracle-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(name);
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy)]
pub struct Layout {
    pub padding_words: usize,
    pub metadata: u64,
    pub force_indirect: bool,
    pub atom_padding_words: usize,
}
impl Default for Layout {
    fn default() -> Self {
        Self {
            padding_words: 8,
            metadata: 0,
            force_indirect: false,
            atom_padding_words: 0,
        }
    }
}

pub struct Fixture {
    pub pma: PathBuf,
    pub manifest: PathBuf,
    pub root: u64,
    pub raw_nodes: Vec<u64>,
    pub original_pma: Vec<u8>,
    pub original_manifest: Vec<u8>,
}

// An independent encoding of the on-disk bincode manifest contract.
#[derive(Encode)]
enum Kind {
    Epoch,
}
#[derive(Encode)]
struct Payload {
    magic: u64,
    version: u32,
    kind: Kind,
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

impl Fixture {
    pub fn write(dir: &Path, name: &str, model: &Model, root: usize, layout: Layout) -> Self {
        let mut words = vec![0xfeed_face_cafe_beef; layout.padding_words];
        let mut raw_nodes = Vec::with_capacity(model.nodes.len());
        for value in &model.nodes {
            match value {
                Value::Atom(bytes) => {
                    let direct = if bytes.len() <= 8 {
                        let mut storage = [0u8; 8];
                        storage[..bytes.len()].copy_from_slice(bytes);
                        let n = u64::from_le_bytes(storage);
                        (n < 1 << 63).then_some(n)
                    } else {
                        None
                    };
                    if !layout.force_indirect && direct.is_some() {
                        raw_nodes.push(direct.unwrap());
                    } else {
                        let offset = words.len() as u64;
                        let atom_words = bytes.len().div_ceil(8).max(1) + layout.atom_padding_words;
                        words.push(layout.metadata);
                        words.push(atom_words as u64);
                        for index in 0..atom_words {
                            let mut word = [0u8; 8];
                            let start = index * 8;
                            let end = (start + 8).min(bytes.len());
                            if start < end {
                                word[..end - start].copy_from_slice(&bytes[start..end]);
                            }
                            words.push(u64::from_le_bytes(word));
                        }
                        raw_nodes.push((1 << 63) | (1 << 60) | offset);
                    }
                }
                Value::Cell(head, tail) => {
                    let offset = words.len() as u64;
                    words.push(layout.metadata);
                    words.push(raw_nodes[*head]);
                    words.push(raw_nodes[*tail]);
                    raw_nodes.push((3 << 62) | (1 << 60) | offset);
                }
            }
        }
        let alloc_words = words.len() as u64;
        let mut data: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        let used_blake3 = *blake3::hash(&data).as_bytes();
        let capacity_words = alloc_words + 16;
        data.resize(capacity_words as usize * 8, 0);
        // Legacy trailer is still explicitly supported by NockVM and this reader.
        for word in [
            u64::from_le_bytes(*b"NOCKPMA1"),
            1,
            capacity_words,
            alloc_words,
        ] {
            data.extend_from_slice(&word.to_le_bytes());
        }
        let root_raw = raw_nodes[root];
        let payload = Payload {
            magic: u64::from_le_bytes(*b"SNAPMAN1"),
            version: 2,
            kind: Kind::Epoch,
            timestamp_tag: name.to_owned(),
            ker_hash: [0x42; 32],
            event_num: 42,
            pma_words: capacity_words,
            alloc_words,
            kernel_root_raw: root_raw,
            cold_offset: 0,
            used_blake3,
            structure_blake3: None,
            created_at_ms: 0,
        };
        let mut manifest = bincode::encode_to_vec(&payload, bincode::config::standard()).unwrap();
        let checksum = *blake3::hash(&manifest).as_bytes();
        // The checksum is the final fixed-size array field, with no length prefix.
        manifest.extend_from_slice(&checksum);
        let pma_path = dir.join(format!("{name}.pma"));
        let manifest_path = dir.join(format!("{name}.manifest"));
        fs::write(&pma_path, &data).unwrap();
        fs::write(&manifest_path, &manifest).unwrap();
        Self {
            pma: pma_path,
            manifest: manifest_path,
            root: root_raw,
            raw_nodes,
            original_pma: data,
            original_manifest: manifest,
        }
    }

    pub fn assert_unchanged(&self) {
        assert!(
            fs::read(&self.pma).unwrap() == self.original_pma,
            "source PMA mutated"
        );
        assert!(
            fs::read(&self.manifest).unwrap() == self.original_manifest,
            "source manifest mutated"
        );
    }
}
