#[path = "../hashcons.rs"]
mod hashcons;
use anyhow::{bail, Context, Result};
use hashcons::{Delta, Dictionary, Options, View};
use nockchain_noun_delta_bench::{engine, pma::Snapshot};
use serde_json::json;
use std::{
    fs::{self, File},
    io::{BufReader, BufWriter, Write},
    path::Path,
    time::Instant,
};
fn memory() -> Vec<String> {
    fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .filter(|l| l.starts_with("VmRSS:") || l.starts_with("VmHWM:"))
        .map(str::to_owned)
        .collect()
}
fn emit(stage: &str, start: Instant, value: serde_json::Value) {
    println!(
        "{}",
        json!({"stage":stage,"seconds":start.elapsed().as_secs_f64(),"details":value,"memory":memory()})
    );
}
fn open(p: &str, m: &str) -> Result<Snapshot> {
    if m.ends_with(".meta") {
        Snapshot::open_operative(p, m)
    } else {
        Snapshot::open(p, m)
    }
}
fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if !(5..=6).contains(&args.len()) {
        bail!("usage: hashcons BASE_PMA BASE_DESC TARGET_PMA TARGET_DESC OUTPUT_DIR [MAX_NODES]")
    }
    let total = Instant::now();
    let a = open(&args[0], &args[1])?;
    let b = open(&args[2], &args[3])?;
    let options = Options {
        max_nodes: args.get(5).map(|s| s.parse()).transpose()?,
        progress_every: 10_000_000,
    };
    let mut d = Dictionary::new();
    let t = Instant::now();
    let ar = d.intern(&a, options)?;
    emit(
        "intern_base",
        t,
        json!({"stats":ar.stats,"root_hash":d.root_hash(ar.root)?}),
    );
    let count = d.node_count();
    let t = Instant::now();
    let br = d.intern(&b, options)?;
    emit(
        "intern_target_full_physical_traversal",
        t,
        json!({"stats":br.stats,"root_hash":d.root_hash(br.root)?}),
    );
    let out = Path::new(&args[4]);
    fs::create_dir_all(out)?;
    let path = out.join("hashcons.delta");
    let t = Instant::now();
    let mut file = File::options()
        .create_new(true)
        .write(true)
        .open(&path)
        .context("create independent delta")?;
    let bytes;
    {
        let mut w = BufWriter::new(&mut file);
        bytes = d.write_delta(count, ar.root, br.root, &mut w)?;
        w.flush()?
    }
    file.sync_all()?;
    emit(
        "delta_write_fsync",
        t,
        json!({"bytes":bytes,"actual_file_bytes":file.metadata()?.len()}),
    );
    drop(file);
    let t = Instant::now();
    let decoded = Delta::read_from(&mut BufReader::new(File::open(path)?))?;
    let base = d.into_base(count, ar.root)?;
    let view = View::new(&base, &decoded)?;
    emit(
        "delta_read_validated_mount",
        t,
        json!({"new_atoms_owned_by_decoded_delta":true}),
    );
    let t = Instant::now();
    let checked = engine::verify_equal(&b, &view)?;
    emit("full_structural_verification", t, json!({"result":checked}));
    emit(
        "complete",
        total,
        json!({"delta_bytes":bytes,"portable_fingerprint_bits":256,"read_only_sources":true}),
    );
    Ok(())
}
