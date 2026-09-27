//! Read-only noun-value Merkle indexes and base-relative DAG envelopes.
//! Hash matches assume collision resistance at the selected fingerprint width.
//! Default is BLAKE3-256; `compact-hash` is an explicit truncated128 experiment.
//! `verify_equal` separately
//! compares actual atom values and cell structure, without relying on hashes.
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;

use crate::pma::{Node, Snapshot};

pub const DIGEST_BYTES: usize = if cfg!(feature = "compact-hash") {
    16
} else {
    32
};
pub type Digest = [u8; DIGEST_BYTES];
const INDEX_HEADER_BYTES: u64 = 24 + 2 * DIGEST_BYTES as u64;
const INDEX_RECORD_BYTES: u64 = 8 + DIGEST_BYTES as u64;
const DELTA_HEADER_BYTES: u64 = 24 + 3 * DIGEST_BYTES as u64;
const INCREMENTAL_HEADER_BYTES: u64 = 32 + 3 * DIGEST_BYTES as u64;
const INDEX_MAGIC: &[u8; 8] = if cfg!(feature = "compact-hash") {
    b"NNIX1281"
} else {
    b"NNIDX001"
};
const DELTA_MAGIC: &[u8; 8] = if cfg!(feature = "compact-hash") {
    b"NNDL1281"
} else {
    b"NNDLT001"
};
const INCREMENTAL_MAGIC: &[u8; 8] = if cfg!(feature = "compact-hash") {
    b"NNIN1281"
} else {
    b"NNINC001"
};
fn digest(hash: blake3::Hash) -> Digest {
    hash.as_bytes()[..DIGEST_BYTES]
        .try_into()
        .expect("selected digest width")
}
const DIRECT_LIMIT: u64 = 1 << 63;
const LOCAL_TAG: u64 = 1 << 63;
const BASE_TAG: u64 = 3 << 62;
const REF_MASK: u64 = (1 << 62) - 1;
const PENDING: u32 = u32::MAX;
const ATOM_DOMAIN: &[u8] = b"nockchain-noun-atom-v1\0";
const CELL_DOMAIN: &[u8] = b"nockchain-noun-cell-v1\0";

/// All direct handles are the atom value (< 2^63); other handles have bit 63 set.
/// The caller keeps backing files immutable for the lifetime of the graph/index.
pub trait Graph {
    fn root(&self) -> u64;
    fn node(&self, raw: u64) -> Result<Node<'_>>;
}
impl Graph for Snapshot {
    fn root(&self) -> u64 {
        Snapshot::root(self)
    }
    fn node(&self, raw: u64) -> Result<Node<'_>> {
        Snapshot::node(self, raw)
    }
}

// Benchmark-only integer-key hasher: PMA handles are not attacker-selected keys.
// Full content fingerprints are always checked after a 64-bit lookup hit.
#[derive(Default)]
struct IntegerHasher(u64);
impl Hasher for IntegerHasher {
    fn finish(&self) -> u64 {
        let mut x = self.0;
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
        x ^ (x >> 31)
    }
    fn write_u64(&mut self, value: u64) {
        self.0 = self
            .0
            .rotate_left(27)
            .wrapping_add(value)
            .wrapping_add(0x9e3779b97f4a7c15);
    }
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut value = [0; 8];
            value[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(value));
        }
    }
}
type IntMap<V> = HashMap<u64, V, BuildHasherDefault<IntegerHasher>>;
type PairSet = HashSet<(u64, u64), BuildHasherDefault<IntegerHasher>>;

#[derive(Clone, Copy, Debug)]
pub struct BuildOptions {
    pub max_nodes: Option<usize>,
    pub progress_every: usize,
}
impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            max_nodes: None,
            progress_every: 10_000_000,
        }
    }
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct IndexStats {
    pub fingerprint_bits: usize,
    pub nodes: usize,
    pub cells: usize,
    pub indirect_atoms: usize,
    pub atom_bytes: u64,
    pub direct_references: u64,
    pub memo_hits: u64,
    pub max_stack_frames: usize,
    pub elapsed_secs: f64,
    pub estimated_index_bytes: u64,
    pub serialized_index_bytes: u64,
    pub inherited_nodes: usize,
    pub incremental_index_bytes: u64,
    pub index_depth: usize,
    pub reachable_nodes_exact: bool,
    pub incremental_sidecar_bytes: Option<u64>,
}
#[derive(Clone, Copy)]
struct IndexedNode {
    raw: u64,
    hash: Digest,
}

