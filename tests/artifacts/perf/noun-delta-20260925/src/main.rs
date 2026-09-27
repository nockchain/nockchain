use anyhow::{bail, Context, Result};
use nockchain_noun_delta_bench::{
    engine::{self, BuildOptions, ContentLookup, Delta, DeltaView, Graph},
    pma::Snapshot,
};
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::{BufReader, BufWriter, Cursor, Write},
    path::Path,
    time::Instant,
};

fn emit(stage: &str, elapsed: std::time::Duration, details: Value) {
    println!(
        "{}",
        json!({"stage":stage,"seconds":elapsed.as_secs_f64(),"details":details})
    );
}
fn memory() -> Value {
    let s = fs::read_to_string("/proc/self/status").unwrap_or_default();
    json!(s
        .lines()
        .filter(|l| l.starts_with("VmRSS:") || l.starts_with("VmHWM:") || l.starts_with("VmSize:"))
        .collect::<Vec<_>>())
}
fn open(pma: &str, descriptor: &str) -> Result<Snapshot> {
    if descriptor.ends_with(".meta") {
        Snapshot::open_operative(Path::new(pma), Path::new(descriptor))
    } else {
        Snapshot::open(Path::new(pma), Path::new(descriptor))
    }
}
fn info(s: &Snapshot) -> Value {
    json!({"root_raw":s.root(),"event_num":s.manifest.event_num,"allocated_words":s.manifest.alloc_words,"pma_words":s.manifest.pma_words,"kernel_hash":s.manifest.ker_hash})
}
fn main() -> Result<()> {
    let a = std::env::args().skip(1).collect::<Vec<_>>();
    if a.is_empty() {
        bail!("usage: inspect PMA MANIFEST_OR_META | pilot PMA MANIFEST_OR_META MAX_NODES | bench BASE_PMA BASE_DESC TARGET_PMA TARGET_DESC OUTPUT_DIR [REPEATS]");
    }
    let total = Instant::now();
    match a[0].as_str() {
        "inspect" => {
            if a.len() != 3 {
                bail!("inspect requires PMA and descriptor")
            }
            let s = open(&a[1], &a[2])?;
            emit("inspect", total.elapsed(), info(&s));
        }
        "pilot" => {
            if a.len() != 4 {
                bail!("pilot requires PMA, descriptor, and MAX_NODES")
            }
            let s = open(&a[1], &a[2])?;
            emit("open", total.elapsed(), info(&s));
            let limit = a[3].parse()?;
            let t = Instant::now();
            match engine::build_index(
                &s,
                BuildOptions {
                    max_nodes: Some(limit),
                    progress_every: 100_000,
                },
            ) {
                Ok(i) => emit(
                    "pilot_complete",
                    t.elapsed(),
                    json!({"stats":i.stats(),"root_hash":i.root_hash(),"memory":memory()}),
                ),
                Err(e) => {
                    emit(
                        "pilot_stopped",
                        t.elapsed(),
                        json!({"error":e.to_string(),"node_limit":limit,"memory":memory()}),
                    );
                    return Err(e);
                }
            }
        }
        "bench" => {
            if !(6..=7).contains(&a.len()) {
                bail!("bench requires two PMA/descriptor pairs, output dir, optional repetitions")
            }
            let repeats: usize = a.get(6).map(|s| s.parse()).transpose()?.unwrap_or(20);
            let out = Path::new(&a[5]);
            fs::create_dir_all(out)?;
            let base = open(&a[1], &a[2])?;
            let target = open(&a[3], &a[4])?;
            emit(
                "open",
                total.elapsed(),
                json!({"base":info(&base),"target":info(&target)}),
            );
            let t = Instant::now();
            let base_index = engine::build_index(
                &base,
                BuildOptions {
                    max_nodes: None,
                    progress_every: 5_000_000,
                },
            )?;
            emit(
                "index_base",
                t.elapsed(),
                json!({"stats":base_index.stats(),"root_hash":base_index.root_hash(),"memory":memory()}),
            );
            let t = Instant::now();
            let target_index = engine::build_index(
                &target,
                BuildOptions {
                    max_nodes: None,
                    progress_every: 5_000_000,
                },
            )?;
            emit(
                "index_target",
                t.elapsed(),
                json!({"stats":target_index.stats(),"root_hash":target_index.root_hash(),"memory":memory()}),
            );
            let t = Instant::now();
            let lookup = ContentLookup::new(&base_index);
            emit(
                "base_content_lookup",
                t.elapsed(),
                json!({"stats":lookup.stats(),"memory":memory()}),
            );
            let t = Instant::now();
            let delta = engine::build_delta(&target, &target_index, &base_index, &lookup)?;
            emit(
                "delta_build",
                t.elapsed(),
                json!({"stats":delta.stats(),"memory":memory()}),
            );
            fs::write(
                out.join("envelope.json"),
                serde_json::to_vec_pretty(
                    &json!({"version":1,"base":info(&base),"target":info(&target),"base_state_hash":base_index.root_hash(),"base_layout_hash":base_index.layout_hash(),"target_state_hash":target_index.root_hash(),"delta_stats":delta.stats()}),
                )?,
            )?;
            let delta_path = out.join("state.delta");
            let t = Instant::now();
            let mut file = File::options()
                .write(true)
                .create_new(true)
                .open(&delta_path)
                .context("create independent delta artifact")?;
            {
                let mut w = BufWriter::new(&mut file);
                delta.write_to(&mut w)?;
                w.flush()?;
            }
            file.sync_all()?;
            let delta_bytes = file.metadata()?.len();
            emit(
                "delta_write_fsync",
                t.elapsed(),
                json!({"bytes":delta_bytes}),
            );
            drop(file);
            let t = Instant::now();
            let decoded = Delta::read_from(&mut BufReader::new(File::open(&delta_path)?))?;
            let view = DeltaView::new(&base, &base_index, &decoded)?;
            std::hint::black_box(view.root());
            emit(
                "delta_read_and_virtual_mount",
                t.elapsed(),
                json!({"bytes":delta_bytes,"memory":memory()}),
            );
            let mut encoded = Vec::new();
            let warm = Instant::now();
            let mut done = 0;
            for iteration in 0..repeats {
                let t = Instant::now();
                let d = engine::build_delta(&target, &target_index, &base_index, &lookup)?;
                let build = t.elapsed();
                encoded.clear();
                let t = Instant::now();
                d.write_to(&mut encoded)?;
                let encode = t.elapsed();
                let t = Instant::now();
                let roundtrip = Delta::read_from(&mut Cursor::new(&encoded))?;
                let v = DeltaView::new(&base, &base_index, &roundtrip)?;
                std::hint::black_box(v.root());
                let mount = t.elapsed();
                emit(
                    "warm_iteration",
                    build + encode + mount,
                    json!({"iteration":iteration,"build_seconds":build.as_secs_f64(),"encode_seconds":encode.as_secs_f64(),"decode_virtual_mount_seconds":mount.as_secs_f64(),"delta_bytes":encoded.len()}),
                );
                done += 1;
                if warm.elapsed().as_secs() >= 60 {
                    break;
                }
            }
            emit(
                "warm_summary",
                warm.elapsed(),
                json!({"completed":done,"requested":repeats,"budget_seconds":60}),
            );
            drop(encoded);
            drop(delta);
            drop(lookup);
            drop(target_index);
            let t = Instant::now();
            let verification = engine::verify_equal(&target, &view)?;
            emit(
                "full_structural_verification",
                t.elapsed(),
                json!({"result":verification,"memory":memory()}),
            );
            emit(
                "complete",
                total.elapsed(),
                json!({"delta_bytes":delta_bytes,"memory":memory()}),
            );
        }
        _ => bail!("unknown command"),
    }
    Ok(())
}
