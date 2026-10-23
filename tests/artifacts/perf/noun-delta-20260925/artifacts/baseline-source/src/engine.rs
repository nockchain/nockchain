//! Read-only noun-value Merkle indexes and base-relative DAG envelopes.
//! Hash matches assume BLAKE3-256 collision resistance. `verify_equal` separately
//! compares actual atom values and cell structure, without relying on hashes.
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};
use std::io::{Read, Write};
use std::time::Instant;

use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;

use crate::pma::{Node, Snapshot};

pub type Digest = [u8; 32];
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
}
#[derive(Clone, Copy)]
struct IndexedNode {
    raw: u64,
    hash: Digest,
}

pub struct MerkleIndex {
    root: u64,
    root_hash: Digest,
    layout_hash: Digest,
    nodes: Vec<IndexedNode>,
    ids: IntMap<u32>,
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
        self.nodes.len()
    }
    fn hash_of(&self, raw: u64) -> Result<Digest> {
        if raw < DIRECT_LIMIT {
            return Ok(self.direct_hash(raw));
        }
        let id = *self
            .ids
            .get(&raw)
            .context("noun missing from Merkle index")?;
        ensure!(id != PENDING, "cycle or unfinished child in noun graph");
        Ok(self.nodes[id as usize].hash)
    }
    fn direct_hash(&self, value: u64) -> Digest {
        if value < 256 {
            self.direct_hashes[value as usize]
        } else {
            hash_direct(value)
        }
    }
    /// Fixed-width cache sidecar: magic, root handle, root hash, layout hash,
    /// node count, then (source handle u64 LE, fingerprint 32 bytes) per node.
    /// This cache is optional; its construction and size are benchmark costs.
    pub fn write_to(&self, out: &mut impl Write) -> Result<u64> {
        out.write_all(b"NNIDX001")?;
        put_u64(out, self.root)?;
        out.write_all(&self.root_hash)?;
        out.write_all(&self.layout_hash)?;
        put_u64(out, self.nodes.len() as u64)?;
        for node in &self.nodes {
            put_u64(out, node.raw)?;
            out.write_all(&node.hash)?;
        }
        Ok(88 + self.nodes.len() as u64 * 40)
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
    *hasher.finalize().as_bytes()
}
fn hash_direct(value: u64) -> Digest {
    hash_atom(&value.to_le_bytes())
}
fn hash_cell(head: Digest, tail: Digest) -> Digest {
    let mut hasher = blake3::Hasher::new();
    hasher.update(CELL_DOMAIN);
    hasher.update(&head);
    hasher.update(&tail);
    *hasher.finalize().as_bytes()
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
        root_hash: [0; 32],
        layout_hash: [0; 32],
        nodes: Vec::new(),
        ids: IntMap::default(),
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
            index.ids.insert(raw, PENDING);
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
        index.nodes.push(IndexedNode { raw, hash });
        *index.ids.get_mut(&raw).expect("visited node") = id;
        layout.update(&raw.to_le_bytes());
        layout.update(&hash);
    }
    index.root_hash = index.hash_of(index.root)?;
    index.layout_hash = *layout.finalize().as_bytes();
    index.stats.nodes = index.nodes.len();
    index.stats.elapsed_secs = started.elapsed().as_secs_f64();
    index.stats.estimated_index_bytes = estimated_index_bytes(&index);
    index.stats.serialized_index_bytes = 88 + index.nodes.len() as u64 * 40;
    Ok(index)
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct LookupStats {
    pub unique_hashes: usize,
    pub prefix_collisions: usize,
    pub estimated_bytes: u64,
    pub elapsed_secs: f64,
}
pub struct ContentLookup {
    first: IntMap<u32>,
    collisions: IntMap<Vec<u32>>,
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
            first: IntMap::default(),
            collisions: IntMap::default(),
            base_layout: base.layout_hash,
            stats: LookupStats::default(),
        };
        for (id, node) in base.nodes.iter().enumerate() {
            let key = prefix(&node.hash);
            if let Some(&first) = lookup.first.get(&key) {
                if base.nodes[first as usize].hash != node.hash {
                    let alternatives = lookup.collisions.entry(key).or_default();
                    if !alternatives
                        .iter()
                        .any(|&id| base.nodes[id as usize].hash == node.hash)
                    {
                        alternatives.push(id as u32);
                        lookup.stats.prefix_collisions += 1;
                        lookup.stats.unique_hashes += 1;
                    }
                }
            } else {
                lookup.first.insert(key, id as u32);
                lookup.stats.unique_hashes += 1;
            }
            if id != 0 && id % 10_000_000 == 0 {
                eprintln!(
                    "lookup_progress nodes={id} unique_hashes={} elapsed_secs={:.3}",
                    lookup.stats.unique_hashes,
                    started.elapsed().as_secs_f64()
                );
            }
        }
        lookup.stats.estimated_bytes = (lookup.first.capacity() * 20
            + lookup.collisions.capacity() * 40
            + lookup
                .collisions
                .values()
                .map(|ids| ids.capacity() * 4)
                .sum::<usize>()) as u64;
        lookup.stats.elapsed_secs = started.elapsed().as_secs_f64();
        lookup
    }
    pub fn stats(&self) -> &LookupStats {
        &self.stats
    }
    fn find(&self, base: &MerkleIndex, hash: &Digest) -> Option<u32> {
        let key = prefix(hash);
        let first = *self.first.get(&key)?;
        if base.nodes[first as usize].hash == *hash {
            return Some(first);
        }
        self.collisions
            .get(&key)?
            .iter()
            .copied()
            .find(|&id| base.nodes[id as usize].hash == *hash)
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
        out.write_all(b"NNDLT001")?;
        out.write_all(&self.base_root_hash)?;
        out.write_all(&self.base_layout_hash)?;
        out.write_all(&self.target_root_hash)?;
        put_u64(out, self.root.0)?;
        put_u64(out, self.nodes.len() as u64)?;
        let mut written = 120;
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
        ensure!(&magic == b"NNDLT001", "invalid delta magic/version");
        let mut base_root_hash = [0; 32];
        input.read_exact(&mut base_root_hash)?;
        let mut base_layout_hash = [0; 32];
        input.read_exact(&mut base_layout_hash)?;
        let mut target_root_hash = [0; 32];
        input.read_exact(&mut target_root_hash)?;
        let root = NounRef(get_u64(input)?);
        let count = get_u64(input)?;
        ensure!(count < PENDING as u64, "delta exceeds node capacity");
        let mut nodes = Vec::new();
        let mut stats = DeltaStats {
            encoded_bytes: 120,
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
    delta.stats.encoded_bytes = 120
        + delta.stats.new_cells as u64 * 17
        + delta.stats.new_atoms as u64 * 9
        + delta.stats.atom_bytes;
    delta.stats.estimated_delta_bytes = delta.nodes.capacity() as u64
        * std::mem::size_of::<DeltaNode>() as u64
        + delta.stats.atom_bytes;
    delta.stats.elapsed_secs = started.elapsed().as_secs_f64();
    Ok(delta)
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
        ensure!(
            delta.base_root_hash == base_index.root_hash
                && delta.base_layout_hash == base_index.layout_hash,
            "delta base identity mismatch"
        );
        let validate = |reference: NounRef, before: usize| -> Result<()> {
            validate_local_ref(reference, before)?;
            if reference.is_base() {
                ensure!(
                    reference.index() < base_index.nodes.len(),
                    "missing base node reference"
                );
            }
            Ok(())
        };
        let fingerprint = |reference: NounRef, hashes: &[Digest]| -> Result<Digest> {
            if reference.is_direct() {
                Ok(base_index.direct_hash(reference.0))
            } else if reference.is_base() {
                Ok(base_index.nodes[reference.index()].hash)
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
        let id = *self
            .index
            .ids
            .get(&raw)
            .context("base child absent from index")?;
        ensure!(id != PENDING, "unfinished base index");
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
                .nodes
                .get(reference.index())
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
