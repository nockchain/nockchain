//! Content-addressed Nockasm artifacts for compiled kernel inspection.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::error::Error;
use std::fmt::Write as FmtWrite;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use nockasm::{
    cue, jam, lift_bundle, lift_dag, lift_noun_dag, DagId, DagInput, DagMode, DagNode, DagOp,
    NasmBundle, Noun, NounRef, NASM_BUNDLE_VERSION,
};
use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const ARTIFACT_VERSION: u32 = 1;
const GRAPH_FILE: &str = "graph.ndag";
const MANIFEST_FILE: &str = "manifest.json";
const ROOT_NAME: &str = "kernel";
const SHARD_COUNT: usize = 256;
const MAX_FRAGMENT_NODES: usize = 10_000;

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    artifact_version: u32,
    nockasm_bundle_version: u32,
    debug_hash_bits: u16,
    root_name: String,
    root_blake3: String,
    canonical_jam_blake3: String,
    canonical_jam_bytes: u64,
    source_jam_blake3: String,
    source_jam_bytes: u64,
    graph_blake3: String,
    graph_bytes: u64,
    unique_nodes: u64,
    atom_nodes: u64,
    cell_nodes: u64,
}

struct Graph {
    noun: Noun,
    nodes: Vec<DagNode>,
    root: DagId,
    hashes: Vec<blake3::Hash>,
}

impl Graph {
    fn from_noun(noun: Noun) -> Result<Self> {
        let dag = lift_noun_dag(&noun)?;
        let nodes = dag.nodes().to_vec();
        let root = dag.root();
        let hashes = hash_nodes(&nodes);
        Ok(Self {
            noun,
            nodes,
            root,
            hashes,
        })
    }

    fn from_bundle(bundle: NasmBundle) -> Result<Self> {
        let root = bundle
            .roots()
            .iter()
            .find(|root| root.name() == ROOT_NAME)
            .ok_or_else(|| format!("bundle has no {ROOT_NAME:?} root"))?
            .id();
        let noun = bundle
            .lower_root(ROOT_NAME)
            .ok_or_else(|| format!("bundle has no {ROOT_NAME:?} root"))?;
        let nodes = bundle.nodes().to_vec();
        let hashes = hash_nodes(&nodes);
        Ok(Self {
            noun,
            nodes,
            root,
            hashes,
        })
    }
}

/// Handle `honk nockasm ...`. Returns a human-readable success message.
pub fn run_cli(args: &[String]) -> Result<String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err(nockasm_usage().into());
    };
    match command {
        "export" => {
            let input = positional(args, 1, "input jam")?;
            let output = option_path(args, "--output")?;
            let formula_root = args.iter().any(|arg| arg == "--formula-root");
            export(Path::new(input), &output, formula_root)?;
            Ok(format!("exported {} to {}", input, output.display()))
        }
        "verify" => {
            let directory = positional(args, 1, "artifact directory")?;
            let report = verify(Path::new(directory))?;
            Ok(report)
        }
        "diff" => {
            let left = positional(args, 1, "left jam or artifact")?;
            let right = positional(args, 2, "right jam or artifact")?;
            let output = option_path(args, "--output")?;
            let limit = option_value(args, "--limit")
                .map(|raw| raw.parse::<usize>())
                .transpose()?
                .unwrap_or(64);
            diff(Path::new(left), Path::new(right), &output, limit)?;
            Ok(format!("wrote structural diff to {}", output.display()))
        }
        "--help" | "-h" | "help" => Ok(nockasm_usage()),
        other => Err(format!("unknown nockasm command {other:?}\n{}", nockasm_usage()).into()),
    }
}

fn nockasm_usage() -> String {
    "Usage: honk nockasm export <kernel.jam> --output <directory> [--formula-root]\n\
     Usage: honk nockasm verify <directory>\n\
     Usage: honk nockasm diff <left.jam|directory> <right.jam|directory> --output <directory> [--limit <n>]"
        .to_string()
}