#[derive(Clone)]
pub struct MerkleIndex {
    root: u64,
    root_hash: Digest,
    layout_hash: Digest,
    nodes: Arc<Vec<IndexedNode>>,
    ids: Arc<IntMap<u32>>,
    parent: Option<Arc<MerkleIndex>>,
    direct_hashes: [Digest; 256],
    stats: IndexStats,
}
impl MerkleIndex {
    pub fn root_hash(&self) -> Digest {
        self.root_hash
    }
    pub fn layout_hash(&self) -> Digest {
        self.layout_hash
    }
    pub fn stats(&self) -> &IndexStats {
        &self.stats
    }
    pub fn node_count(&self) -> usize {
        // Published indexes are immutable; cache the retained total so inherited
        // entry lookup does not repeatedly walk the ancestor chain to count it.
        self.stats.nodes
    }
    fn entry(&self, id: usize) -> Option<IndexedNode> {
        if let Some(parent) = &self.parent {
            let inherited = parent.node_count();
            if id < inherited {
                return parent.entry(id).map(|node| IndexedNode {
                    raw: NounRef::base(id as u32).0,
                    hash: node.hash,
                });
            }
            return self.nodes.get(id - inherited).copied();
        }
        self.nodes.get(id).copied()
    }
    fn id_of(&self, raw: u64) -> Result<u32> {
        if let Some(parent) = &self.parent {
            let reference = NounRef(raw);
            ensure!(!reference.is_direct(), "immediate noun has no index entry");
            let inherited = parent.node_count();
            let id = if reference.is_base() {
                ensure!(
                    reference.index() < inherited,
                    "base handle outside derived index"
                );
                reference.index()
            } else {
                ensure!(
                    reference.index() < self.nodes.len(),
                    "local handle outside derived index"
                );
                inherited + reference.index()
            };
            return Ok(id as u32);
        }
        let id = *self
            .ids
            .get(&raw)
            .context("noun missing from Merkle index")?;
        ensure!(id != PENDING, "cycle or unfinished child in noun graph");
        Ok(id)
    }
    fn hash_of(&self, raw: u64) -> Result<Digest> {
        if raw < DIRECT_LIMIT {
            return Ok(self.direct_hash(raw));
        }
        Ok(self
            .entry(self.id_of(raw)? as usize)
            .context("index ID out of bounds")?
            .hash)
    }
    fn direct_hash(&self, value: u64) -> Digest {
        if value < 256 {
            self.direct_hashes[value as usize]
        } else {
            hash_direct(value)
        }
    }
    /// Fixed-width cache sidecar: magic, root handle, root hash, layout hash,
    /// node count, then (source handle u64 LE, fingerprint DIGEST_BYTES bytes) per node.
    /// This cache is optional; its construction and size are benchmark costs.
    pub fn write_to(&self, out: &mut impl Write) -> Result<u64> {
        out.write_all(INDEX_MAGIC)?;
        put_u64(out, self.root)?;
        out.write_all(&self.root_hash)?;
        out.write_all(&self.layout_hash)?;
        put_u64(out, self.node_count() as u64)?;
        for id in 0..self.node_count() {
            let node = self.entry(id).expect("bounded index entry");
            put_u64(out, node.raw)?;
            out.write_all(&node.hash)?;
        }
        Ok(INDEX_HEADER_BYTES + self.node_count() as u64 * INDEX_RECORD_BYTES)
    }
    /// Parent-relative cache sidecar. Requires the parent cache and corresponding
    /// delta; unlike `write_to`, this writes only new fingerprints.
    pub fn write_incremental_to(&self, out: &mut impl Write) -> Result<u64> {
        let parent = self.parent.as_ref().context("index has no parent")?;
        out.write_all(INCREMENTAL_MAGIC)?;
        out.write_all(&parent.layout_hash)?;
        put_u64(out, parent.node_count() as u64)?;
        put_u64(out, self.root)?;
        out.write_all(&self.root_hash)?;
        out.write_all(&self.layout_hash)?;
        put_u64(out, self.nodes.len() as u64)?;
        for node in self.nodes.iter() {
            out.write_all(&node.hash)?;
        }
        Ok(INCREMENTAL_HEADER_BYTES + self.nodes.len() as u64 * DIGEST_BYTES as u64)
    }
}
fn trim_atom(bytes: &[u8]) -> &[u8] {
    let len = bytes
        .iter()
        .rposition(|&byte| byte != 0)
        .map_or(0, |i| i + 1);
    &bytes[..len]
}
fn hash_atom(bytes: &[u8]) -> Digest {
    let bytes = trim_atom(bytes);
    let mut hasher = blake3::Hasher::new();
    hasher.update(ATOM_DOMAIN);
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
    digest(hasher.finalize())
}
fn hash_direct(value: u64) -> Digest {
    hash_atom(&value.to_le_bytes())
}
fn hash_cell(head: Digest, tail: Digest) -> Digest {
    #[cfg(feature = "one-shot-cell")]
    {
        let mut bytes = [0; CELL_DOMAIN.len() + 2 * DIGEST_BYTES];
        bytes[..CELL_DOMAIN.len()].copy_from_slice(CELL_DOMAIN);
        bytes[CELL_DOMAIN.len()..CELL_DOMAIN.len() + DIGEST_BYTES].copy_from_slice(&head);
        bytes[CELL_DOMAIN.len() + DIGEST_BYTES..].copy_from_slice(&tail);
        digest(blake3::hash(&bytes))
    }
    #[cfg(not(feature = "one-shot-cell"))]
    {
        let mut hasher = blake3::Hasher::new();
        hasher.update(CELL_DOMAIN);
        hasher.update(&head);
        hasher.update(&tail);
        digest(hasher.finalize())
    }
}
fn estimated_index_bytes(index: &MerkleIndex) -> u64 {
    // HashMap bucket/control allocation is implementation-dependent: this is an
    // estimate, not RSS. The harness separately samples/measures process memory.
    (index.nodes.capacity() * std::mem::size_of::<IndexedNode>()
        + index.ids.capacity() * 20
        + std::mem::size_of::<MerkleIndex>()) as u64
}

