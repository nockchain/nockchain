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
fn emit(stage: &str, seconds: f64, details: Value) {
    println!(
        "{}",
        json!({"stage":stage,"seconds":seconds,"details":details})
    );
}
fn memory() -> Value {
    let s = fs::read_to_string("/proc/self/status").unwrap_or_default();
    json!(s
        .lines()
        .filter(|l| l.starts_with("VmRSS:") || l.starts_with("VmHWM:") || l.starts_with("VmSize:"))
        .collect::<Vec<_>>())
}
fn open(p: &str, d: &str) -> Result<Snapshot> {
    if d.ends_with(".meta") {
        Snapshot::open_operative(p, d)
    } else {
        Snapshot::open(p, d)
    }
}
fn info(s: &Snapshot) -> Value {
    json!({"event_num":s.manifest.event_num,"allocated_words":s.manifest.alloc_words,"kernel_hash":s.manifest.ker_hash,"root_raw":s.root()})
}
fn save(delta: &Delta, path: &Path) -> Result<Delta> {
    let t = Instant::now();
    let mut f = File::options().write(true).create_new(true).open(path)?;
    {
        let mut w = BufWriter::new(&mut f);
        delta.write_to(&mut w)?;
        w.flush()?;
    }
    f.sync_all()?;
    emit(
        "write_fsync",
        t.elapsed().as_secs_f64(),
        json!({"path":path,"bytes":f.metadata()?.len()}),
    );
    let t = Instant::now();
    let d = Delta::read_from(&mut BufReader::new(File::open(path)?))?;
    emit(
        "read_delta",
        t.elapsed().as_secs_f64(),
        json!({"path":path}),
    );
    Ok(d)
}
fn main() -> Result<()> {
    let a = std::env::args().skip(1).collect::<Vec<_>>();
    if !(7..=8).contains(&a.len()) {
        bail!("sequence A_PMA A_DESC B_PMA B_DESC C_PMA C_DESC OUTPUT_DIR [REPEATS]")
    }
    let total = Instant::now();
    let out = Path::new(&a[6]);
    fs::create_dir_all(out)?;
    let repeats: usize = a.get(7).map(|x| x.parse()).transpose()?.unwrap_or(20);
    let base = open(&a[0], &a[1])?;
    let middle = open(&a[2], &a[3])?;
    let current = open(&a[4], &a[5])?;
    let metadata = json!({"epoch":info(&base),"middle":info(&middle),"current":info(&current),"parallel_initial_index":true,"hash_bytes":std::mem::size_of::<engine::Digest>()});
    fs::write(
        out.join("inputs.json"),
        serde_json::to_vec_pretty(&metadata)?,
    )?;
    emit("open", total.elapsed().as_secs_f64(), metadata);
    let options = BuildOptions {
        max_nodes: None,
        progress_every: 10_000_000,
    };
    let t = Instant::now();
    let (base_index, middle_index) = std::thread::scope(|scope| -> Result<_> {
        let left = scope.spawn(|| {
            let t = Instant::now();
            let i = engine::build_index(&base, options)?;
            emit(
                "index_epoch",
                t.elapsed().as_secs_f64(),
                json!({"stats":i.stats(),"memory":memory()}),
            );
            Ok::<_, anyhow::Error>(i)
        });
        let t = Instant::now();
        let right = engine::build_index(&middle, options)?;
        emit(
            "index_middle",
            t.elapsed().as_secs_f64(),
            json!({"stats":right.stats(),"memory":memory()}),
        );
        Ok((
            left.join()
                .map_err(|_| anyhow::anyhow!("epoch indexing thread panicked"))??,
            right,
        ))
    })?;
    emit(
        "parallel_initial_index",
        t.elapsed().as_secs_f64(),
        json!({"memory":memory()}),
    );
    let t = Instant::now();
    let lookup_a = ContentLookup::new(&base_index);
    emit(
        "lookup_epoch",
        t.elapsed().as_secs_f64(),
        json!({"stats":lookup_a.stats(),"memory":memory()}),
    );
    let t = Instant::now();
    let ab = engine::build_delta(&middle, &middle_index, &base_index, &lookup_a)?;
    emit(
        "build_epoch_to_middle",
        t.elapsed().as_secs_f64(),
        json!({"stats":ab.stats(),"memory":memory()}),
    );
    let decoded_ab = save(&ab, &out.join("epoch-to-middle.delta"))?;
    drop(ab);
    drop(middle_index);
    drop(middle);
    let t = Instant::now();
    let view_b = DeltaView::new(&base, &base_index, &decoded_ab)?;
    std::hint::black_box(view_b.root());
    emit(
        "mount_middle_from_epoch_delta",
        t.elapsed().as_secs_f64(),
        json!({"memory":memory()}),
    );
    let t = Instant::now();
    let index_b = engine::derive_index(&base_index, &decoded_ab)?;
    emit(
        "derive_middle_index",
        t.elapsed().as_secs_f64(),
        json!({"stats":index_b.stats(),"memory":memory()}),
    );
    let t = Instant::now();
    let lookup_b = ContentLookup::derive(&lookup_a, &index_b)?;
    emit(
        "derive_middle_lookup",
        t.elapsed().as_secs_f64(),
        json!({"stats":lookup_b.stats(),"memory":memory()}),
    );
    drop(lookup_a);
    let t = Instant::now();
    let index_c = engine::build_index(&current, options)?;
    emit(
        "index_current",
        t.elapsed().as_secs_f64(),
        json!({"stats":index_c.stats(),"memory":memory()}),
    );
    let t = Instant::now();
    let bc = engine::build_delta(&current, &index_c, &index_b, &lookup_b)?;
    emit(
        "diff_current_against_reconstructed_middle",
        t.elapsed().as_secs_f64(),
        json!({"stats":bc.stats(),"memory":memory()}),
    );
    let decoded_bc = save(&bc, &out.join("middle-to-current.delta"))?;
    drop(bc);
    let t = Instant::now();
    let view_c = DeltaView::new(&view_b, &index_b, &decoded_bc)?;
    std::hint::black_box(view_c.root());
    emit(
        "mount_current_from_two_deltas",
        t.elapsed().as_secs_f64(),
        json!({"memory":memory()}),
    );
    let mut encoded = Vec::new();
    let warm = Instant::now();
    let mut done = 0;
    for iteration in 0..repeats {
        let t = Instant::now();
        let delta = engine::build_delta(&current, &index_c, &index_b, &lookup_b)?;
        let build = t.elapsed();
        encoded.clear();
        let t = Instant::now();
        delta.write_to(&mut encoded)?;
        let encode = t.elapsed();
        let t = Instant::now();
        let decoded = Delta::read_from(&mut Cursor::new(&encoded))?;
        let view = DeltaView::new(&view_b, &index_b, &decoded)?;
        std::hint::black_box(view.root());
        let mount = t.elapsed();
        emit(
            "warm_chain_iteration",
            (build + encode + mount).as_secs_f64(),
            json!({"iteration":iteration,"build_seconds":build.as_secs_f64(),"encode_seconds":encode.as_secs_f64(),"decode_virtual_mount_seconds":mount.as_secs_f64(),"delta_bytes":encoded.len()}),
        );
        done += 1;
        if warm.elapsed().as_secs() >= 60 {
            break;
        }
    }
    emit(
        "warm_summary",
        warm.elapsed().as_secs_f64(),
        json!({"completed":done,"requested":repeats,"budget_seconds":60}),
    );
    drop(encoded);
    drop(index_c);
    drop(lookup_b);
    let t = Instant::now();
    let verification = engine::verify_equal(&current, &view_c).context(
        "independent exact atom/cell verification of current against epoch plus two deltas",
    )?;
    emit(
        "full_chain_structural_verification",
        t.elapsed().as_secs_f64(),
        json!({"result":verification,"memory":memory()}),
    );
    emit(
        "complete",
        total.elapsed().as_secs_f64(),
        json!({"memory":memory()}),
    );
    Ok(())
}