fn positional<'a>(args: &'a [String], index: usize, label: &str) -> Result<&'a str> {
    args.get(index)
        .filter(|arg| !arg.starts_with('-'))
        .map(String::as_str)
        .ok_or_else(|| format!("missing {label}\n{}", nockasm_usage()).into())
}

fn option_value<'a>(args: &'a [String], option: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == option)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

fn option_path(args: &[String], option: &str) -> Result<PathBuf> {
    option_value(args, option)
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing {option}\n{}", nockasm_usage()).into())
}

/// Export any valid JAM noun as a compact Nockasm graph plus stable,
/// content-addressed text shards suitable for ordinary directory diffs.
pub fn export(input: &Path, output: &Path, formula_root: bool) -> Result<()> {
    let source = fs::read(input)?;
    let noun = cue(&source).map_err(|error| format!("cue {}: {error}", input.display()))?;
    let bundle = lift_bundle(&[DagInput {
        name: ROOT_NAME,
        noun: &noun,
        mode: DagMode::Noun,
    }])?;
    let graph_bytes = bundle.to_bytes();
    let hashes = hash_nodes(bundle.nodes());
    let root = bundle.roots()[0].id();
    let canonical_jam = jam(&noun);
    let atom_nodes = bundle
        .nodes()
        .iter()
        .filter(|node| matches!(node, DagNode::Atom(_)))
        .count();
    let cell_nodes = bundle
        .nodes()
        .iter()
        .filter(|node| matches!(node, DagNode::Cell(_, _)))
        .count();
    let manifest = Manifest {
        artifact_version: ARTIFACT_VERSION,
        nockasm_bundle_version: NASM_BUNDLE_VERSION,
        debug_hash_bits: 128,
        root_name: ROOT_NAME.to_string(),
        root_blake3: hashes[root.index()].to_hex().to_string(),
        canonical_jam_blake3: blake3::hash(&canonical_jam).to_hex().to_string(),
        canonical_jam_bytes: canonical_jam.len() as u64,
        source_jam_blake3: blake3::hash(&source).to_hex().to_string(),
        source_jam_bytes: source.len() as u64,
        graph_blake3: blake3::hash(&graph_bytes).to_hex().to_string(),
        graph_bytes: graph_bytes.len() as u64,
        unique_nodes: bundle.nodes().len() as u64,
        atom_nodes: atom_nodes as u64,
        cell_nodes: cell_nodes as u64,
    };

    let pending = PendingDirectory::new(output)?;
    fs::create_dir_all(pending.path().join("tree"))?;
    fs::write(pending.path().join(GRAPH_FILE), graph_bytes)?;
    fs::write(
        pending.path().join(MANIFEST_FILE),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    fs::write(
        pending.path().join("tree/root.ref"),
        format!("{}\t{}\n", ROOT_NAME, manifest.root_blake3),
    )?;
    write_hash_shards(pending.path(), bundle.nodes(), &hashes)?;
    if formula_root {
        fs::write(
            pending.path().join("formula-root.nasm-dag"),
            lift_dag(&noun)?.render(),
        )?;
    }
    pending.commit()
}

/// Verify the compact graph and all hashes recorded by an artifact manifest.
pub fn verify(directory: &Path) -> Result<String> {
    let manifest: Manifest = serde_json::from_slice(&fs::read(directory.join(MANIFEST_FILE))?)?;
    if manifest.artifact_version != ARTIFACT_VERSION {
        return Err(format!("unsupported artifact version {}", manifest.artifact_version).into());
    }
    if manifest.nockasm_bundle_version != NASM_BUNDLE_VERSION || manifest.debug_hash_bits != 128 {
        return Err("artifact encoding parameters do not match this Honk build".into());
    }
    let bytes = fs::read(directory.join(GRAPH_FILE))?;
    check_hex("graph", blake3::hash(&bytes), &manifest.graph_blake3)?;
    if bytes.len() as u64 != manifest.graph_bytes {
        return Err("graph byte count does not match manifest".into());
    }
    let bundle = NasmBundle::from_bytes(&bytes)?;
    if bundle.to_bytes() != bytes {
        return Err("graph encoding is not canonical".into());
    }
    let root = bundle
        .roots()
        .iter()
        .find(|root| root.name() == manifest.root_name)
        .ok_or_else(|| "manifest root is missing from graph".to_string())?;
    if bundle.nodes().len() as u64 != manifest.unique_nodes {
        return Err("node count does not match manifest".into());
    }
    let atom_nodes = bundle
        .nodes()
        .iter()
        .filter(|node| matches!(node, DagNode::Atom(_)))
        .count() as u64;
    let cell_nodes = bundle
        .nodes()
        .iter()
        .filter(|node| matches!(node, DagNode::Cell(_, _)))
        .count() as u64;
    if atom_nodes != manifest.atom_nodes || cell_nodes != manifest.cell_nodes {
        return Err("node-kind counts do not match manifest".into());
    }
    let hashes = hash_nodes(bundle.nodes());
    check_hex("root", hashes[root.id().index()], &manifest.root_blake3)?;
    let noun = bundle
        .lower_root(root.name())
        .ok_or_else(|| "manifest root is missing from graph".to_string())?;
    let canonical_jam = jam(&noun);
    check_hex(
        "canonical jam",
        blake3::hash(&canonical_jam),
        &manifest.canonical_jam_blake3,
    )?;
    if canonical_jam.len() as u64 != manifest.canonical_jam_bytes {
        return Err("canonical jam byte count does not match manifest".into());
    }
    verify_hash_shards(directory, bundle.nodes(), &hashes)?;
    Ok(format!(
        "verified {}: {} unique nodes, {} graph bytes",
        directory.display(),
        manifest.unique_nodes,
        manifest.graph_bytes
    ))
}

fn check_hex(label: &str, actual: blake3::Hash, expected: &str) -> Result<()> {
    if actual.to_hex().as_str() != expected {
        return Err(format!("{label} hash does not match manifest").into());
    }
    Ok(())
}

/// Produce a sharing-aware structural diff. Equal subgraphs are skipped by
/// content hash, so a small compiler change does not expand the whole kernel.
pub fn diff(left: &Path, right: &Path, output: &Path, limit: usize) -> Result<()> {
    let left = load_graph(left)?;
    let right = load_graph(right)?;
    let mut stack = vec![(left.root, right.root, AxisPath::root())];
    let mut changes = Vec::new();
    let mut equal_subgraphs = 0u64;
    while let Some((left_id, right_id, axis)) = stack.pop() {
        if left.hashes[left_id.index()] == right.hashes[right_id.index()] {
            equal_subgraphs += 1;
            continue;
        }
        match (&left.nodes[left_id.index()], &right.nodes[right_id.index()]) {
            (DagNode::Cell(lh, lt), DagNode::Cell(rh, rt)) => {
                stack.push((*lt, *rt, axis.child(true)));
                stack.push((*lh, *rh, axis.child(false)));
            }
            _ => {
                if changes.len() < limit {
                    changes.push(Change {
                        axis,
                        left: left_id,
                        right: right_id,
                    });
                }
                if changes.len() >= limit {
                    break;
                }
            }
        }
    }

    let pending = PendingDirectory::new(output)?;
    fs::create_dir_all(pending.path().join("changes"))?;
    let mut summary = String::new();
    writeln!(summary, "left-root\t{}", left.hashes[left.root.index()])?;
    writeln!(summary, "right-root\t{}", right.hashes[right.root.index()])?;
    writeln!(summary, "equal-subgraphs-skipped\t{equal_subgraphs}")?;
    writeln!(summary, "reported-changes\t{}", changes.len())?;
    writeln!(summary, "change-limit\t{limit}")?;
    for (index, change) in changes.iter().enumerate() {
        writeln!(
            summary,
            "change-{index:04}\taxis={}\tleft={}\tright={}",
            change.axis,
            left.hashes[change.left.index()],
            right.hashes[change.right.index()]
        )?;
        write_fragment(
            &left,
            change.left,
            &change.axis,
            &pending.path().join(format!("changes/{index:04}-left")),
        )?;
        write_fragment(
            &right,
            change.right,
            &change.axis,
            &pending.path().join(format!("changes/{index:04}-right")),
        )?;
    }
    fs::write(pending.path().join("summary.tsv"), summary)?;
    pending.commit()
}

fn load_graph(path: &Path) -> Result<Graph> {
    if path.is_dir() {
        let bundle = NasmBundle::from_bytes(&fs::read(path.join(GRAPH_FILE))?)?;
        Graph::from_bundle(bundle)
    } else {
        let noun =
            cue(&fs::read(path)?).map_err(|error| format!("cue {}: {error}", path.display()))?;
        Graph::from_noun(noun)
    }
}

struct Change {
    axis: AxisPath,
    left: DagId,
    right: DagId,
}

#[derive(Clone)]
struct AxisPath(Vec<bool>);

impl AxisPath {
    fn root() -> Self {
        Self(Vec::new())
    }

    fn child(&self, tail: bool) -> Self {
        let mut path = self.0.clone();
        path.push(tail);
        Self(path)
    }
}

impl std::fmt::Display for AxisPath {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("0b1")?;
        for bit in &self.0 {
            out.write_str(if *bit { "1" } else { "0" })?;
        }
        Ok(())
    }
}