pub fn build_index<G: Graph + ?Sized>(graph: &G, options: BuildOptions) -> Result<MerkleIndex> {
    let started = Instant::now();
    let mut index = MerkleIndex {
        root: graph.root(),
        root_hash: [0; DIGEST_BYTES],
        layout_hash: [0; DIGEST_BYTES],
        nodes: Arc::new(Vec::new()),
        ids: Arc::new(IntMap::default()),
        parent: None,
        direct_hashes: std::array::from_fn(|i| hash_direct(i as u64)),
        stats: IndexStats::default(),
    };
    let mut layout = blake3::Hasher::new();
    layout.update(b"nockchain-noun-layout-v1\0");
    layout.update(&graph.root().to_le_bytes());
    let mut stack = vec![(graph.root(), false)];
    while let Some((raw, finish)) = stack.pop() {
        if raw < DIRECT_LIMIT {
            // Validate generic implementations too, instead of trusting a tagged
            // handle alone to manufacture a direct atom.
            ensure!(
                matches!(graph.node(raw)?, Node::Direct(value) if value == raw),
                "invalid direct handle"
            );
            index.stats.direct_references += 1;
            continue;
        }
        if !finish {
            if let Some(&id) = index.ids.get(&raw) {
                ensure!(id != PENDING, "cycle in noun graph at {raw:#x}");
                index.stats.memo_hits += 1;
                continue;
            }
            if let Some(limit) = options.max_nodes {
                ensure!(index.ids.len() < limit,
                    "node limit {limit} reached: visited={} completed={} elapsed_secs={:.3} estimated_index_bytes={} stack_frames={}",
                    index.ids.len(), index.nodes.len(), started.elapsed().as_secs_f64(), estimated_index_bytes(&index), stack.len());
            }
            ensure!(
                index.ids.len() < PENDING as usize,
                "index exceeds u32 node capacity"
            );
            Arc::get_mut(&mut index.ids)
                .expect("exclusive index build")
                .insert(raw, PENDING);
            if options.progress_every != 0 && index.ids.len() % options.progress_every == 0 {
                eprintln!("index_progress visited={} completed={} atom_bytes={} elapsed_secs={:.3} estimated_index_bytes={} stack_frames={}",
                    index.ids.len(), index.nodes.len(), index.stats.atom_bytes, started.elapsed().as_secs_f64(), estimated_index_bytes(&index), stack.len());
            }
            match graph.node(raw)? {
                Node::Cell { head, tail } => {
                    stack.push((raw, true));
                    stack.push((tail, false));
                    stack.push((head, false));
                    index.stats.max_stack_frames = index.stats.max_stack_frames.max(stack.len());
                    continue;
                }
                Node::Atom(_) => {}
                Node::Direct(_) => bail!("indirect handle resolves to a direct node"),
            }
        }
        let hash = match graph.node(raw)? {
            Node::Cell { head, tail } => {
                index.stats.cells += 1;
                hash_cell(index.hash_of(head)?, index.hash_of(tail)?)
            }
            Node::Atom(bytes) => {
                index.stats.indirect_atoms += 1;
                index.stats.atom_bytes += trim_atom(bytes).len() as u64;
                hash_atom(bytes)
            }
            Node::Direct(_) => bail!("indirect handle resolves to a direct node"),
        };
        let id = index.nodes.len() as u32;
        Arc::get_mut(&mut index.nodes)
            .expect("exclusive index build")
            .push(IndexedNode { raw, hash });
        *Arc::get_mut(&mut index.ids)
            .expect("exclusive index build")
            .get_mut(&raw)
            .expect("visited node") = id;
        layout.update(&raw.to_le_bytes());
        layout.update(&hash);
    }
    index.root_hash = index.hash_of(index.root)?;
    index.layout_hash = digest(layout.finalize());
    index.stats.fingerprint_bits = DIGEST_BYTES * 8;
    index.stats.nodes = index.nodes.len();
    index.stats.elapsed_secs = started.elapsed().as_secs_f64();
    index.stats.estimated_index_bytes = estimated_index_bytes(&index);
    index.stats.serialized_index_bytes =
        INDEX_HEADER_BYTES + index.nodes.len() as u64 * INDEX_RECORD_BYTES;
    index.stats.incremental_index_bytes = index.stats.estimated_index_bytes;
    index.stats.reachable_nodes_exact = true;
    Ok(index)
}

