//! Experimental exact hash-consing. Physical nouns and mapped files stay read-only.
use anyhow::{bail, ensure, Context, Result};
use nockchain_noun_delta_bench::{engine::Graph, pma::Node};
use serde::Serialize;
use std::{
    collections::HashMap,
    hash::{BuildHasherDefault, Hasher},
    io::{Read, Write},
    time::Instant,
};
const TAG: u64 = 1 << 63;
const PENDING: u64 = u64::MAX;
type Hash = [u8; 32];
const AD: &[u8] = b"nockchain-noun-atom-v1\0";
const CD: &[u8] = b"nockchain-noun-cell-v1\0";
#[derive(Default)]
struct Fast(u64);
impl Hasher for Fast {
    fn finish(&self) -> u64 {
        let mut x = self.0;
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
        x ^ (x >> 31)
    }
    fn write_u64(&mut self, x: u64) {
        self.0 = self
            .0
            .rotate_left(27)
            .wrapping_add(x)
            .wrapping_add(0x9e3779b97f4a7c15)
    }
    fn write(&mut self, b: &[u8]) {
        for c in b.chunks(8) {
            let mut x = [0; 8];
            x[..c.len()].copy_from_slice(c);
            self.write_u64(u64::from_le_bytes(x))
        }
    }
}
type Map<K, V> = HashMap<K, V, BuildHasherDefault<Fast>>;
fn trim(b: &[u8]) -> &[u8] {
    &b[..b.iter().rposition(|x| *x != 0).map_or(0, |i| i + 1)]
}
fn ah(b: &[u8]) -> Hash {
    let b = trim(b);
    let mut h = blake3::Hasher::new();
    h.update(AD);
    h.update(&(b.len() as u64).to_le_bytes());
    h.update(b);
    *h.finalize().as_bytes()
}
fn ch(a: Hash, b: Hash) -> Hash {
    let mut x = [0; CD.len() + 64];
    x[..CD.len()].copy_from_slice(CD);
    x[CD.len()..CD.len() + 32].copy_from_slice(&a);
    x[CD.len() + 32..].copy_from_slice(&b);
    *blake3::hash(&x).as_bytes()
}
fn small(b: &[u8]) -> Option<u64> {
    let b = trim(b);
    if b.len() > 8 {
        return None;
    }
    let mut w = [0; 8];
    w[..b.len()].copy_from_slice(b);
    let v = u64::from_le_bytes(w);
    (v < TAG).then_some(v)
}
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub max_nodes: Option<usize>,
    pub progress_every: usize,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            max_nodes: None,
            progress_every: 10_000_000,
        }
    }
}
#[derive(Default, Debug, Serialize)]
pub struct InternStats {
    pub physical_nodes: usize,
    pub physical_cells: usize,
    pub physical_atoms: usize,
    pub atom_bytes_hashed: u64,
    pub atom_bytes_compared: u64,
    pub canonical_nodes_before: usize,
    pub canonical_nodes_after: usize,
    pub new_cells: usize,
    pub new_atoms: usize,
    pub memo_hits: usize,
    pub raw_memo_peak_estimated_bytes: u64,
    pub raw_memo_entries_retained: usize,
    pub estimated_dictionary_bytes: u64,
    pub elapsed_secs: f64,
}
pub struct Interned {
    pub root: u64,
    pub stats: InternStats,
}
#[derive(Clone, Copy)]
enum Data<'a> {
    Atom(&'a [u8]),
    Cell(u64, u64),
}
#[derive(Clone, Copy)]
struct Record<'a> {
    data: Data<'a>,
    hash: Hash,
}
pub struct Dictionary<'a> {
    nodes: Vec<Record<'a>>,
    cells: Map<(u64, u64), u64>,
    atoms: Map<Hash, u64>,
    collisions: Map<Hash, Vec<u64>>,
    direct: [Hash; 256],
    layout: blake3::Hasher,
    checkpoints: Map<usize, Hash>,
}
impl<'a> Dictionary<'a> {
    pub fn new() -> Self {
        let mut layout = blake3::Hasher::new();
        layout.update(b"exact-hashcons-layout-v1\0");
        Self {
            nodes: vec![],
            cells: Map::default(),
            atoms: Map::default(),
            collisions: Map::default(),
            direct: std::array::from_fn(|i| ah(&(i as u64).to_le_bytes())),
            layout,
            checkpoints: Map::default(),
        }
    }
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
    pub fn root_hash(&self, r: u64) -> Result<Hash> {
        if r < TAG {
            Ok(if r < 256 {
                self.direct[r as usize]
            } else {
                ah(&r.to_le_bytes())
            })
        } else {
            Ok(self
                .nodes
                .get((r & !TAG) as usize)
                .context("canonical node missing")?
                .hash)
        }
    }
    pub fn estimated_bytes(&self) -> u64 {
        (self.nodes.capacity() * std::mem::size_of::<Record>()
            + self.cells.capacity() * 28
            + self.atoms.capacity() * 44
            + self.collisions.capacity() * 64
            + self.checkpoints.capacity() * 48) as u64
    }
    fn add(&mut self, data: Data<'a>, hash: Hash) -> u64 {
        let id = TAG | self.nodes.len() as u64;
        self.nodes.push(Record { data, hash });
        self.layout.update(&id.to_le_bytes());
        match data {
            Data::Atom(b) => {
                self.layout.update(&[0]);
                self.layout.update(&(b.len() as u64).to_le_bytes());
            }
            Data::Cell(a, b) => {
                self.layout.update(&[1]);
                self.layout.update(&a.to_le_bytes());
                self.layout.update(&b.to_le_bytes());
            }
        }
        self.layout.update(&hash);
        id
    }
    fn atom(&mut self, b: &'a [u8], s: &mut InternStats) -> Result<u64> {
        let b = trim(b);
        if let Some(v) = small(b) {
            return Ok(v);
        }
        s.atom_bytes_hashed += b.len() as u64;
        let hash = ah(b);
        if let Some(&id) = self.atoms.get(&hash) {
            if let Data::Atom(old) = self.nodes[(id & !TAG) as usize].data {
                s.atom_bytes_compared += old.len().min(b.len()) as u64;
                if old == b {
                    return Ok(id);
                }
            }
            if let Some(ids) = self.collisions.get(&hash) {
                for &id in ids {
                    if let Data::Atom(old) = self.nodes[(id & !TAG) as usize].data {
                        s.atom_bytes_compared += old.len().min(b.len()) as u64;
                        if old == b {
                            return Ok(id);
                        }
                    }
                }
            }
            let id = self.add(Data::Atom(b), hash);
            self.collisions.entry(hash).or_default().push(id);
            s.new_atoms += 1;
            return Ok(id);
        }
        let id = self.add(Data::Atom(b), hash);
        self.atoms.insert(hash, id);
        s.new_atoms += 1;
        Ok(id)
    }
    pub fn intern<G: Graph + ?Sized>(&mut self, g: &'a G, options: Options) -> Result<Interned> {
        let t = Instant::now();
        let mut s = InternStats {
            canonical_nodes_before: self.nodes.len(),
            ..InternStats::default()
        };
        let mut memo: Map<u64, u64> = Map::default();
        let mut stack = vec![(g.root(), false)];
        while let Some((raw, finish)) = stack.pop() {
            if raw < TAG {
                continue;
            }
            if !finish {
                if let Some(&id) = memo.get(&raw) {
                    ensure!(id != PENDING, "cycle in physical graph");
                    s.memo_hits += 1;
                    continue;
                }
                if let Some(limit) = options.max_nodes {
                    ensure!(memo.len()<limit,"node limit {limit} reached: physical={} canonical={} elapsed_secs={:.3} estimated_dictionary_bytes={} raw_memo_bytes={}",memo.len(),self.nodes.len(),t.elapsed().as_secs_f64(),self.estimated_bytes(),memo.capacity()*20)
                }
                memo.insert(raw, PENDING);
                s.physical_nodes += 1;
                if options.progress_every != 0 && s.physical_nodes % options.progress_every == 0 {
                    eprintln!("hashcons_progress physical={} canonical={} new_cells={} new_atoms={} elapsed_secs={:.3} estimated_dictionary_bytes={} raw_memo_bytes={}",s.physical_nodes,self.nodes.len(),s.new_cells,s.new_atoms,t.elapsed().as_secs_f64(),self.estimated_bytes(),memo.capacity()*20)
                }
                if let Node::Cell { head, tail } = g.node(raw)? {
                    stack.push((raw, true));
                    stack.push((tail, false));
                    stack.push((head, false));
                    continue;
                }
            }
            let id = match g.node(raw)? {
                Node::Direct(v) => v,
                Node::Atom(b) => {
                    s.physical_atoms += 1;
                    self.atom(b, &mut s)?
                }
                Node::Cell { head, tail } => {
                    s.physical_cells += 1;
                    let a = resolve(head, &memo)?;
                    let b = resolve(tail, &memo)?;
                    if let Some(&id) = self.cells.get(&(a, b)) {
                        id
                    } else {
                        let h = ch(self.root_hash(a)?, self.root_hash(b)?);
                        let id = self.add(Data::Cell(a, b), h);
                        self.cells.insert((a, b), id);
                        s.new_cells += 1;
                        id
                    }
                }
            };
            memo.insert(raw, id);
        }
        let root = resolve(g.root(), &memo)?;
        s.raw_memo_peak_estimated_bytes = memo.capacity() as u64 * 20;
        drop(memo);
        s.canonical_nodes_after = self.nodes.len();
        s.estimated_dictionary_bytes = self.estimated_bytes();
        s.elapsed_secs = t.elapsed().as_secs_f64();
        self.checkpoints
            .insert(self.nodes.len(), *self.layout.clone().finalize().as_bytes());
        Ok(Interned { root, stats: s })
    }
    pub fn write_delta(
        &self,
        base_count: usize,
        base_root: u64,
        target_root: u64,
        w: &mut impl Write,
    ) -> Result<u64> {
        ensure!(base_count <= self.nodes.len(), "invalid base count");
        let layout = self
            .checkpoints
            .get(&base_count)
            .context("base not an intern checkpoint")?;
        w.write_all(b"HCDLT001")?;
        put(w, base_count as u64)?;
        put(w, base_root)?;
        w.write_all(&self.root_hash(base_root)?)?;
        w.write_all(layout)?;
        put(w, target_root)?;
        w.write_all(&self.root_hash(target_root)?)?;
        put(w, (self.nodes.len() - base_count) as u64)?;
        let mut bytes = 136;
        for r in &self.nodes[base_count..] {
            match r.data {
                Data::Atom(b) => {
                    w.write_all(&[0])?;
                    put(w, b.len() as u64)?;
                    w.write_all(b)?;
                    bytes += 9 + b.len() as u64
                }
                Data::Cell(a, b) => {
                    w.write_all(&[1])?;
                    put(w, a)?;
                    put(w, b)?;
                    bytes += 17
                }
            }
        }
        Ok(bytes)
    }
    pub fn into_base(mut self, count: usize, root: u64) -> Result<BaseDictionary<'a>> {
        ensure!(count <= self.nodes.len(), "invalid base cut");
        let layout = *self
            .checkpoints
            .get(&count)
            .context("base checkpoint absent")?;
        ensure!(
            root < TAG || (root & !TAG) < count as u64,
            "base root outside prefix"
        );
        let root_hash = self.root_hash(root)?;
        self.nodes.truncate(count);
        Ok(BaseDictionary {
            nodes: self.nodes,
            root,
            root_hash,
            layout,
            direct: self.direct,
        })
    }
}
fn resolve(raw: u64, memo: &Map<u64, u64>) -> Result<u64> {
    if raw < TAG {
        Ok(raw)
    } else {
        let id = *memo.get(&raw).context("unresolved physical child")?;
        ensure!(id != PENDING, "unfinished physical child");
        Ok(id)
    }
}
pub struct BaseDictionary<'a> {
    nodes: Vec<Record<'a>>,
    root: u64,
    root_hash: Hash,
    layout: Hash,
    direct: [Hash; 256],
}
impl BaseDictionary<'_> {
    fn hash(&self, r: u64) -> Result<Hash> {
        if r < TAG {
            Ok(if r < 256 {
                self.direct[r as usize]
            } else {
                ah(&r.to_le_bytes())
            })
        } else {
            Ok(self
                .nodes
                .get((r & !TAG) as usize)
                .context("base node absent")?
                .hash)
        }
    }
}
impl Graph for BaseDictionary<'_> {
    fn root(&self) -> u64 {
        self.root
    }
    fn node(&self, r: u64) -> Result<Node<'_>> {
        if r < TAG {
            return Ok(Node::Direct(r));
        }
        match self
            .nodes
            .get((r & !TAG) as usize)
            .context("base node absent")?
            .data
        {
            Data::Atom(b) => Ok(Node::Atom(b)),
            Data::Cell(head, tail) => Ok(Node::Cell { head, tail }),
        }
    }
}
enum Owned {
    Atom(Vec<u8>),
    Cell(u64, u64),
}
pub struct Delta {
    base_count: usize,
    base_root: u64,
    base_hash: Hash,
    base_layout: Hash,
    target_root: u64,
    target_hash: Hash,
    nodes: Vec<Owned>,
}
impl Delta {
    pub fn read_from(r: &mut impl Read) -> Result<Self> {
        let mut magic = [0; 8];
        r.read_exact(&mut magic)?;
        ensure!(&magic == b"HCDLT001", "bad hashcons delta magic");
        let base_count = usize::try_from(get(r)?)?;
        let base_root = get(r)?;
        let mut base_hash = [0; 32];
        r.read_exact(&mut base_hash)?;
        let mut base_layout = [0; 32];
        r.read_exact(&mut base_layout)?;
        let target_root = get(r)?;
        let mut target_hash = [0; 32];
        r.read_exact(&mut target_hash)?;
        let count = usize::try_from(get(r)?)?;
        let total = base_count
            .checked_add(count)
            .context("node count overflow")?;
        ensure!((total as u64) < TAG - 1, "canonical ID capacity");
        let mut nodes = vec![];
        for id in base_count..total {
            let mut tag = [0];
            r.read_exact(&mut tag)?;
            nodes.push(match tag[0] {
                0 => {
                    let n = usize::try_from(get(r)?)?;
                    let mut b = vec![];
                    b.try_reserve_exact(n)?;
                    b.resize(n, 0);
                    r.read_exact(&mut b)?;
                    ensure!(
                        trim(&b).len() == b.len() && small(&b).is_none(),
                        "noncanonical atom"
                    );
                    Owned::Atom(b)
                }
                1 => {
                    let a = get(r)?;
                    let b = get(r)?;
                    valid(a, id)?;
                    valid(b, id)?;
                    Owned::Cell(a, b)
                }
                _ => bail!("bad node tag"),
            });
        }
        valid(target_root, total)?;
        let mut end = [0];
        ensure!(r.read(&mut end)? == 0, "trailing delta bytes");
        Ok(Self {
            base_count,
            base_root,
            base_hash,
            base_layout,
            target_root,
            target_hash,
            nodes,
        })
    }
}
fn valid(r: u64, before: usize) -> Result<()> {
    ensure!(
        r < TAG || (r & !TAG) < before as u64,
        "missing/forward/cyclic canonical reference"
    );
    Ok(())
}
pub struct View<'a, 'b> {
    base: &'a BaseDictionary<'b>,
    delta: &'a Delta,
}
impl<'a, 'b> View<'a, 'b> {
    pub fn new(base: &'a BaseDictionary<'b>, delta: &'a Delta) -> Result<Self> {
        ensure!(
            base.nodes.len() == delta.base_count
                && base.root == delta.base_root
                && base.root_hash == delta.base_hash
                && base.layout == delta.base_layout,
            "hashcons base identity mismatch"
        );
        let mut hashes = Vec::with_capacity(delta.nodes.len());
        let lookup = |id: u64, hashes: &Vec<Hash>| -> Result<Hash> {
            if id < TAG || (id & !TAG) < delta.base_count as u64 {
                base.hash(id)
            } else {
                Ok(*hashes
                    .get((id & !TAG) as usize - delta.base_count)
                    .context("delta hash child missing")?)
            }
        };
        for r in &delta.nodes {
            hashes.push(match r {
                Owned::Atom(b) => ah(b),
                Owned::Cell(a, b) => ch(lookup(*a, &hashes)?, lookup(*b, &hashes)?),
            });
        }
        ensure!(
            lookup(delta.target_root, &hashes)? == delta.target_hash,
            "hashcons target hash mismatch"
        );
        Ok(Self { base, delta })
    }
}
impl Graph for View<'_, '_> {
    fn root(&self) -> u64 {
        self.delta.target_root
    }
    fn node(&self, id: u64) -> Result<Node<'_>> {
        if id < TAG || (id & !TAG) < self.delta.base_count as u64 {
            return self.base.node(id);
        }
        match self
            .delta
            .nodes
            .get((id & !TAG) as usize - self.delta.base_count)
            .context("new canonical node missing")?
        {
            Owned::Atom(b) => Ok(Node::Atom(b)),
            Owned::Cell(head, tail) => Ok(Node::Cell {
                head: *head,
                tail: *tail,
            }),
        }
    }
}
fn put(w: &mut impl Write, v: u64) -> Result<()> {
    w.write_all(&v.to_le_bytes())?;
    Ok(())
}
fn get(r: &mut impl Read) -> Result<u64> {
    let mut b = [0; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atom_candidate_collision_requires_exact_bytes_and_reuses_the_correct_alternative() {
        let first_bytes = [0x80; 16];
        let second_bytes = [0x81; 16];
        let mut dictionary = Dictionary::new();
        let mut stats = InternStats::default();
        let first = dictionary.atom(&first_bytes, &mut stats).unwrap();
        // Force a mismatching candidate under the second atom's lookup key.
        // This exercises exact collision handling without assuming a real
        // cryptographic collision or weakening the production hash function.
        dictionary.atoms.insert(ah(&second_bytes), first);
        let second = dictionary.atom(&second_bytes, &mut stats).unwrap();
        assert_ne!(first, second);
        assert_eq!(dictionary.atom(&second_bytes, &mut stats).unwrap(), second);
        assert_eq!(dictionary.node_count(), 2);
        assert!(stats.atom_bytes_compared >= 3 * second_bytes.len() as u64);
    }
}