fn noun_at_axis<'a>(root: &'a Noun, axis: &AxisPath) -> Option<&'a Noun> {
    let mut noun = root;
    for tail in &axis.0 {
        let NounRef::Cell(head, rest) = noun.view() else {
            return None;
        };
        noun = if *tail { rest } else { head };
    }
    Some(noun)
}

fn write_fragment(graph: &Graph, id: DagId, axis: &AxisPath, path: &Path) -> Result<()> {
    let hash = graph.hashes[id.index()];
    let nodes = subtree_size_bounded(graph, id, MAX_FRAGMENT_NODES + 1);
    let noun = noun_at_axis(&graph.noun, axis)
        .ok_or_else(|| format!("axis {axis} is invalid in lowered graph"))?;
    if nodes <= MAX_FRAGMENT_NODES {
        fs::write(path.with_extension("nasm-dag"), lift_dag(noun)?.render())?;
    } else {
        fs::write(
            path.with_extension("ref"),
            format!("axis\t{axis}\nhash\t{hash}\nsubtree-nodes\t>{MAX_FRAGMENT_NODES}\n"),
        )?;
    }
    Ok(())
}

fn subtree_size_bounded(graph: &Graph, root: DagId, limit: usize) -> usize {
    let mut seen = HashSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if seen.len() >= limit {
            break;
        }
        push_children(&graph.nodes[id.index()], &mut stack);
    }
    seen.len()
}