/// Index a reconstructed view without visiting the base graph or copying its
/// tables. Inherited entries stay available even when the new root no longer
/// reaches them. Their stable IDs permit cheap lookup-table extension and reuse
/// of older content in later deltas. Stats therefore describe retained backing,
/// not the precise reachable graph. Dropping ancestors requires a full rebuild.
pub fn derive_index(base: &MerkleIndex, delta: &Delta) -> Result<MerkleIndex> {
    let started = Instant::now();
    let hashes = validated_delta_hashes(base, delta)?;
    let total = base
        .node_count()
        .checked_add(hashes.len())
        .context("derived node count overflow")?;
    ensure!(
        total < PENDING as usize,
        "derived index exceeds u32 node capacity"
    );
    let mut layout = blake3::Hasher::new();
    layout.update(b"nockchain-noun-derived-layout-v1\0");
    layout.update(&base.layout_hash);
    layout.update(&(base.node_count() as u64).to_le_bytes());
    layout.update(&delta.root.0.to_le_bytes());
    layout.update(&delta.target_root_hash);
    layout.update(&(hashes.len() as u64).to_le_bytes());
    let nodes: Vec<_> = hashes
        .into_iter()
        .enumerate()
        .map(|(id, hash)| {
            layout.update(&hash);
            IndexedNode {
                raw: NounRef::local(id).0,
                hash,
            }
        })
        .collect();
    let mut index = MerkleIndex {
        root: delta.root.0,
        root_hash: delta.target_root_hash,
        layout_hash: digest(layout.finalize()),
        nodes: Arc::new(nodes),
        ids: Arc::new(IntMap::default()),
        parent: Some(Arc::new(base.clone())),
        direct_hashes: base.direct_hashes,
        stats: IndexStats::default(),
    };
    let own_bytes = estimated_index_bytes(&index);
    index.stats = IndexStats {
        fingerprint_bits: DIGEST_BYTES * 8,
        nodes: total,
        cells: base.stats.cells
            + delta
                .nodes
                .iter()
                .filter(|node| matches!(node, DeltaNode::Cell { .. }))
                .count(),
        indirect_atoms: base.stats.indirect_atoms
            + delta
                .nodes
                .iter()
                .filter(|node| matches!(node, DeltaNode::Atom(_)))
                .count(),
        atom_bytes: base.stats.atom_bytes
            + delta
                .nodes
                .iter()
                .map(|node| match node {
                    DeltaNode::Atom(bytes) => bytes.len() as u64,
                    _ => 0,
                })
                .sum::<u64>(),
        elapsed_secs: started.elapsed().as_secs_f64(),
        estimated_index_bytes: base.stats.estimated_index_bytes + own_bytes,
        serialized_index_bytes: INDEX_HEADER_BYTES + total as u64 * INDEX_RECORD_BYTES,
        inherited_nodes: base.node_count(),
        incremental_index_bytes: own_bytes,
        index_depth: base.stats.index_depth + 1,
        reachable_nodes_exact: false,
        incremental_sidecar_bytes: Some(
            INCREMENTAL_HEADER_BYTES + index.nodes.len() as u64 * DIGEST_BYTES as u64,
        ),
        ..IndexStats::default()
    };
    Ok(index)
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct LookupStats {
    pub unique_hashes: usize,
    pub prefix_collisions: usize,
    /// Total shared backing retained by this lookup, not independently allocated.
    pub estimated_bytes: u64,
    pub incremental_bytes: u64,
    pub elapsed_secs: f64,
}
#[derive(Clone)]
pub struct ContentLookup {
    first: Arc<IntMap<u32>>,
    collisions: Arc<IntMap<Vec<u32>>>,
    parent: Option<Arc<ContentLookup>>,
    base_layout: Digest,
    stats: LookupStats,
}
fn prefix(hash: &Digest) -> u64 {
    u64::from_le_bytes(hash[..8].try_into().unwrap())
}
impl ContentLookup {
    pub fn new(base: &MerkleIndex) -> Self {
        let started = Instant::now();
        let mut lookup = Self {
            first: Arc::new(IntMap::default()),
            collisions: Arc::new(IntMap::default()),
            parent: None,
            base_layout: base.layout_hash,
            stats: LookupStats::default(),
        };
        for id in 0..base.node_count() {
            lookup.insert(base, id as u32);
            if id != 0 && id % 10_000_000 == 0 {
                eprintln!(
                    "lookup_progress nodes={id} unique_hashes={} elapsed_secs={:.3}",
                    lookup.stats.unique_hashes,
                    started.elapsed().as_secs_f64()
                );
            }
        }
        lookup.finish_stats(started);
        lookup
    }
    /// Extend the matching parent's table without scanning or copying its nodes.
    pub fn derive(parent: &ContentLookup, derived: &MerkleIndex) -> Result<Self> {
        let started = Instant::now();
        let base = derived
            .parent
            .as_ref()
            .context("derived index has no parent")?;
        ensure!(
            parent.base_layout == base.layout_hash,
            "lookup/derived parent identity mismatch"
        );
        let mut lookup = Self {
            first: Arc::new(IntMap::default()),
            collisions: Arc::new(IntMap::default()),
            parent: Some(Arc::new(parent.clone())),
            base_layout: derived.layout_hash,
            stats: LookupStats {
                unique_hashes: parent.stats.unique_hashes,
                prefix_collisions: parent.stats.prefix_collisions,
                ..LookupStats::default()
            },
        };
        for id in base.node_count()..derived.node_count() {
            lookup.insert(derived, id as u32);
        }
        lookup.finish_stats(started);
        Ok(lookup)
    }
    fn insert(&mut self, base: &MerkleIndex, id: u32) {
        let hash = base.entry(id as usize).expect("bounded index entry").hash;
        if self.find(base, &hash).is_some() {
            return;
        }
        let key = prefix(&hash);
        if self.first.contains_key(&key) {
            Arc::get_mut(&mut self.collisions)
                .expect("exclusive lookup construction")
                .entry(key)
                .or_default()
                .push(id);
            self.stats.prefix_collisions += 1;
        } else {
            Arc::get_mut(&mut self.first)
                .expect("exclusive lookup construction")
                .insert(key, id);
        }
        self.stats.unique_hashes += 1;
    }
    fn finish_stats(&mut self, started: Instant) {
        self.stats.incremental_bytes = (self.first.capacity() * 20
            + self.collisions.capacity() * 40
            + self
                .collisions
                .values()
                .map(|ids| ids.capacity() * 4)
                .sum::<usize>()
            + std::mem::size_of::<Self>()) as u64;
        self.stats.estimated_bytes = self.stats.incremental_bytes
            + self.parent.as_ref().map_or(0, |p| p.stats.estimated_bytes);
        self.stats.elapsed_secs = started.elapsed().as_secs_f64();
    }
    pub fn stats(&self) -> &LookupStats {
        &self.stats
    }
    fn find(&self, base: &MerkleIndex, hash: &Digest) -> Option<u32> {
        let key = prefix(hash);
        if let Some(&first) = self.first.get(&key) {
            if base.entry(first as usize)?.hash == *hash {
                return Some(first);
            }
            if let Some(alternatives) = self.collisions.get(&key) {
                if let Some(id) = alternatives
                    .iter()
                    .copied()
                    .find(|&id| base.entry(id as usize).map(|node| node.hash) == Some(*hash))
                {
                    return Some(id);
                }
            }
        }
        self.parent
            .as_ref()
            .and_then(|parent| parent.find(base, hash))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NounRef(pub u64);
impl NounRef {
    fn direct(value: u64) -> Self {
        Self(value)
    }
    fn local(id: usize) -> Self {
        Self(LOCAL_TAG | id as u64)
    }
    fn base(id: u32) -> Self {
        Self(BASE_TAG | id as u64)
    }
    fn index(self) -> usize {
        (self.0 & REF_MASK) as usize
    }
    fn is_direct(self) -> bool {
        self.0 < DIRECT_LIMIT
    }
    fn is_base(self) -> bool {
        self.0 & BASE_TAG == BASE_TAG
    }
}
#[derive(Clone, Debug)]
pub enum DeltaNode {
    Atom(Vec<u8>),
    Cell { head: NounRef, tail: NounRef },
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct DeltaStats {
    pub new_cells: usize,
    pub new_atoms: usize,
    pub atom_bytes: u64,
    pub base_references: usize,
    pub memo_hits: usize,
    pub encoded_bytes: u64,
    pub elapsed_secs: f64,
    pub estimated_delta_bytes: u64,
}
#[derive(Clone, Debug)]
pub struct Delta {
    pub base_root_hash: Digest,
    pub base_layout_hash: Digest,
    pub target_root_hash: Digest,
    pub root: NounRef,
    pub nodes: Vec<DeltaNode>,
    stats: DeltaStats,
}
impl Delta {
    pub fn stats(&self) -> &DeltaStats {
        &self.stats
    }
    pub fn write_to(&self, out: &mut impl Write) -> Result<u64> {
        out.write_all(DELTA_MAGIC)?;
        out.write_all(&self.base_root_hash)?;
        out.write_all(&self.base_layout_hash)?;
        out.write_all(&self.target_root_hash)?;
        put_u64(out, self.root.0)?;
        put_u64(out, self.nodes.len() as u64)?;
        let mut written = DELTA_HEADER_BYTES;
        for node in &self.nodes {
            match node {
                DeltaNode::Atom(bytes) => {
                    out.write_all(&[0])?;
                    put_u64(out, bytes.len() as u64)?;
                    out.write_all(bytes)?;
                    written += 9 + bytes.len() as u64;
                }
                DeltaNode::Cell { head, tail } => {
                    out.write_all(&[1])?;
                    put_u64(out, head.0)?;
                    put_u64(out, tail.0)?;
                    written += 17;
                }
            }
        }
        Ok(written)
    }
    pub fn read_from(input: &mut impl Read) -> Result<Self> {
        let mut magic = [0; 8];
        input.read_exact(&mut magic)?;
        ensure!(&magic == DELTA_MAGIC, "invalid delta magic/version");
        let mut base_root_hash = [0; DIGEST_BYTES];
        input.read_exact(&mut base_root_hash)?;
        let mut base_layout_hash = [0; DIGEST_BYTES];
        input.read_exact(&mut base_layout_hash)?;
        let mut target_root_hash = [0; DIGEST_BYTES];
        input.read_exact(&mut target_root_hash)?;
        let root = NounRef(get_u64(input)?);
        let count = get_u64(input)?;
        ensure!(count < PENDING as u64, "delta exceeds node capacity");
        let mut nodes = Vec::new();
        let mut stats = DeltaStats {
            encoded_bytes: DELTA_HEADER_BYTES,
            ..DeltaStats::default()
        };
        for id in 0..count as usize {
            let mut tag = [0];
            input.read_exact(&mut tag)?;
            let node = match tag[0] {
                0 => {
                    let len = usize::try_from(get_u64(input)?)?;
                    let mut bytes = Vec::new();
                    bytes
                        .try_reserve_exact(len)
                        .context("delta atom allocation")?;
                    bytes.resize(len, 0);
                    input.read_exact(&mut bytes)?;
                    ensure!(
                        trim_atom(&bytes).len() == bytes.len(),
                        "noncanonical delta atom"
                    );
                    stats.new_atoms += 1;
                    stats.atom_bytes += len as u64;
                    stats.encoded_bytes += 9 + len as u64;
                    DeltaNode::Atom(bytes)
                }
                1 => {
                    let head = NounRef(get_u64(input)?);
                    let tail = NounRef(get_u64(input)?);
                    validate_local_ref(head, id)?;
                    validate_local_ref(tail, id)?;
                    stats.new_cells += 1;
                    stats.encoded_bytes += 17;
                    DeltaNode::Cell { head, tail }
                }
                _ => bail!("unknown delta node tag {}", tag[0]),
            };
            nodes.push(node);
        }
        validate_local_ref(root, nodes.len())?;
        let mut trailing = [0];
        ensure!(
            input.read(&mut trailing)? == 0,
            "trailing data after delta records"
        );
        stats.estimated_delta_bytes =
            (nodes.capacity() * std::mem::size_of::<DeltaNode>()) as u64 + stats.atom_bytes;
        Ok(Self {
            base_root_hash,
            base_layout_hash,
            target_root_hash,
            root,
            nodes,
            stats,
        })
    }
}
fn validate_local_ref(reference: NounRef, before: usize) -> Result<()> {
    if !reference.is_direct() && !reference.is_base() {
        ensure!(
            reference.index() < before,
            "missing, forward, or cyclic local delta reference"
        );
    }
    Ok(())
}
fn resolved_ref(raw: u64, refs: &IntMap<NounRef>) -> Result<NounRef> {
    if raw < DIRECT_LIMIT {
        Ok(NounRef::direct(raw))
    } else {
        refs.get(&raw).copied().context("unfinished delta child")
    }
}

pub fn build_delta<G: Graph + ?Sized>(
    target: &G,
    target_index: &MerkleIndex,
    base_index: &MerkleIndex,
    lookup: &ContentLookup,
) -> Result<Delta> {
    ensure!(
        target.root() == target_index.root,
        "target/index root mismatch"
    );
    ensure!(
        lookup.base_layout == base_index.layout_hash,
        "lookup/base index mismatch"
    );
    let started = Instant::now();
    let mut delta = Delta {
        base_root_hash: base_index.root_hash,
        base_layout_hash: base_index.layout_hash,
        target_root_hash: target_index.root_hash,
        root: NounRef(0),
        nodes: Vec::new(),
        stats: DeltaStats::default(),
    };
    let mut refs = IntMap::default();
    let mut stack = vec![(target.root(), false)];
    while let Some((raw, finish)) = stack.pop() {
        if raw < DIRECT_LIMIT {
            continue;
        }
        if refs.contains_key(&raw) {
            delta.stats.memo_hits += 1;
            continue;
        }
        if !finish {
            let hash = target_index.hash_of(raw)?;
            if let Some(id) = lookup.find(base_index, &hash) {
                refs.insert(raw, NounRef::base(id));
                delta.stats.base_references += 1;
                continue;
            }
            if let Node::Cell { head, tail } = target.node(raw)? {
                stack.push((raw, true));
                stack.push((tail, false));
                stack.push((head, false));
                continue;
            }
        }
        let node = match target.node(raw)? {
            Node::Cell { head, tail } => {
                delta.stats.new_cells += 1;
                DeltaNode::Cell {
                    head: resolved_ref(head, &refs)?,
                    tail: resolved_ref(tail, &refs)?,
                }
            }
            Node::Atom(bytes) => {
                let bytes = trim_atom(bytes);
                if bytes.len() <= 8 {
                    let mut word = [0; 8];
                    word[..bytes.len()].copy_from_slice(bytes);
                    let value = u64::from_le_bytes(word);
                    if value < DIRECT_LIMIT {
                        refs.insert(raw, NounRef::direct(value));
                        continue;
                    }
                }
                delta.stats.new_atoms += 1;
                delta.stats.atom_bytes += bytes.len() as u64;
                DeltaNode::Atom(bytes.to_vec())
            }
            Node::Direct(_) => bail!("invalid target indirect handle"),
        };
        ensure!(
            delta.nodes.len() < PENDING as usize,
            "delta exceeds node capacity"
        );
        refs.insert(raw, NounRef::local(delta.nodes.len()));
        delta.nodes.push(node);
    }
    delta.root = resolved_ref(target.root(), &refs)?;
    delta.stats.encoded_bytes = DELTA_HEADER_BYTES
        + delta.stats.new_cells as u64 * 17
        + delta.stats.new_atoms as u64 * 9
        + delta.stats.atom_bytes;
    delta.stats.estimated_delta_bytes = delta.nodes.capacity() as u64
        * std::mem::size_of::<DeltaNode>() as u64
        + delta.stats.atom_bytes;
    delta.stats.elapsed_secs = started.elapsed().as_secs_f64();
    Ok(delta)
}

fn validated_delta_hashes(base_index: &MerkleIndex, delta: &Delta) -> Result<Vec<Digest>> {
    ensure!(
        delta.base_root_hash == base_index.root_hash
            && delta.base_layout_hash == base_index.layout_hash,
        "delta base identity mismatch"
    );
    let validate = |reference: NounRef, before: usize| -> Result<()> {
        validate_local_ref(reference, before)?;
        if reference.is_base() {
            ensure!(
                reference.index() < base_index.node_count(),
                "missing base node reference"
            );
        }
        Ok(())
    };
    let fingerprint = |reference: NounRef, hashes: &[Digest]| -> Result<Digest> {
        if reference.is_direct() {
            Ok(base_index.direct_hash(reference.0))
        } else if reference.is_base() {
            Ok(base_index
                .entry(reference.index())
                .context("base fingerprint out of bounds")?
                .hash)
        } else {
            hashes
                .get(reference.index())
                .copied()
                .context("missing local fingerprint")
        }
    };
    // Validate the new envelope using only cached base fingerprints and
    // new record bytes. Full base-backed value verification is a separate
    // pass; this phase is O(delta nodes + new atom bytes), not O(base size).
    let mut hashes = Vec::new();
    hashes
        .try_reserve_exact(delta.nodes.len())
        .context("delta fingerprint allocation")?;
    for (id, node) in delta.nodes.iter().enumerate() {
        let hash = match node {
            DeltaNode::Cell { head, tail } => {
                validate(*head, id)?;
                validate(*tail, id)?;
                hash_cell(fingerprint(*head, &hashes)?, fingerprint(*tail, &hashes)?)
            }
            DeltaNode::Atom(bytes) => {
                ensure!(
                    trim_atom(bytes).len() == bytes.len(),
                    "noncanonical delta atom"
                );
                hash_atom(bytes)
            }
        };
        hashes.push(hash);
    }
    validate(delta.root, delta.nodes.len())?;
    ensure!(
        fingerprint(delta.root, &hashes)? == delta.target_root_hash,
        "delta target fingerprint mismatch"
    );
    Ok(hashes)
}

pub struct DeltaView<'a, G: Graph + ?Sized> {
    base: &'a G,
    index: &'a MerkleIndex,
    delta: &'a Delta,
}
impl<'a, G: Graph + ?Sized> DeltaView<'a, G> {
    pub fn new(base: &'a G, base_index: &'a MerkleIndex, delta: &'a Delta) -> Result<Self> {
        ensure!(
            base.root() == base_index.root,
            "base graph/index root mismatch"
        );
        validated_delta_hashes(base_index, delta)?;
        Ok(Self {
            base,
            index: base_index,
            delta,
        })
    }
    fn base_ref(&self, raw: u64) -> Result<u64> {
        if raw < DIRECT_LIMIT {
            return Ok(raw);
        }
        let id = self.index.id_of(raw)?;
        Ok(NounRef::base(id).0)
    }
}
impl<G: Graph + ?Sized> Graph for DeltaView<'_, G> {
    fn root(&self) -> u64 {
        self.delta.root.0
    }
    fn node(&self, raw: u64) -> Result<Node<'_>> {
        let reference = NounRef(raw);
        if reference.is_direct() {
            return Ok(Node::Direct(raw));
        }
        if reference.is_base() {
            let source = self
                .index
                .entry(reference.index())
                .context("base node out of bounds")?
                .raw;
            return match self.base.node(source)? {
                Node::Cell { head, tail } => Ok(Node::Cell {
                    head: self.base_ref(head)?,
                    tail: self.base_ref(tail)?,
                }),
                node => Ok(node),
            };
        }
        match self
            .delta
            .nodes
            .get(reference.index())
            .context("local node out of bounds")?
        {
            DeltaNode::Atom(bytes) => Ok(Node::Atom(bytes)),
            DeltaNode::Cell { head, tail } => Ok(Node::Cell {
                head: head.0,
                tail: tail.0,
            }),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct VerifyStats {
    pub compared_pairs: usize,
    pub compared_atom_bytes: u64,
    pub elapsed_secs: f64,
}
/// Full value comparison independent of the Merkle hashes; pair memoization
/// avoids checking the same node pair repeatedly when physical sharing differs.
pub fn verify_equal<A: Graph + ?Sized, B: Graph + ?Sized>(
    left: &A,
    right: &B,
) -> Result<VerifyStats> {
    let started = Instant::now();
    let mut seen = PairSet::default();
    let mut stack = vec![(left.root(), right.root())];
    let mut stats = VerifyStats::default();
    while let Some((a, b)) = stack.pop() {
        if a < DIRECT_LIMIT && b < DIRECT_LIMIT {
            ensure!(a == b, "direct atom mismatch {a} != {b}");
            continue;
        }
        if !seen.insert((a, b)) {
            continue;
        }
        stats.compared_pairs += 1;
        match (left.node(a)?, right.node(b)?) {
            (Node::Cell { head: ah, tail: at }, Node::Cell { head: bh, tail: bt }) => {
                stack.push((at, bt));
                stack.push((ah, bh));
            }
            (Node::Atom(a), Node::Atom(b)) => {
                ensure!(trim_atom(a) == trim_atom(b), "indirect atom mismatch");
                stats.compared_atom_bytes += trim_atom(a).len() as u64;
            }
            (Node::Direct(a), Node::Direct(b)) => ensure!(a == b, "direct atom mismatch"),
            (Node::Direct(value), Node::Atom(bytes)) | (Node::Atom(bytes), Node::Direct(value)) => {
                ensure!(
                    trim_atom(&value.to_le_bytes()) == trim_atom(bytes),
                    "direct/indirect atom mismatch"
                );
                stats.compared_atom_bytes += trim_atom(bytes).len() as u64;
            }
            _ => bail!("cell/atom structure mismatch"),
        }
        if stats.compared_pairs % 10_000_000 == 0 {
            eprintln!(
                "verify_progress pairs={} atom_bytes={} elapsed_secs={:.3}",
                stats.compared_pairs,
                stats.compared_atom_bytes,
                started.elapsed().as_secs_f64()
            );
        }
    }
    stats.elapsed_secs = started.elapsed().as_secs_f64();
    Ok(stats)
}
fn put_u64(out: &mut impl Write, value: u64) -> Result<()> {
    out.write_all(&value.to_le_bytes())?;
    Ok(())
}
fn get_u64(input: &mut impl Read) -> Result<u64> {
    let mut bytes = [0; 8];
    input.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}