fn push_children(node: &DagNode, output: &mut Vec<DagId>) {
    match node {
        DagNode::Atom(_) | DagNode::Op(DagOp::Slot(_)) => {}
        DagNode::Cell(a, b)
        | DagNode::Op(DagOp::Eval(a, b))
        | DagNode::Op(DagOp::Eq(a, b))
        | DagNode::Op(DagOp::Comp(a, b))
        | DagNode::Op(DagOp::Push(a, b))
        | DagNode::Op(DagOp::Hint(a, b))
        | DagNode::Op(DagOp::Scry(a, b)) => output.extend([*a, *b]),
        DagNode::Op(DagOp::Isa(a))
        | DagNode::Op(DagOp::Inc(a))
        | DagNode::Op(DagOp::Const(a))
        | DagNode::Nock(a) => output.push(*a),
        DagNode::Op(DagOp::If(a, b, c)) | DagNode::Op(DagOp::Hintd(a, b, c)) => {
            output.extend([*a, *b, *c])
        }
        DagNode::Op(DagOp::Call(_, a)) => output.push(*a),
        DagNode::Op(DagOp::Edit(_, a, b)) => output.extend([*a, *b]),
    }
}

fn hash_nodes(nodes: &[DagNode]) -> Vec<blake3::Hash> {
    let mut hashes = Vec::with_capacity(nodes.len());
    for node in nodes {
        let mut hash = blake3::Hasher::new();
        hash.update(b"honk-nockasm-node-v1\0");
        match node {
            DagNode::Atom(atom) => {
                hash.update(b"atom\0");
                let bytes = atom.to_le_bytes();
                hash.update(&(bytes.len() as u64).to_le_bytes());
                hash.update(&bytes);
            }
            DagNode::Cell(head, tail) => {
                hash.update(b"cell\0");
                update_child_hash(&mut hash, &hashes, *head);
                update_child_hash(&mut hash, &hashes, *tail);
            }
            DagNode::Nock(raw) => {
                hash.update(b"nock\0");
                update_child_hash(&mut hash, &hashes, *raw);
            }
            DagNode::Op(op) => hash_op(&mut hash, &hashes, op),
        }
        hashes.push(hash.finalize());
    }
    hashes
}

fn update_child_hash(hash: &mut blake3::Hasher, hashes: &[blake3::Hash], child: DagId) {
    hash.update(hashes[child.index()].as_bytes());
}

fn update_atom(hash: &mut blake3::Hasher, atom: &nockasm::Atom) {
    let bytes = atom.to_le_bytes();
    hash.update(&(bytes.len() as u64).to_le_bytes());
    hash.update(&bytes);
}

fn hash_op(hash: &mut blake3::Hasher, hashes: &[blake3::Hash], op: &DagOp) {
    hash.update(b"op\0");
    match op {
        DagOp::Slot(axis) => {
            hash.update(&[0]);
            update_atom(hash, axis);
        }
        DagOp::Const(a) => {
            hash.update(&[1]);
            update_child_hash(hash, hashes, *a);
        }
        DagOp::Eval(a, b) => {
            hash.update(&[2]);
            update_child_hash(hash, hashes, *a);
            update_child_hash(hash, hashes, *b);
        }
        DagOp::Isa(a) => {
            hash.update(&[3]);
            update_child_hash(hash, hashes, *a);
        }
        DagOp::Inc(a) => {
            hash.update(&[4]);
            update_child_hash(hash, hashes, *a);
        }
        DagOp::Eq(a, b) => {
            hash.update(&[5]);
            update_child_hash(hash, hashes, *a);
            update_child_hash(hash, hashes, *b);
        }
        DagOp::If(a, b, c) => {
            hash.update(&[6]);
            update_child_hash(hash, hashes, *a);
            update_child_hash(hash, hashes, *b);
            update_child_hash(hash, hashes, *c);
        }
        DagOp::Comp(a, b) => {
            hash.update(&[7]);
            update_child_hash(hash, hashes, *a);
            update_child_hash(hash, hashes, *b);
        }
        DagOp::Push(a, b) => {
            hash.update(&[8]);
            update_child_hash(hash, hashes, *a);
            update_child_hash(hash, hashes, *b);
        }
        DagOp::Call(axis, a) => {
            hash.update(&[9]);
            update_atom(hash, axis);
            update_child_hash(hash, hashes, *a);
        }
        DagOp::Edit(axis, a, b) => {
            hash.update(&[10]);
            update_atom(hash, axis);
            update_child_hash(hash, hashes, *a);
            update_child_hash(hash, hashes, *b);
        }
        DagOp::Hint(a, b) => {
            hash.update(&[11, 0]);
            update_child_hash(hash, hashes, *a);
            update_child_hash(hash, hashes, *b);
        }
        DagOp::Hintd(a, b, c) => {
            hash.update(&[11, 1]);
            update_child_hash(hash, hashes, *a);
            update_child_hash(hash, hashes, *b);
            update_child_hash(hash, hashes, *c);
        }
        DagOp::Scry(a, b) => {
            hash.update(&[12]);
            update_child_hash(hash, hashes, *a);
            update_child_hash(hash, hashes, *b);
        }
    }
}

fn write_hash_shards(directory: &Path, nodes: &[DagNode], hashes: &[blake3::Hash]) -> Result<()> {
    let mut shards: Vec<Vec<u32>> = (0..SHARD_COUNT).map(|_| Vec::new()).collect();
    for (index, hash) in hashes.iter().enumerate() {
        shards[hash.as_bytes()[0] as usize].push(index as u32);
    }
    for (shard, ids) in shards.iter_mut().enumerate() {
        ids.sort_unstable_by(|left, right| compare_hashes(hashes, *left, *right));
        let path = directory.join(format!("tree/{shard:02x}.nodes"));
        let mut output = BufWriter::new(File::create(path)?);
        for id in ids {
            let id = DagId::from_index(*id as usize).ok_or("node ID exceeds u32")?;
            write_node_line(&mut output, nodes, hashes, id)?;
        }
    }
    Ok(())
}

fn verify_hash_shards(directory: &Path, nodes: &[DagNode], hashes: &[blake3::Hash]) -> Result<()> {
    let temporary = directory.join(format!(".verify-shards-{}", std::process::id()));
    if temporary.exists() {
        return Err(format!(
            "temporary verification directory exists: {}",
            temporary.display()
        )
        .into());
    }
    fs::create_dir(&temporary)?;
    fs::create_dir(temporary.join("tree"))?;
    let result = (|| -> Result<()> {
        write_hash_shards(&temporary, nodes, hashes)?;
        for shard in 0..SHARD_COUNT {
            let name = format!("{shard:02x}.nodes");
            if fs::read(directory.join("tree").join(&name))?
                != fs::read(temporary.join("tree").join(&name))?
            {
                return Err(format!("content-addressed shard {name} does not match graph").into());
            }
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&temporary);
    result
}

fn compare_hashes(hashes: &[blake3::Hash], left: u32, right: u32) -> Ordering {
    hashes[left as usize]
        .as_bytes()
        .cmp(hashes[right as usize].as_bytes())
        .then_with(|| left.cmp(&right))
}

fn write_node_line(
    output: &mut impl Write,
    nodes: &[DagNode],
    hashes: &[blake3::Hash],
    id: DagId,
) -> Result<()> {
    write!(output, "{}\t", short_hash(hashes[id.index()]))?;
    match &nodes[id.index()] {
        DagNode::Atom(atom) => {
            write!(output, "atom\t")?;
            write_hex(output, &atom.to_le_bytes())?;
        }
        DagNode::Cell(head, tail) => {
            write!(
                output,
                "cell\t{}\t{}",
                short_hash(hashes[head.index()]),
                short_hash(hashes[tail.index()])
            )?;
        }
        DagNode::Nock(raw) => write!(output, "nock\t{}", short_hash(hashes[raw.index()]))?,
        DagNode::Op(op) => write_op_line(output, hashes, op)?,
    }
    writeln!(output)?;
    Ok(())
}

fn write_op_line(output: &mut impl Write, hashes: &[blake3::Hash], op: &DagOp) -> Result<()> {
    let h = |id: DagId| short_hash(hashes[id.index()]);
    match op {
        DagOp::Slot(axis) => write!(output, "slot\t{}", axis)?,
        DagOp::Const(a) => write!(output, "const\t{}", h(*a))?,
        DagOp::Eval(a, b) => write!(output, "eval\t{}\t{}", h(*a), h(*b))?,
        DagOp::Isa(a) => write!(output, "isa\t{}", h(*a))?,
        DagOp::Inc(a) => write!(output, "inc\t{}", h(*a))?,
        DagOp::Eq(a, b) => write!(output, "eq\t{}\t{}", h(*a), h(*b))?,
        DagOp::If(a, b, c) => write!(output, "if\t{}\t{}\t{}", h(*a), h(*b), h(*c))?,
        DagOp::Comp(a, b) => write!(output, "comp\t{}\t{}", h(*a), h(*b))?,
        DagOp::Push(a, b) => write!(output, "push\t{}\t{}", h(*a), h(*b))?,
        DagOp::Call(axis, a) => write!(output, "call\t{}\t{}", axis, h(*a))?,
        DagOp::Edit(axis, a, b) => write!(output, "edit\t{}\t{}\t{}", axis, h(*a), h(*b))?,
        DagOp::Hint(a, b) => write!(output, "hint\t{}\t{}", h(*a), h(*b))?,
        DagOp::Hintd(a, b, c) => write!(output, "hintd\t{}\t{}\t{}", h(*a), h(*b), h(*c))?,
        DagOp::Scry(a, b) => write!(output, "scry\t{}\t{}", h(*a), h(*b))?,
    }
    Ok(())
}

fn short_hash(hash: blake3::Hash) -> String {
    let mut output = String::with_capacity(32);
    for byte in &hash.as_bytes()[..16] {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn write_hex(output: &mut impl Write, bytes: &[u8]) -> Result<()> {
    for byte in bytes {
        write!(output, "{byte:02x}")?;
    }
    Ok(())
}

struct PendingDirectory {
    temporary: PathBuf,
    final_path: PathBuf,
    committed: bool,
}

impl PendingDirectory {
    fn new(final_path: &Path) -> Result<Self> {
        if final_path.exists() {
            return Err(format!("refusing to replace existing {}", final_path.display()).into());
        }
        let parent = final_path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let file_name = final_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("nockasm-artifact");
        let temporary = parent.join(format!(
            ".{file_name}.tmp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        fs::create_dir(&temporary)?;
        Ok(Self {
            temporary,
            final_path: final_path.to_path_buf(),
            committed: false,
        })
    }

    fn path(&self) -> &Path {
        &self.temporary
    }

    fn commit(mut self) -> Result<()> {
        fs::rename(&self.temporary, &self.final_path)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for PendingDirectory {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_dir_all(&self.temporary);
        }
    }
}

#[cfg(test)]
mod tests {
    use nockasm::noun;

    use super::*;

    #[test]
    fn artifact_round_trip_and_structural_diff() {
        let temp = tempfile::tempdir().unwrap();
        let left_jam = temp.path().join("left.jam");
        let right_jam = temp.path().join("right.jam");
        fs::write(&left_jam, jam(&noun![[1 2] [1 2]])).unwrap();
        fs::write(&right_jam, jam(&noun![[1 3] [1 2]])).unwrap();
        let artifact = temp.path().join("left.nockasm");
        export(&left_jam, &artifact, false).unwrap();
        verify(&artifact).unwrap();
        let diff_dir = temp.path().join("diff");
        diff(&artifact, &right_jam, &diff_dir, 8).unwrap();
        let summary = fs::read_to_string(diff_dir.join("summary.tsv")).unwrap();
        assert!(summary.contains("reported-changes\t1"));
    }

    #[test]
    fn rejects_corrupt_graph() {
        let temp = tempfile::tempdir().unwrap();
        let jam_path = temp.path().join("kernel.jam");
        fs::write(&jam_path, jam(&noun![[1 2] [1 2]])).unwrap();
        let artifact = temp.path().join("kernel.nockasm");
        export(&jam_path, &artifact, false).unwrap();
        let mut bytes = fs::read(artifact.join(GRAPH_FILE)).unwrap();
        bytes[0] ^= 1;
        fs::write(artifact.join(GRAPH_FILE), bytes).unwrap();
        assert!(verify(&artifact).is_err());
    }
}
