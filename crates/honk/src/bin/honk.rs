use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use std::{cmp, env, fs, process};

use hatch::ast::hoon::{Hoon, Limb};
use hatch::utils::hoon_to_noun;
use honk::native::formula::comb;
use honk::native::hot::native_hot_state;
use honk::native::noun::term_to_noun;
use honk::native::ut::{ty_noun, Ut};
use honk::pipeline;
use honk::pipeline::{NativeImportKind, ScopeMode};
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::noun::{BrandedEvalExt, BrandedNounSpaceExt, NounAllocatorExt};
use nockapp::utils::{create_context, NOCK_STACK_SIZE_MEDIUM};
use nockapp::AtomExt;
use nockvm::ext::NounExt;
use nockvm::hamt::Hamt;
use nockvm::interpreter::{interpret, Context, Error as NockError};
use nockvm::jets::cold::{Cold, Nounable};
use nockvm::jets::math::util::lth_b;
use nockvm::jets::nock::util::mook;
use nockvm::jets::warm::Warm;
use nockvm::jets::JetDispatchMode;
use nockvm::mem::{AllocationError, NockStack};
use nockvm::mug::{calc_atom_mug_u32, calc_cell_mug_u32, get_mug, set_mug};
use nockvm::noun::{Atom, Cell, Noun, NounAllocator, NounSpace, D, T};
use nockvm::serialization::jam as nock_jam;
use tracing::{debug, info};
use tracing_subscriber::EnvFilter;
use walkdir::WalkDir;

type DynError = Box<dyn Error>;
type Result<T> = std::result::Result<T, DynError>;
type SourcePosition = (u64, u64);
type Spot = (String, SourcePosition, SourcePosition);

static EMBEDDED_HOON_138_SOURCE: &[u8] = include_bytes!(env!("HONK_HOON_138_SOURCE"));
static EMBEDDED_HONC_TYPE_138_JAM: &[u8] = include_bytes!(env!("HONK_HONC_TYPE_138_JAM"));
static EMBEDDED_HONC_FORMULA_138_JAM: &[u8] = include_bytes!(env!("HONK_HONC_FORMULA_138_JAM"));
static EMBEDDED_HONC_COLD_138_JAM: &[u8] = honc_cold_138::HONC_COLD_138_JAM;
// Canonical hoon-138 type noun produced by hoonc.hoon's `data-vase` for
// non-Hoon `/*` leaves.  They use this exact hoonc compiler-core hold, not a
// fresh local `[p=@ud q=@]` alias.
static EMBEDDED_HOONC_OCTS_TYPE_138_JAM: &[u8] =
    include_bytes!(env!("HONK_HOONC_OCTS_TYPE_138_JAM"));
static PARSE_NANOS: AtomicU64 = AtomicU64::new(0);
static COMPILE_SELF_NANOS: AtomicU64 = AtomicU64::new(0);
static COMPILE_WITH_CHILDREN_NANOS: AtomicU64 = AtomicU64::new(0);
static INTERPRET_NANOS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static COMPILE_CHILD_NANOS_STACK: RefCell<Vec<u128>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Debug)]
struct Cli {
    entry: Option<PathBuf>,
    directory: PathBuf,
    output: Option<PathBuf>,
    prelude: PathBuf,
    sut_jam: Option<PathBuf>,
    mode: CompileMode,
    batch_manifest: Option<PathBuf>,
    wrapper_asset_dump: Option<PathBuf>,
    native_wrapper_asset_dump: Option<PathBuf>,
    dbug: bool,
    vet: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompileMode {
    Standard,
    Arbitrary,
    Dynock,
    DynockTyped,
}

impl CompileMode {
    fn from_flags(
        arbitrary: bool,
        dynock: bool,
        dynock_typed: bool,
    ) -> std::result::Result<Self, String> {
        let mode_count = (arbitrary as u8) + (dynock as u8) + (dynock_typed as u8);
        if mode_count > 1 {
            return Err(
                "--arbitrary, --dynock, and --dynock-typed are mutually exclusive".to_string(),
            );
        }
        if arbitrary {
            Ok(Self::Arbitrary)
        } else if dynock {
            Ok(Self::Dynock)
        } else if dynock_typed {
            Ok(Self::DynockTyped)
        } else {
            Ok(Self::Standard)
        }
    }

    fn parse(value: &str) -> std::result::Result<Self, String> {
        match value {
            "standard" => Ok(Self::Standard),
            "arbitrary" => Ok(Self::Arbitrary),
            "dynock" => Ok(Self::Dynock),
            "dynock-typed" => Ok(Self::DynockTyped),
            other => Err(format!("unknown batch compile mode: {other}")),
        }
    }
}

#[derive(Debug)]
struct BatchEntry {
    output: PathBuf,
    entry: PathBuf,
    mode: CompileMode,
    directory_files: Option<Vec<PathBuf>>,
}

fn usage(program: &str) -> String {
    format!(
        "Usage: {program} [--new] [--arbitrary|--dynock|--dynock-typed] --output <file> --prelude <hoon.hoon> [--sut-jam <file>] [--dbug|--no-dbug] [--vet|--no-vet] <entry> <deps_dir>\n\
         Usage: {program} [--new] --batch-manifest <file> --prelude <hoon.hoon> [--sut-jam <file>] <deps_dir>\n\
         Usage: {program} --dump-wrapper-assets <dir> --prelude <hoon.hoon> <deps_dir>\n\
         Usage: {program} --dump-native-wrapper-assets <dir> --prelude <hoon.hoon> <deps_dir>\n\
         --new is accepted for hoonc-script compatibility and has no effect. Vet checking is enabled by default and applies to the whole build (entry and dependency files, matching hoonc); pass --no-vet to disable strict/nice type-checking.\n\
         Example: {program} --new --output assets/native/roswell.jam --prelude hoon/common/hoon.hoon hoon/apps/roswell/roswell.hoon hoon"
    )
}

fn parse_args() -> std::result::Result<Cli, String> {
    let mut args = env::args().skip(1);
    let mut output: Option<PathBuf> = None;
    let mut prelude: Option<PathBuf> = None;
    let mut sut_jam: Option<PathBuf> = None;
    let mut batch_manifest: Option<PathBuf> = None;
    let mut wrapper_asset_dump: Option<PathBuf> = None;
    let mut native_wrapper_asset_dump: Option<PathBuf> = None;
    let mut arbitrary = false;
    let mut dynock = false;
    let mut dynock_typed = false;
    let mut dbug = true;
    let mut vet = true;
    let mut positionals = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--arbitrary" => arbitrary = true,
            "--dynock" => dynock = true,
            "--dynock-typed" => dynock_typed = true,
            "--new" => {}
            "--no-dbug" => dbug = false,
            "--dbug" => dbug = true,
            "--no-vet" => vet = false,
            "--vet" | "--strict" => vet = true,
            "--output" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value after --output".to_string())?;
                output = Some(PathBuf::from(value));
            }
            "--prelude" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value after --prelude".to_string())?;
                prelude = Some(PathBuf::from(value));
            }
            "--sut-jam" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value after --sut-jam".to_string())?;
                sut_jam = Some(PathBuf::from(value));
            }
            "--batch-manifest" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value after --batch-manifest".to_string())?;
                batch_manifest = Some(PathBuf::from(value));
            }
            "--dump-wrapper-assets" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value after --dump-wrapper-assets".to_string())?;
                wrapper_asset_dump = Some(PathBuf::from(value));
            }
            "--dump-native-wrapper-assets" => {
                let value = args.next().ok_or_else(|| {
                    "missing value after --dump-native-wrapper-assets".to_string()
                })?;
                native_wrapper_asset_dump = Some(PathBuf::from(value));
            }
            "--help" | "-h" => return Err(String::new()),
            unknown if unknown.starts_with('-') => {
                return Err(format!("unknown argument: {unknown}"));
            }
            _ => positionals.push(PathBuf::from(arg)),
        }
    }

    let mode = CompileMode::from_flags(arbitrary, dynock, dynock_typed)?;

    if wrapper_asset_dump.is_some() && native_wrapper_asset_dump.is_some() {
        return Err(
            "--dump-wrapper-assets and --dump-native-wrapper-assets are mutually exclusive"
                .to_string(),
        );
    }

    if let Some(wrapper_asset_dump) = wrapper_asset_dump {
        if output.is_some() || batch_manifest.is_some() {
            return Err(
                "--dump-wrapper-assets cannot be combined with --output or --batch-manifest"
                    .to_string(),
            );
        }
        if mode != CompileMode::Standard {
            return Err(
                "--dump-wrapper-assets cannot be combined with compile mode flags".to_string(),
            );
        }
        if positionals.len() != 1 {
            return Err("expected <deps_dir> with --dump-wrapper-assets".to_string());
        }
        return Ok(Cli {
            entry: None,
            directory: positionals.remove(0),
            output: None,
            prelude: prelude.ok_or_else(|| "missing required --prelude".to_string())?,
            sut_jam,
            mode,
            batch_manifest: None,
            wrapper_asset_dump: Some(wrapper_asset_dump),
            native_wrapper_asset_dump: None,
            dbug,
            vet,
        });
    }

    if let Some(native_wrapper_asset_dump) = native_wrapper_asset_dump {
        if output.is_some() || batch_manifest.is_some() {
            return Err(
                "--dump-native-wrapper-assets cannot be combined with --output or --batch-manifest"
                    .to_string(),
            );
        }
        if mode != CompileMode::Standard {
            return Err(
                "--dump-native-wrapper-assets cannot be combined with compile mode flags"
                    .to_string(),
            );
        }
        if positionals.len() != 1 {
            return Err("expected <deps_dir> with --dump-native-wrapper-assets".to_string());
        }
        return Ok(Cli {
            entry: None,
            directory: positionals.remove(0),
            output: None,
            prelude: prelude.ok_or_else(|| "missing required --prelude".to_string())?,
            sut_jam,
            mode,
            batch_manifest: None,
            wrapper_asset_dump: None,
            native_wrapper_asset_dump: Some(native_wrapper_asset_dump),
            dbug,
            vet,
        });
    }

    if batch_manifest.is_some() {
        if output.is_some() {
            return Err("--output is not used with --batch-manifest".to_string());
        }
        if mode != CompileMode::Standard {
            return Err(
                "use per-entry modes in the batch manifest instead of top-level mode flags"
                    .to_string(),
            );
        }
        if positionals.len() != 1 {
            return Err("expected <deps_dir> with --batch-manifest".to_string());
        }
        return Ok(Cli {
            entry: None,
            directory: positionals.remove(0),
            output: None,
            prelude: prelude.ok_or_else(|| "missing required --prelude".to_string())?,
            sut_jam,
            mode,
            batch_manifest,
            wrapper_asset_dump: None,
            native_wrapper_asset_dump: None,
            dbug,
            vet,
        });
    }

    if positionals.len() != 2 {
        return Err("expected <entry> and <deps_dir>".to_string());
    }

    Ok(Cli {
        entry: Some(positionals.remove(0)),
        directory: positionals.remove(0),
        output: Some(output.ok_or_else(|| "missing required --output".to_string())?),
        prelude: prelude.ok_or_else(|| "missing required --prelude".to_string())?,
        sut_jam,
        mode,
        batch_manifest: None,
        wrapper_asset_dump: None,
        native_wrapper_asset_dump: None,
        dbug,
        vet,
    })
}

fn parse_batch_manifest(path: &Path) -> Result<Vec<BatchEntry>> {
    let contents = fs::read_to_string(path)?;
    let mut entries = Vec::new();
    for (idx, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 3 {
            return Err(format!(
                "{}:{}: expected tab-separated output, entry, mode[, directory files...]",
                path.display(),
                idx + 1
            )
            .into());
        }
        let directory_files = if fields.len() > 3 {
            Some(fields[3..].iter().map(PathBuf::from).collect())
        } else {
            None
        };
        entries.push(BatchEntry {
            output: PathBuf::from(fields[0]),
            entry: PathBuf::from(fields[1]),
            mode: CompileMode::parse(fields[2])?,
            directory_files,
        });
    }
    if entries.is_empty() {
        return Err(format!("{}: batch manifest has no entries", path.display()).into());
    }
    Ok(entries)
}

fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,honk=info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .try_init();
}

#[derive(Clone, Copy)]
enum HoonLogOperation {
    Parse,
    Compile,
}

struct TimedHoonPathLog<'a> {
    path: &'a Path,
    operation: HoonLogOperation,
    start: Instant,
}

impl<'a> TimedHoonPathLog<'a> {
    fn new(path: &'a Path, operation: HoonLogOperation) -> Self {
        if matches!(operation, HoonLogOperation::Compile) {
            push_compile_child_timing_frame();
        }
        Self {
            path,
            operation,
            start: Instant::now(),
        }
    }
}

impl Drop for TimedHoonPathLog<'_> {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        let elapsed_nanos = elapsed.as_nanos();
        let elapsed_ms = nanos_to_ms(elapsed_nanos);
        let path = hoon_log_path(self.path);
        match self.operation {
            HoonLogOperation::Parse => {
                add_timing(&PARSE_NANOS, elapsed);
                add_compile_child_timing(elapsed);
                info!(path = %path, elapsed_ms = elapsed_ms, "parsed Hoon source file")
            }
            HoonLogOperation::Compile => {
                let child_nanos = pop_compile_child_timing_frame();
                let self_nanos = elapsed_nanos.saturating_sub(child_nanos);
                add_timing_nanos(&COMPILE_SELF_NANOS, self_nanos);
                add_timing_nanos(&COMPILE_WITH_CHILDREN_NANOS, elapsed_nanos);
                add_compile_child_nanos(elapsed_nanos);
                info!(
                    path = %path,
                    self_ms = nanos_to_ms(self_nanos),
                    with_children_ms = elapsed_ms,
                    "compiled Hoon source file"
                )
            }
        }
    }
}

fn push_compile_child_timing_frame() {
    COMPILE_CHILD_NANOS_STACK.with(|stack| stack.borrow_mut().push(0));
}

fn pop_compile_child_timing_frame() -> u128 {
    COMPILE_CHILD_NANOS_STACK.with(|stack| stack.borrow_mut().pop().unwrap_or(0))
}

fn add_compile_child_timing(elapsed: Duration) {
    add_compile_child_nanos(elapsed.as_nanos());
}

fn add_compile_child_nanos(nanos: u128) {
    COMPILE_CHILD_NANOS_STACK.with(|stack| {
        if let Some(current) = stack.borrow_mut().last_mut() {
            *current = current.saturating_add(nanos);
        }
    });
}

fn add_timing(counter: &AtomicU64, elapsed: Duration) {
    add_timing_nanos(counter, elapsed.as_nanos());
}

fn add_timing_nanos(counter: &AtomicU64, nanos: u128) {
    let nanos = u64::try_from(nanos).unwrap_or(u64::MAX);
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_add(nanos))
    });
}

fn add_interpret_timing(elapsed: Duration) {
    add_timing(&INTERPRET_NANOS, elapsed);
    add_compile_child_timing(elapsed);
}

fn reset_native_timing_totals() {
    PARSE_NANOS.store(0, Ordering::Relaxed);
    COMPILE_SELF_NANOS.store(0, Ordering::Relaxed);
    COMPILE_WITH_CHILDREN_NANOS.store(0, Ordering::Relaxed);
    INTERPRET_NANOS.store(0, Ordering::Relaxed);
    COMPILE_CHILD_NANOS_STACK.with(|stack| stack.borrow_mut().clear());
}

fn timing_ms(counter: &AtomicU64) -> f64 {
    counter.load(Ordering::Relaxed) as f64 / 1_000_000.0
}

fn nanos_to_ms(nanos: u128) -> f64 {
    nanos as f64 / 1_000_000.0
}

fn report_native_timing_totals() {
    info!(
        parsing_ms = timing_ms(&PARSE_NANOS),
        compiling_ms = timing_ms(&COMPILE_SELF_NANOS),
        compiling_with_children_ms = timing_ms(&COMPILE_WITH_CHILDREN_NANOS),
        interpreting_ms = timing_ms(&INTERPRET_NANOS),
        "native compiler timing totals"
    );
}

fn hoon_log_path(path: &Path) -> String {
    let normalized = lexical_absolute_path(path).unwrap_or_else(|_| path.to_path_buf());
    let cwd = env::current_dir()
        .ok()
        .and_then(|cwd| lexical_absolute_path(&cwd).ok());

    if let Some(cwd) = cwd {
        if let Ok(relative) = normalized.strip_prefix(&cwd) {
            if relative.as_os_str().is_empty() {
                return ".".to_string();
            }
            return relative.to_string_lossy().into_owned();
        }
    }

    path.to_string_lossy().into_owned()
}

fn main() {
    init_tracing();
    reset_native_timing_totals();

    let program = env::args().next().unwrap_or_else(|| "honk".to_string());
    let cli = match parse_args() {
        Ok(parsed) => parsed,
        Err(err) if err.is_empty() => {
            println!("{}", usage(&program));
            process::exit(0);
        }
        Err(err) => {
            eprintln!("{err}");
            eprintln!("{}", usage(&program));
            process::exit(2);
        }
    };

    // The deep recursions (musk `^~` constant folds, `live_to_noun`) grow via
    // stacker's segmented stacks, so the worker's base stack barely matters:
    // the hoon-138 native self-mint — the deepest build in the tree —
    // completes byte-identically with as little as a 64MB stack (measured by
    // sweeping HONK_WORKER_STACK_BYTES 32GB → 64MB). 4GB is a ≥64x margin
    // over that floor while staying trivially mappable everywhere — Linux
    // refuses a thread-stack larger than RAM+swap under heuristic overcommit
    // (pthread_create → EAGAIN on a 16GB CI runner at the old 32GB size).
    const WORKER_STACK_BYTES: usize = 4 * 1024 * 1024 * 1024;
    let stack_size: usize = env::var("HONK_WORKER_STACK_BYTES")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(WORKER_STACK_BYTES);
    let worker = std::thread::Builder::new()
        .name("honk".to_string())
        .stack_size(stack_size)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| format!("failed to initialize tokio runtime: {err}"))?;
            runtime.block_on(run(cli)).map_err(|err| err.to_string())
        });

    let result = match worker {
        Ok(worker) => match worker.join() {
            Ok(result) => result,
            Err(_) => Err("native compiler worker thread panicked".to_string()),
        },
        Err(err) => Err(format!("failed to spawn native compiler worker: {err}")),
    };

    report_native_timing_totals();

    if let Err(err) = result {
        eprintln!("native hoon compile failed: {err}");
        process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    let prelude_source = fs::read_to_string(&cli.prelude)?;
    // hoonc compiles DEPENDENCY files (the prelude) with debug OFF: no `%spot`
    // anywhere in the prelude's formula, coil seminouns, or stored arm ASTs (only
    // the ENTRY file and the build wrapper keep spots). The native-parity self-mint
    // takes hoon.hoon as both prelude and entry; parsing the prelude leg debug-off
    // reproduces hoonc's bare-dependency artifact (the entry leg, parsed in
    // `parse_build_leaf` with `self.dbug`, stays spotted). Non-parity builds keep
    // the embedded hoonc prelude TYPE, so this only affects native-parity output.
    //
    // Docs stay ON for the prelude: honk's prelude-with-docs help anchoring
    // partially matches hoonc (the divergence sits at 1781402, a doc honk's
    // anchoring misses), and flipping docs to the entry leg is worse (honk's
    // entry-doc anchoring diverges at byte ~212). The remaining gap is honk's
    // doc-anchoring COMPLETENESS (it captures ~609 help nodes vs hoonc's 1335) —
    // a hatch LineMap parity follow-up, not a prelude/entry flag choice.
    let prelude_dbug = cli.dbug && !native_parity_enabled();
    let prelude_expr = parse_prelude_hoon(&cli.prelude, prelude_dbug, true)?;
    let subject_type_jam = cli.sut_jam.as_ref().map(fs::read).transpose()?;

    if let Some(out_dir) = &cli.wrapper_asset_dump {
        let prelude_eval_subject = parse_prelude_hoon(&cli.prelude, false, true)?;
        let mut builder = build_context_with_dynamic_wrapper_prelude(
            &cli,
            &prelude_expr,
            &prelude_eval_subject,
            &prelude_source,
            subject_type_jam.as_deref(),
        )?;
        return dump_exact_wrapper_assets(&mut builder, out_dir);
    }

    if let Some(out_dir) = &cli.native_wrapper_asset_dump {
        let mut builder = build_context_with_shared_prelude(
            &cli,
            &prelude_expr,
            &prelude_source,
            subject_type_jam.as_deref(),
        )?;
        return dump_native_wrapper_assets(&mut builder, out_dir);
    }

    if let Some(manifest) = &cli.batch_manifest {
        let entries = parse_batch_manifest(manifest)?;
        return compile_batch_with_shared_prelude(
            &cli,
            &prelude_expr,
            &prelude_source,
            subject_type_jam.as_deref(),
            &entries,
        )
        .await;
    }

    let entry = cli
        .entry
        .as_ref()
        .ok_or_else(|| "missing entry path".to_string())?;
    let output = cli
        .output
        .as_ref()
        .ok_or_else(|| "missing output path".to_string())?;

    let mut builder = build_context_with_shared_prelude(
        &cli,
        &prelude_expr,
        &prelude_source,
        subject_type_jam.as_deref(),
    )?;
    let mut product = builder.compile_entry(entry)?;
    let mut jam = builder.jam_product(&mut product, cli.mode, entry, None)?;
    pad_hoonc_jam_atom_bytes(&mut jam);

    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, jam)?;
    Ok(())
}

fn parse_prelude_hoon(path: &Path, dbug: bool, docs: bool) -> Result<Hoon> {
    let _parse_log = TimedHoonPathLog::new(path, HoonLogOperation::Parse);
    let source = fs::read_to_string(path)?;
    if docs {
        Ok(pipeline::parse_native_hoon_source(
            path,
            source.as_str(),
            Vec::new(),
            dbug,
        )?)
    } else {
        Ok(pipeline::parse_native_hoon_source_without_docs(
            path,
            source.as_str(),
            Vec::new(),
            dbug,
        )?)
    }
}

fn parse_inline_wrapper(name: &str, source: &str, wer: Vec<String>, dbug: bool) -> Result<Hoon> {
    let path = PathBuf::from(format!("/native/wrappers/{name}.hoon"));
    Ok(pipeline::parse_native_hoon_source(
        &path, source, wer, dbug,
    )?)
}

fn parse_build_leaf(
    path: &Path,
    deps_dir: &Path,
    is_entry: bool,
    absolute_entry_wer: bool,
    dbug: bool,
) -> Result<Hoon> {
    let _parse_log = TimedHoonPathLog::new(path, HoonLogOperation::Parse);
    let source = fs::read_to_string(path)?;
    let wer = if is_entry {
        build_entry_wer(path, deps_dir, absolute_entry_wer)
    } else {
        build_import_wer(path, deps_dir)
    };
    // Build-leaf docs remain off for byte parity. The hoon-138 self-mint keeps
    // docs on the shared prelude leg; enabling them on the entry leg reparses the
    // whole source with duplicate doc-bearing ASTs and regresses compile time.
    Ok(pipeline::parse_native_hoon_source_without_docs(
        path,
        source.as_str(),
        wer,
        dbug,
    )?)
}

fn hoonc_wrapper_source(first_line: usize, body: &str) -> String {
    let mut source = "\n".repeat(first_line.saturating_sub(1));
    source.push_str(body);
    if !source.ends_with('\n') {
        source.push('\n');
    }
    source
}

fn hoonc_standard_output_source() -> String {
    let mut source = hoonc_wrapper_source(
        556,
        r#"=<
|=  [tase=(trap vase) dir-hash=@uvI]
  ^-  *
  =>  %+  shot  tase
    =>  d=!>(dir-hash)
    |.(d)
  |.(+:^$)
"#,
    );
    source.push_str(&"\n".repeat(507));
    source.push_str(
        r#"|%
++  shot
  ~/  %shot
  |=  [gat=(trap vase) sam=(trap vase)]
  ^-  (trap vase)
  =/  [typ=type gen=hoon]
    :-  [%cell p:$:gat p:$:sam]
    [%cnsg [%$ ~] [%$ 2] [%$ 3] ~]
  =+  gun=(~(mint ut typ) %noun gen)
  =>  [typ=p.gun +<.$]
  |.
  [typ .*([q:$:gat q:$:sam] [%9 2 %10 [6 %0 3] %0 2])]
--
"#,
    );
    source
}

fn hoonc_dir_hash_source() -> String {
    let mut source = hoonc_wrapper_source(
        535,
        r#"=<
|=  [target=@t contents=@t directory=(list [@t @t])]
  ^-  @uvI
  =/  target-path=path  (parse-file-path target)
  =/  dir
    %-  ~(gas by *(map path cord))
    :-  [target-path contents]
    (turn directory |=((pair @t @t) [(stab p) q]))
  `@uvI`(mug dir)
"#,
    );
    source.push_str(&"\n".repeat(18));
    source.push_str(
        r#"|%
++  parse-file-path
  |=  pat=cord
  (rash pat gawp)
++  gawp
  %+  sear
    |=  p=path
    ^-  (unit path)
    ?:  ?=([~ ~] p)  `~
    ?.  =(~ (rear p))  `p
    ~
  ;~(pfix fas (most fas bic))
++  bic
  %+  cook
  |=(a=tape (rap 3 ^-((list @) a)))
  (star ;~(pose nud low hig hep dot sig cab))
--
"#,
    );
    source
}

fn pad_hoonc_jam_atom_bytes(jam: &mut Vec<u8>) {
    let rem = jam.len() % 8;
    if rem != 0 {
        jam.resize(jam.len() + (8 - rem), 0);
    }
}

fn hoonc_wrapper_wer() -> Vec<String> {
    vec!["apps".to_string(), "hoonc".to_string(), "hoonc.hoon".to_string()]
}

fn build_entry_wer(path: &Path, deps_dir: &Path, absolute_entry_wer: bool) -> Vec<String> {
    if !absolute_entry_wer || path_is_inside_dir(path, deps_dir) {
        return build_import_wer(path, deps_dir);
    }
    let lexical_path = lexical_absolute_path(path).unwrap_or_else(|_| path.to_path_buf());
    let wer_path = path.canonicalize().unwrap_or(lexical_path);
    path_components_for_dbug(&wer_path)
}

fn path_is_inside_dir(path: &Path, dir: &Path) -> bool {
    let lexical_path = lexical_absolute_path(path).unwrap_or_else(|_| path.to_path_buf());
    let lexical_dir = lexical_absolute_path(dir).unwrap_or_else(|_| dir.to_path_buf());
    if lexical_path.strip_prefix(&lexical_dir).is_ok() {
        return true;
    }

    let canonical_path = path.canonicalize().unwrap_or(lexical_path);
    let canonical_dir = dir.canonicalize().unwrap_or(lexical_dir);
    canonical_path.strip_prefix(&canonical_dir).is_ok()
        || matching_hoon_root_marker(path, dir).is_some()
}

fn matching_hoon_root_marker(path: &Path, dir: &Path) -> Option<String> {
    let path_marker = hoon_root_marker(&lexical_absolute_path(path).ok()?);
    let dir_marker = hoon_root_marker(&lexical_absolute_path(dir).ok()?);
    match (path_marker, dir_marker) {
        (Some(path_marker), Some(dir_marker)) if path_marker == dir_marker => Some(path_marker),
        _ => None,
    }
}

fn build_import_wer(path: &Path, deps_dir: &Path) -> Vec<String> {
    let wer_path = path
        .canonicalize()
        .or_else(|_| lexical_absolute_path(path))
        .unwrap_or_else(|_| path.to_path_buf());
    let lexical_path = lexical_absolute_path(path).unwrap_or_else(|_| path.to_path_buf());
    let lexical_base = lexical_absolute_path(deps_dir).unwrap_or_else(|_| deps_dir.to_path_buf());
    if let Ok(rel) = lexical_path.strip_prefix(&lexical_base) {
        return path_components_for_dbug(rel);
    }

    let wer_base = deps_dir
        .canonicalize()
        .or_else(|_| lexical_absolute_path(deps_dir))
        .unwrap_or_else(|_| deps_dir.to_path_buf());
    if let Ok(relative) = wer_path.strip_prefix(&wer_base) {
        return path_components_for_dbug(relative);
    }
    if matching_hoon_root_marker(path, deps_dir).is_some() {
        if let Some(components) = hoon_relative_components(path) {
            return components;
        }
    }
    path_components_for_dbug(&wer_path)
}

fn hoon_source_content_key(path: &Path) -> Result<blake3::Hash> {
    Ok(blake3::hash(&fs::read(path)?))
}

fn hoon_root_marker(path: &Path) -> Option<String> {
    for ancestor in path.ancestors() {
        if ancestor.file_name().and_then(|seg| seg.to_str()) != Some("hoon") {
            continue;
        }
        let parent = ancestor
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|seg| seg.to_str());
        if matches!(parent, Some("open" | "closed")) {
            return parent.map(ToOwned::to_owned);
        }
    }
    None
}

fn path_components_for_dbug(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(seg) => Some(seg.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn hoon_relative_components(path: &Path) -> Option<Vec<String>> {
    let path = lexical_absolute_path(path).ok()?;
    let components = normal_path_components(&path);
    let marker_idx = components.windows(2).position(|window| {
        matches!(window, [scope, hoon] if matches!(scope.as_str(), "open" | "closed") && hoon == "hoon")
    })?;
    Some(components[marker_idx + 2..].to_vec())
}

/// When set (HONK_NATIVE_PARITY), the canonical hoon-138 build does NOT
/// substitute hoonc's embedded prelude formula/subject-type; honk mints them
/// natively instead. This exposes whether honk's own +mint of the prelude
/// matches hoonc, rather than passing parity by importing hoonc-produced
/// nouns. The honk-produced cold state and the canonical $octs input are kept
/// (they are jet-registration / data inputs, not compiler artifacts).
fn native_parity_enabled() -> bool {
    std::env::var_os("HONK_NATIVE_PARITY").is_some()
}

fn build_context_with_shared_prelude(
    cli: &Cli,
    prelude: &Hoon,
    prelude_source: &str,
    subject_type_jam: Option<&[u8]>,
) -> Result<NativeBuildContext<'static>> {
    let slab = Box::leak(Box::new(NounSlab::new()));
    let mut ut = Ut::new(slab);
    let canonical_hoon_138 = prelude_source.as_bytes() == EMBEDDED_HOON_138_SOURCE;
    let have_embedded_cold = !EMBEDDED_HONC_COLD_138_JAM.is_empty();
    let mut eval_context = create_eval_context();
    if canonical_hoon_138 && have_embedded_cold {
        trace_timed("loading canonical honc cold state", || {
            load_cold_state(
                &mut eval_context, EMBEDDED_HONC_COLD_138_JAM, "canonical honc",
            )
        })?;
        trace_timed("loading canonical honc musk cold state", || {
            ut.load_musk_cold_state(EMBEDDED_HONC_COLD_138_JAM, "canonical honc")
                .map_err(|err| -> DynError { Box::new(err) })
        })?;
    }
    // The seed play's only product is the returned prelude type. On the
    // embedded / supplied-subject-type path that type is overwritten below by a
    // cued subject type, so the full-prelude play is pure waste — skip it.
    // Native-parity now overwrites this type with the MINTED prelude type
    // (complete coils; see the parity fix below), so the seed play is pure waste
    // in every mode — skip it. (Native-parity used to pay the O(N^2) prelude play
    // here AND mint it again for the formula.)
    let skip_prelude_play = subject_type_jam.is_some()
        || (canonical_hoon_138 && !native_parity_enabled())
        || native_parity_enabled();
    let mut prelude_vase = trace_timed("seeding shared honc type", || {
        seed_honc_type_with_ut(&mut ut, &mut eval_context, prelude, skip_prelude_play)
    })?;
    if canonical_hoon_138 && !have_embedded_cold {
        trace_timed(
            "evaluating canonical honc prelude for cold fallback",
            || {
                let mut fallback_formula_slab: NounSlab<NockJammer> = NounSlab::new();
                let _ =
                    evaluate_honc_isolated(&mut eval_context, prelude, &mut fallback_formula_slab)?;
                Ok(())
            },
        )?;
    }
    let use_embedded = canonical_hoon_138 && !native_parity_enabled();
    let prelude_formula = if use_embedded {
        trace_timed("cueing canonical honc formula", || {
            cue_honc_formula_to_slab(&mut *ut.slab, EMBEDDED_HONC_FORMULA_138_JAM)
        })?
    } else {
        let (minted_ty, formula) = trace_timed("minting shared honc formula", || {
            mint_honc_formula_with_ut(&mut ut, &mut eval_context, prelude)
        })?;
        // PARITY FIX (native self-mint): use the MINTED prelude type as the subject
        // type, NOT the play-seeded one. `++play` (seed_honc_type_with_ut) builds
        // core types with the blocked `*seminoun`; `++mint` (here) computes the
        // actual battery seminoun (`[%full ~]` complete). The `!>`-vase output runs
        // `++burp`, which KEEPS complete coils but BLANKS everything else to blocked
        // — so the played subject left every stdlib core blocked where hoonc (which
        // mints) has them complete, the root of the native≠hoonc divergence. The
        // mint already runs for the formula, so this reuses its type (no extra work;
        // the now-redundant seed play is left intact to preserve cache/memo setup).
        let minted_ty = if std::env::var_os("NATIVE_HOON_SKIP_BURP").is_some() {
            minted_ty
        } else {
            ut.burp_type(minted_ty)?
        };
        prelude_vase.ty = minted_ty;
        formula
    };
    // Native-types migration Phase 1: flag-gated IR-completeness invariant on the
    // largest real formula honk has — the entire compiled hoon-138 prelude.
    // Asserts the native Formula IR can represent and re-emit it byte-for-byte.
    // Default-off (HONK_IR_ROUNDTRIP) → zero impact on the shipping path.
    if env::var_os("HONK_IR_ROUNDTRIP").is_some() {
        let space = ut.slab.noun_space();
        honk::native::ir::roundtrip_check(prelude_formula, &space)?;
        if env::var_os("NATIVE_HOON_TRACE").is_some() {
            eprintln!("[ir-roundtrip] prelude formula OK (native IR represents the compiled hoon-138 prelude byte-exact)");
        }
    }
    let prelude_eval_value = D(0);
    let subject_type_jam =
        subject_type_jam.or_else(|| use_embedded.then_some(EMBEDDED_HONC_TYPE_138_JAM));
    let subject_type_override = if let Some(subject_type_jam) = subject_type_jam {
        Some(trace_timed("cueing shared subject type override", || {
            cue_subject_type_to_slab(&mut *ut.slab, subject_type_jam)
        })?)
    } else {
        None
    };
    if let Some(subject_ty) = subject_type_override {
        let space = ut.slab.noun_space();
        // Native-types migration: type-IR completeness invariant on the real
        // prelude subject type (the entire compiled hoon-138 type). Default-off.
        if env::var_os("HONK_IR_ROUNDTRIP").is_some() {
            honk::native::ir::type_roundtrip_check(subject_ty, &space)?;
            if env::var_os("NATIVE_HOON_TRACE").is_some() {
                eprintln!("[ir-roundtrip] prelude TYPE OK (native type IR represents the compiled hoon-138 subject type byte-exact)");
                if let Ok((total, distinct)) =
                    honk::native::ir::type_intern_stats(subject_ty, &space)
                {
                    let ratio = if distinct > 0 {
                        total as f64 / distinct as f64
                    } else {
                        0.0
                    };
                    eprintln!("[ir-intern] prelude TYPE hash-cons: {total} unshared nodes -> {distinct} distinct ({ratio:.1}x dedup)");
                }
            }
        }
        prelude_vase.ty = prelude_type_from_subject_type(subject_ty, &space)?;
    }
    prelude_vase.eval_value = None;
    prelude_vase.trap = D(0);
    let wrapper_subject_ty = {
        let empty_ty = empty_subject_type(&mut *ut.slab);
        ty_cell_local(&mut *ut.slab, empty_ty, prelude_vase.ty)
    };
    // build.rs makes the embedded $octs asset mandatory, so a canonical
    // hoon-138 build always has it; never silently fall back to the local
    // `[p=@ud q=@]` approximation for canonical data imports.
    assert!(
        !canonical_hoon_138 || !EMBEDDED_HOONC_OCTS_TYPE_138_JAM.is_empty(),
        "canonical hoon-138 build is missing the embedded hoonc $octs type; \
         regenerate crates/honk/assets/hoonc-octs-type-138.jam"
    );
    let canonical_data_octs_ty =
        if canonical_hoon_138 && !EMBEDDED_HOONC_OCTS_TYPE_138_JAM.is_empty() {
            Some(trace_timed("cueing canonical hoonc octs type", || {
                cue_noun_to_slab(
                    &mut *ut.slab, EMBEDDED_HOONC_OCTS_TYPE_138_JAM, "hoonc octs type",
                )
            })?)
        } else {
            None
        };
    let directory = cli
        .directory
        .canonicalize()
        .unwrap_or_else(|_| cli.directory.clone());
    let mut builder = NativeBuildContext::new(
        directory,
        prelude_vase,
        prelude_eval_value,
        wrapper_subject_ty,
        D(0),
        canonical_data_octs_ty,
        cli.dbug,
        cli.vet,
        ut,
        eval_context,
    );
    trace_timed("constructing native hoonc wrapper traps", || {
        builder.initialize_native_wrappers(prelude_formula)
    })?;
    Ok(builder)
}

fn build_context_with_dynamic_wrapper_prelude(
    cli: &Cli,
    prelude: &Hoon,
    prelude_eval_subject: &Hoon,
    prelude_source: &str,
    subject_type_jam: Option<&[u8]>,
) -> Result<NativeBuildContext<'static>> {
    let slab = Box::leak(Box::new(NounSlab::new()));
    let mut ut = Ut::new(slab);
    let mut eval_context = create_eval_context();
    let canonical_hoon_138 = prelude_source.as_bytes() == EMBEDDED_HOON_138_SOURCE;
    let have_embedded_cold = !EMBEDDED_HONC_COLD_138_JAM.is_empty();
    if canonical_hoon_138 && have_embedded_cold {
        trace_timed("loading canonical honc cold state", || {
            load_cold_state(
                &mut eval_context, EMBEDDED_HONC_COLD_138_JAM, "canonical honc",
            )
        })?;
        trace_timed("loading canonical honc musk cold state", || {
            ut.load_musk_cold_state(EMBEDDED_HONC_COLD_138_JAM, "canonical honc")
                .map_err(|err| -> DynError { Box::new(err) })
        })?;
    }
    // The seed play's only product is the returned prelude type. On the
    // embedded / supplied-subject-type path that type is overwritten below by a
    // cued subject type, so the full-prelude play is pure waste — skip it.
    let skip_prelude_play =
        subject_type_jam.is_some() || (canonical_hoon_138 && !native_parity_enabled());
    let mut prelude_vase = trace_timed("seeding shared honc type", || {
        seed_honc_type_with_ut(&mut ut, &mut eval_context, prelude, skip_prelude_play)
    })?;
    let use_embedded = canonical_hoon_138 && !native_parity_enabled();
    let subject_type_jam =
        subject_type_jam.or_else(|| use_embedded.then_some(EMBEDDED_HONC_TYPE_138_JAM));
    let subject_type_override = if let Some(subject_type_jam) = subject_type_jam {
        Some(trace_timed("cueing shared subject type override", || {
            cue_subject_type_to_slab(&mut *ut.slab, subject_type_jam)
        })?)
    } else {
        None
    };
    if let Some(subject_ty) = subject_type_override {
        let space = ut.slab.noun_space();
        // Native-types migration: type-IR completeness invariant on the real
        // prelude subject type (the entire compiled hoon-138 type). Default-off.
        if env::var_os("HONK_IR_ROUNDTRIP").is_some() {
            honk::native::ir::type_roundtrip_check(subject_ty, &space)?;
            if env::var_os("NATIVE_HOON_TRACE").is_some() {
                eprintln!("[ir-roundtrip] prelude TYPE OK (native type IR represents the compiled hoon-138 subject type byte-exact)");
                if let Ok((total, distinct)) =
                    honk::native::ir::type_intern_stats(subject_ty, &space)
                {
                    let ratio = if distinct > 0 {
                        total as f64 / distinct as f64
                    } else {
                        0.0
                    };
                    eprintln!("[ir-intern] prelude TYPE hash-cons: {total} unshared nodes -> {distinct} distinct ({ratio:.1}x dedup)");
                }
            }
        }
        prelude_vase.ty = prelude_type_from_subject_type(subject_ty, &space)?;
    }
    let prelude_eval = trace_timed("evaluating shared honc", || {
        evaluate_honc_isolated(&mut eval_context, prelude_eval_subject, &mut *ut.slab)
    })?;
    prelude_vase.eval_value = Some(prelude_eval.value);
    prelude_vase.trap = D(0);
    let wrapper_subject_ty = {
        let empty_ty = empty_subject_type(&mut *ut.slab);
        ty_cell_local(&mut *ut.slab, empty_ty, prelude_vase.ty)
    };
    let wrapper_subject_value = T(&mut eval_context.stack, &[D(0), prelude_eval.value]);
    // build.rs makes the embedded $octs asset mandatory, so a canonical
    // hoon-138 build always has it; never silently fall back to the local
    // `[p=@ud q=@]` approximation for canonical data imports.
    assert!(
        !canonical_hoon_138 || !EMBEDDED_HOONC_OCTS_TYPE_138_JAM.is_empty(),
        "canonical hoon-138 build is missing the embedded hoonc $octs type; \
         regenerate crates/honk/assets/hoonc-octs-type-138.jam"
    );
    let canonical_data_octs_ty =
        if canonical_hoon_138 && !EMBEDDED_HOONC_OCTS_TYPE_138_JAM.is_empty() {
            Some(trace_timed("cueing canonical hoonc octs type", || {
                cue_noun_to_slab(
                    &mut *ut.slab, EMBEDDED_HOONC_OCTS_TYPE_138_JAM, "hoonc octs type",
                )
            })?)
        } else {
            None
        };
    let directory = cli
        .directory
        .canonicalize()
        .unwrap_or_else(|_| cli.directory.clone());
    let mut builder = NativeBuildContext::new(
        directory, prelude_vase, prelude_eval.value, wrapper_subject_ty, wrapper_subject_value,
        canonical_data_octs_ty, cli.dbug, cli.vet, ut, eval_context,
    );
    trace_timed("building exact hoonc wrapper traps", || {
        builder.initialize_exact_wrappers(prelude_eval.formula, prelude_source)
    })?;
    Ok(builder)
}

async fn compile_batch_with_shared_prelude(
    cli: &Cli,
    prelude: &Hoon,
    prelude_source: &str,
    subject_type_jam: Option<&[u8]>,
    entries: &[BatchEntry],
) -> Result<()> {
    let mut builder =
        build_context_with_shared_prelude(cli, prelude, prelude_source, subject_type_jam)?;
    for entry in entries {
        // Batch entries intentionally share this builder/Ut so dependency and native
        // IR caches can be reused. The build slab is leaked for the builder lifetime,
        // so native Rc keys never point at a dropped entry arena; semantic cache keys
        // still carry vet/fan/arm context for parity-sensitive misses.
        let label = format!("{}", entry.entry.display());
        trace_native(format!("batch compiling {label}"));
        let mut product = builder.compile_entry(&entry.entry)?;
        let mut jam = builder.jam_product(
            &mut product,
            entry.mode,
            &entry.entry,
            entry.directory_files.as_deref(),
        )?;
        pad_hoonc_jam_atom_bytes(&mut jam);
        if let Some(parent) = entry
            .output
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(&entry.output, jam)?;
    }
    Ok(())
}

#[derive(Clone)]
struct NativeVase {
    ty: Noun,
    eval_value: Option<Noun>,
    trap: Noun,
}

type PathCacheKey = PathBuf;
type ContentCacheKey = blake3::Hash;

struct PreludeEval {
    value: Noun,
    formula: Noun,
}

#[derive(Clone, Copy)]
struct ExactWrapperBatteries {
    constant_vase: Noun,
    data_vase: Noun,
    dir_hash: Noun,
    dir_hash_vase: Noun,
    eval_vase: Noun,
    label_vase: Noun,
    mint_gun: Noun,
    slat: Noun,
    swet_gate: Noun,
    swet_gun: Noun,
    shot: Noun,
    shot_gun: Noun,
    standard_output: Noun,
    value_trap_arbitrary: Noun,
    value_trap_standard: Noun,
}

impl ExactWrapperBatteries {
    fn empty() -> Self {
        Self {
            constant_vase: D(0),
            data_vase: D(0),
            dir_hash: D(0),
            dir_hash_vase: D(0),
            eval_vase: D(0),
            label_vase: D(0),
            mint_gun: D(0),
            slat: D(0),
            swet_gate: D(0),
            swet_gun: D(0),
            shot: D(0),
            shot_gun: D(0),
            standard_output: D(0),
            value_trap_arbitrary: D(0),
            value_trap_standard: D(0),
        }
    }
}

#[derive(Clone)]
struct SubjectVase {
    ty: Noun,
    eval_value: Option<Noun>,
    trap: Noun,
}

struct NativeBuildProduct {
    ty: Noun,
    eval_value: Option<Noun>,
    formula: Noun,
    vase_trap: Option<Noun>,
    standard_jam: Option<Vec<u8>>,
}

struct NativeBuildContext<'a> {
    directory: PathBuf,
    prelude_vase: NativeVase,
    prelude_eval_value: Noun,
    wrapper_subject_ty: Noun,
    wrapper_subject_value: Noun,
    canonical_data_octs_ty: Option<Noun>,
    dbug: bool,
    ut: Ut<'a>,
    eval_context: Context,
    cache: HashMap<PathCacheKey, NativeVase>,
    content_cache: HashMap<ContentCacheKey, NativeVase>,
    visiting: HashSet<PathBuf>,
    wrappers: ExactWrapperBatteries,
    empty_trap_vase: Noun,
    entry_vet: bool,
}

impl<'a> NativeBuildContext<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        directory: PathBuf,
        prelude_vase: NativeVase,
        prelude_eval_value: Noun,
        wrapper_subject_ty: Noun,
        wrapper_subject_value: Noun,
        canonical_data_octs_ty: Option<Noun>,
        dbug: bool,
        entry_vet: bool,
        ut: Ut<'a>,
        eval_context: Context,
    ) -> Self {
        Self {
            directory,
            prelude_vase,
            prelude_eval_value,
            wrapper_subject_ty,
            wrapper_subject_value,
            canonical_data_octs_ty,
            dbug,
            ut,
            eval_context,
            cache: HashMap::new(),
            content_cache: HashMap::new(),
            visiting: HashSet::new(),
            wrappers: ExactWrapperBatteries::empty(),
            empty_trap_vase: D(0),
            entry_vet,
        }
    }

    fn initialize_native_wrappers(&mut self, prelude_formula: Noun) -> Result<()> {
        let (wrappers, empty_trap_vase) = construct_exact_wrapper_batteries(&mut *self.ut.slab)?;
        self.wrappers = wrappers;
        self.empty_trap_vase = empty_trap_vase;

        let prelude_gun = T(&mut *self.ut.slab, &[self.prelude_vase.ty, prelude_formula]);
        let prelude_sample = T(&mut *self.ut.slab, &[prelude_gun, self.empty_trap_vase]);
        self.prelude_vase.trap = self.trap_from_payload(self.wrappers.swet_gun, prelude_sample);
        let space = self.ut.slab.noun_space();
        if let Ok(ty) = swet_trap_payload_type(self.prelude_vase.trap, &space) {
            self.prelude_vase.ty = ty;
        }
        Ok(())
    }

    fn initialize_exact_wrappers(
        &mut self,
        prelude_eval_formula: Noun,
        prelude_source: &str,
    ) -> Result<()> {
        let gates = self.compile_exact_wrapper_gates(None)?;
        self.wrappers = self.extract_exact_wrapper_batteries(gates)?;

        let empty_trap_exact = self.compile_wrapper_expression(
            "empty-trap-vase",
            hoonc_wrapper_source(
                20, r#"  ^-  (trap vase)
  =>  vaz=!>(~)
  |.(vaz)
"#,
            )
            .as_str(),
        )?;
        let empty_vase = {
            let empty_ty = ty_atom_local(&mut *self.ut.slab, "n", Some(D(0)));
            vase_native(&mut *self.ut.slab, empty_ty, D(0))
        };
        let empty_trap_battery = {
            let space = self.ut.slab.noun_space();
            trap_battery(empty_trap_exact, &space)?
        };
        let empty_trap = self.trap_from_payload(empty_trap_battery, empty_vase);
        self.empty_trap_vase = empty_trap;

        // hoonc arbitrary artifacts serialize the exact +build-honc formula produced by hoon.hoon.
        // Keep canonical hoon-138 artifacts byte-identical while preserving the native fallback for
        // non-canonical preludes (and for the HONK_NATIVE_PARITY audit, which mints natively).
        let prelude_eval_formula =
            if prelude_source.as_bytes() == EMBEDDED_HOON_138_SOURCE && !native_parity_enabled() {
                cue_honc_formula_to_slab(&mut *self.ut.slab, EMBEDDED_HONC_FORMULA_138_JAM)?
            } else {
                prelude_eval_formula
            };

        let prelude_gun = T(
            &mut *self.ut.slab,
            &[self.prelude_vase.ty, prelude_eval_formula],
        );
        let prelude_sample = T(&mut *self.ut.slab, &[prelude_gun, empty_trap]);
        self.prelude_vase.trap = self.trap_from_payload(self.wrappers.swet_gun, prelude_sample);
        let space = self.ut.slab.noun_space();
        if let Ok(ty) = swet_trap_payload_type(self.prelude_vase.trap, &space) {
            self.prelude_vase.ty = ty;
        }
        let mut wrapper_subject_trap =
            self.slat_vase_trap(self.empty_trap_vase, self.prelude_vase.trap)?;
        for _ in 0..2 {
            let exact_gates = self.compile_exact_wrapper_gates(Some(wrapper_subject_trap))?;
            self.wrappers = self.extract_exact_wrapper_batteries(exact_gates)?;
            self.prelude_vase.trap = self.trap_from_payload(self.wrappers.swet_gun, prelude_sample);
            let space = self.ut.slab.noun_space();
            if let Ok(ty) = swet_trap_payload_type(self.prelude_vase.trap, &space) {
                self.prelude_vase.ty = ty;
            }
            wrapper_subject_trap =
                self.slat_vase_trap(self.empty_trap_vase, self.prelude_vase.trap)?;
        }
        Ok(())
    }

    fn compile_wrapper_gate_maybe_exact(
        &mut self,
        exact_subject_trap: Option<Noun>,
        name: &str,
        source: &str,
    ) -> Result<Noun> {
        match exact_subject_trap {
            Some(subject_trap) => self.compile_wrapper_gate_exact(name, source, subject_trap),
            None => self.compile_wrapper_gate(name, source),
        }
    }

    fn compile_exact_wrapper_gates(
        &mut self,
        exact_subject_trap: Option<Noun>,
    ) -> Result<ExactWrapperBatteries> {
        Ok(ExactWrapperBatteries {
            constant_vase: self.compile_wrapper_gate_maybe_exact(
                exact_subject_trap, "constant-vase",
                r#"
|=  vaz=vase
^-  (trap vase)
=>  vaz=vaz
|.(vaz)
"#,
            )?,
            // Hoonc compiles non-Hoon `/*` graph leaves by vasing the file
            // bytes as its local `octs` type before returning a constant trap.
            data_vase: match exact_subject_trap {
                Some(subject_trap) => self.compile_wrapper_gate_exact(
                    "data-vase",
                    r#"
=<
|=  octs=octs
^-  (trap vase)
=>  d=!>(octs)
|.(d)
|%
+$  octs  [p=@ud q=@]
--
"#,
                    subject_trap,
                )?,
                None => D(0),
            },
            dir_hash_vase: self.compile_wrapper_gate_maybe_exact(
                exact_subject_trap,
                "dir-hash-vase",
                hoonc_wrapper_source(
                    559, r#"|=  dir-hash=@uvI
    =>  d=!>(dir-hash)
    |.(d)
"#,
                )
                .as_str(),
            )?,
            dir_hash: self.compile_wrapper_gate_maybe_exact(
                exact_subject_trap,
                "dir-hash",
                hoonc_dir_hash_source().as_str(),
            )?,
            eval_vase: self.compile_wrapper_gate_maybe_exact(
                exact_subject_trap,
                "eval-vase",
                hoonc_wrapper_source(
                    1001, r#"|=  vaz=vase
    ^-  (trap vase)
    =>  vaz=vaz
    |.(vaz)
"#,
                )
                .as_str(),
            )?,
            label_vase: self.compile_wrapper_gate_maybe_exact(
                exact_subject_trap,
                "label-vase",
                hoonc_wrapper_source(
                    1042,
                    r#"  |=  [vaz=(trap vase) face=(unit @tas)]
  ^-  (trap vase)
  ?~  face  vaz
  =>  [vaz=vaz face=u.face]
  |.
  =/  vas  $:vaz
  [[%face face p.vas] q.vas]
"#,
                )
                .as_str(),
            )?,
            mint_gun: self.compile_wrapper_gate_maybe_exact(
                exact_subject_trap,
                "mint-gun",
                hoonc_wrapper_source(
                    1089,
                    r#"  ~/  %swet
  |=  [tap=(trap vase) gen=hoon]
  ^-  vase
  (~(mint ut p:$:tap) %noun gen)
"#,
                )
                .as_str(),
            )?,
            slat: self.compile_wrapper_gate_maybe_exact(
                exact_subject_trap,
                "slat",
                hoonc_wrapper_source(
                    1057,
                    r#"  |=  [hed=(trap vase) tal=(trap vase)]
  ^-  (trap vase)
  =>  +<
  |.
  =+  [bed bal]=[$:hed $:tal]
  [[%cell p:bed p:bal] [q:bed q:bal]]
"#,
                )
                .as_str(),
            )?,
            swet_gate: self.compile_wrapper_gate_maybe_exact(
                exact_subject_trap,
                "swet",
                hoonc_wrapper_source(
                    1089,
                    r#"  ~/  %swet
  |=  [tap=(trap vase) gen=hoon]
  ^-  (trap vase)
  =/  gun  (~(mint ut p:$:tap) %noun gen)
  =>  [gun=gun tap=tap]
  |.  ~+
  [p.gun .*(q:$:tap q.gun)]
"#,
                )
                .as_str(),
            )?,
            swet_gun: self.compile_wrapper_gate_maybe_exact(
                exact_subject_trap,
                "swet-gun",
                hoonc_wrapper_source(
                    1089,
                    r#"  ~/  %swet
  |=  [tap=(trap vase) gun=vase]
  ^-  (trap vase)
  =/  gun  gun
  =>  [gun=gun tap=tap]
  |.  ~+
  [p.gun .*(q:$:tap q.gun)]
"#,
                )
                .as_str(),
            )?,
            shot: D(0),
            shot_gun: self.compile_wrapper_gate_maybe_exact(
                exact_subject_trap,
                "shot-gun",
                hoonc_wrapper_source(
                    1072,
                    r#"  ~/  %shot
  |=  [gat=(trap vase) sam=(trap vase)]
  ^-  (trap vase)
  =/  [typ=type gen=hoon]
    :-  [%cell p:$:gat p:$:sam]
    [%cnsg [%$ ~] [%$ 2] [%$ 3] ~]
  =+  gun=(~(mint ut typ) %noun gen)
  =>  [typ=p.gun +<.$]
  |.
  [typ .*([q:$:gat q:$:sam] [%9 2 %10 [6 %0 3] %0 2])]
"#,
                )
                .as_str(),
            )?,
            standard_output: self.compile_wrapper_gate_maybe_exact(
                exact_subject_trap,
                "standard-output",
                hoonc_standard_output_source().as_str(),
            )?,
            value_trap_arbitrary: self.compile_wrapper_gate_maybe_exact(
                exact_subject_trap,
                "value-trap-arbitrary",
                hoonc_wrapper_source(
                    553, r#"|=  vaz=(trap vase)
   =>  vaz
   |.(+:^$)
"#,
                )
                .as_str(),
            )?,
            value_trap_standard: self.compile_wrapper_gate_maybe_exact(
                exact_subject_trap,
                "value-trap-standard",
                hoonc_wrapper_source(
                    560, r#"|=  vaz=(trap vase)
  =>  vaz
  |.(+:^$)
"#,
                )
                .as_str(),
            )?,
        })
    }

    fn extract_exact_wrapper_batteries(
        &mut self,
        gates: ExactWrapperBatteries,
    ) -> Result<ExactWrapperBatteries> {
        let zero = D(0);
        let dummy_vase = {
            let ty = ty_atom_local(&mut *self.ut.slab, "n", Some(zero));
            vase_native(&mut *self.ut.slab, ty, zero)
        };
        let constant_trap =
            self.slam_wrapper_gate(gates.constant_vase, dummy_vase, "constant-vase")?;
        let constant_vase = {
            let space = self.ut.slab.noun_space();
            trap_battery(constant_trap, &space)?
        };
        let eval_trap = self.slam_wrapper_gate(gates.eval_vase, dummy_vase, "eval-vase")?;
        let eval_vase = {
            let space = self.ut.slab.noun_space();
            trap_battery(eval_trap, &space)?
        };

        let dummy_trap = constant_trap;
        let face = term_to_noun(&mut *self.ut.slab, "dummy");
        let face = T(&mut *self.ut.slab, &[D(0), face]);
        let label_sample = T(&mut *self.ut.slab, &[dummy_trap, face]);
        let label_trap = self.slam_wrapper_gate(gates.label_vase, label_sample, "label-vase")?;
        let label_vase = {
            let space = self.ut.slab.noun_space();
            trap_battery(label_trap, &space)?
        };

        let slat_sample = T(&mut *self.ut.slab, &[dummy_trap, dummy_trap]);
        let slat_trap = self.slam_wrapper_gate(gates.slat, slat_sample, "slat")?;
        let slat = {
            let space = self.ut.slab.noun_space();
            trap_battery(slat_trap, &space)?
        };

        let dummy_gun = {
            let ty = ty_atom_local(&mut *self.ut.slab, "n", Some(zero));
            T(&mut *self.ut.slab, &[ty, zero])
        };
        let swet_sample = T(&mut *self.ut.slab, &[dummy_trap, dummy_gun]);
        let swet_trap = self.slam_wrapper_gate(gates.swet_gun, swet_sample, "swet")?;
        let swet_gun = {
            let space = self.ut.slab.noun_space();
            trap_battery(swet_trap, &space)?
        };

        let dummy_gate_trap = {
            let expr = parse_inline_wrapper(
                "shot-dummy-gate",
                r#"|=  a=@
  a
"#,
                vec!["native".to_string(), "wrappers".to_string(), "shot".to_string()],
                false,
            )?;
            let subject_ty = empty_subject_type(&mut *self.ut.slab);
            let (gate_ty, _gate_formula) = self.mint_with_subject_type(subject_ty, &expr)?;
            let gate_vase = vase_native(&mut *self.ut.slab, gate_ty, D(0));
            self.trap_from_payload(constant_vase, gate_vase)
        };
        let shot_sample = T(&mut *self.ut.slab, &[dummy_gate_trap, dummy_trap]);
        let shot_trap = self.slam_wrapper_gate(gates.shot_gun, shot_sample, "shot")?;
        let shot = {
            let space = self.ut.slab.noun_space();
            trap_battery(shot_trap, &space)?
        };

        let value_trap_arbitrary = self.slam_wrapper_gate(
            gates.value_trap_arbitrary, dummy_trap, "value-trap-arbitrary",
        )?;
        let value_trap_arbitrary = {
            let space = self.ut.slab.noun_space();
            trap_battery(value_trap_arbitrary, &space)?
        };

        let value_trap_standard =
            self.slam_wrapper_gate(gates.value_trap_standard, dummy_trap, "value-trap-standard")?;
        let value_trap_standard = {
            let space = self.ut.slab.noun_space();
            trap_battery(value_trap_standard, &space)?
        };

        Ok(ExactWrapperBatteries {
            constant_vase,
            data_vase: gates.data_vase,
            dir_hash: gates.dir_hash,
            dir_hash_vase: gates.dir_hash_vase,
            eval_vase,
            label_vase,
            mint_gun: gates.mint_gun,
            slat,
            swet_gate: gates.swet_gate,
            swet_gun,
            shot,
            shot_gun: gates.shot_gun,
            standard_output: gates.standard_output,
            value_trap_arbitrary,
            value_trap_standard,
        })
    }

    fn compile_wrapper_gate(&mut self, name: &str, source: &str) -> Result<Noun> {
        let wrapper_dbug = self.dbug && name != "constant-vase" && name != "data-vase";
        let wer = if wrapper_dbug {
            hoonc_wrapper_wer()
        } else {
            vec!["native".to_string(), "wrappers".to_string(), name.to_string()]
        };
        let expr = parse_inline_wrapper(name, source, wer, wrapper_dbug)?;
        let (_ty, formula) = self.mint_with_subject_type(self.wrapper_subject_ty, &expr)?;
        let wrapper_subject_value = self.wrapper_subject_value;
        let wrapper_subject_space = self.eval_context.stack.noun_space();
        let slab_space = self.ut.slab.noun_space();
        let label = format!("wrapper {name}");
        let gate = eval_formula_noun_in_context(
            &mut self.eval_context,
            formula,
            &slab_space,
            &label,
            |stack| copy_noun_to_allocator(stack, wrapper_subject_value, &wrapper_subject_space),
        )?;
        let eval_space = self.eval_context.stack.noun_space();
        Ok(copy_noun_to_allocator(
            &mut *self.ut.slab, gate, &eval_space,
        ))
    }

    fn compile_wrapper_expression(&mut self, name: &str, source: &str) -> Result<Noun> {
        let wrapper_dbug = self.dbug;
        let wer = if wrapper_dbug {
            hoonc_wrapper_wer()
        } else {
            vec!["native".to_string(), "wrappers".to_string(), name.to_string()]
        };
        let expr = parse_inline_wrapper(name, source, wer, wrapper_dbug)?;
        let (_ty, formula) = self.mint_with_subject_type(self.wrapper_subject_ty, &expr)?;
        let wrapper_subject_value = self.wrapper_subject_value;
        let wrapper_subject_space = self.eval_context.stack.noun_space();
        let slab_space = self.ut.slab.noun_space();
        let label = format!("wrapper {name}");
        let value = eval_formula_noun_in_context(
            &mut self.eval_context,
            formula,
            &slab_space,
            &label,
            |stack| copy_noun_to_allocator(stack, wrapper_subject_value, &wrapper_subject_space),
        )?;
        let eval_space = self.eval_context.stack.noun_space();
        Ok(copy_noun_to_allocator(
            &mut *self.ut.slab, value, &eval_space,
        ))
    }

    fn compile_wrapper_gate_exact(
        &mut self,
        name: &str,
        source: &str,
        subject_trap: Noun,
    ) -> Result<Noun> {
        let wrapper_dbug = self.dbug && name != "constant-vase" && name != "data-vase";
        let wer = if wrapper_dbug {
            hoonc_wrapper_wer()
        } else {
            vec!["native".to_string(), "wrappers".to_string(), name.to_string()]
        };
        let expr = parse_inline_wrapper(name, source, wer, wrapper_dbug)?;
        let (_ty, formula) = self.exact_mint_gun(subject_trap, &expr, name)?;
        let wrapper_subject_value = self.wrapper_subject_value;
        let wrapper_subject_space = self.eval_context.stack.noun_space();
        let slab_space = self.ut.slab.noun_space();
        let label = format!("wrapper {name}");
        let gate = eval_formula_noun_in_context(
            &mut self.eval_context,
            formula,
            &slab_space,
            &label,
            |stack| copy_noun_to_allocator(stack, wrapper_subject_value, &wrapper_subject_space),
        )?;
        let eval_space = self.eval_context.stack.noun_space();
        Ok(copy_noun_to_allocator(
            &mut *self.ut.slab, gate, &eval_space,
        ))
    }

    fn compile_entry(&mut self, path: &Path) -> Result<NativeBuildProduct> {
        // Entry/kernel compiles always use per-call `miss` memos, never the
        // cross-call persistent one. Persisting `miss` verdicts is only sound
        // while compiling the isolated prelude (a fixed source in a fresh Ut);
        // during a kernel compile the mutable state `miss` reads (rest/redo/
        // nest caches) drifts and memoized verdicts flip, miscompiling as
        // `redo-match` — see Ut::miss. This surfaced as honk failing to
        // compile the roswell test kernel (`test-h-map-dif-large-against-small`)
        // even single-entry; the per-call memo already prevents the >10^8
        // recursion blowup, so dropping cross-call persistence costs no real
        // time (all six kernels stay byte-exact and compile in <60s).
        self.ut.set_miss_memo_persistence(false);
        let canonical = path.canonicalize()?;
        match self.compile_path_uncached(path, true, false, false, true, true)? {
            NativeCompileOutput::Product(product) => Ok(product),
            NativeCompileOutput::Vase(_) => Err(format!(
                "missing top-level native product for {}",
                canonical.display()
            )
            .into()),
        }
    }

    fn subject_vase_from_cached(vase: &NativeVase) -> SubjectVase {
        SubjectVase {
            ty: vase.ty,
            eval_value: vase.eval_value,
            trap: vase.trap,
        }
    }

    fn compile_path(&mut self, path: &Path, need_eval: bool) -> Result<SubjectVase> {
        let canonical = path.canonicalize()?;
        let content_key = hoon_source_content_key(path)?;
        let cache_key = canonical.clone();
        let content_cache_key = content_key;
        if let Some(cached) = self.cache.get(&cache_key) {
            let cached = cached.clone();
            if need_eval && cached.eval_value.is_none() {
                // Recompile with evaluation if an earlier deferred-only pass cached the trap.
            } else {
                return Ok(Self::subject_vase_from_cached(&cached));
            }
        }
        if let Some(cached) = self.content_cache.get(&content_cache_key) {
            let cached = cached.clone();
            if need_eval && cached.eval_value.is_none() {
                // Recompile with evaluation if an earlier deferred-only pass cached the trap.
            } else {
                self.cache.insert(cache_key.clone(), cached.clone());
                return Ok(Self::subject_vase_from_cached(&cached));
            }
        }
        if need_eval {
            self.cache.remove(&cache_key);
        } else if let Some(cached) = self.cache.get(&cache_key) {
            let cached = cached.clone();
            return Ok(Self::subject_vase_from_cached(&cached));
        }
        if !self.visiting.insert(canonical.clone()) {
            return Err(format!("cyclic native dependency at {}", canonical.display()).into());
        }

        let result = self.compile_path_uncached(path, false, need_eval, false, false, false);
        self.visiting.remove(&canonical);
        let subject_vase = match result? {
            NativeCompileOutput::Vase(vase) => vase,
            NativeCompileOutput::Product(product) => SubjectVase {
                ty: product.ty,
                eval_value: product.eval_value,
                trap: product
                    .vase_trap
                    .ok_or_else(|| "dependency product did not retain a trap".to_string())?,
            },
        };
        let native_vase = NativeVase {
            ty: subject_vase.ty,
            eval_value: subject_vase.eval_value,
            trap: subject_vase.trap,
        };
        self.cache.insert(cache_key, native_vase.clone());
        self.content_cache.insert(content_cache_key, native_vase);
        Ok(subject_vase)
    }

    #[allow(clippy::too_many_arguments)]
    fn compile_path_uncached(
        &mut self,
        path: &Path,
        keep_product: bool,
        evaluate_value: bool,
        needs_subject: bool,
        canonical_entry_dbug: bool,
        absolute_entry_wer: bool,
    ) -> Result<NativeCompileOutput> {
        let _compile_log = TimedHoonPathLog::new(path, HoonLogOperation::Compile);
        trace_native(format!("compiling {}", path.display()));
        let imports = trace_timed(format!("resolving imports {}", path.display()), || {
            Ok(pipeline::resolve_native_imports(
                path,
                &self.directory,
                ScopeMode::Standard,
            )?)
        })?;
        let mut imported_vases = Vec::new();
        let imports_need_eval = evaluate_value || needs_subject;
        for import in imports {
            let imported = match import.kind {
                NativeImportKind::Hoon => self.compile_path(&import.path, imports_need_eval)?,
                NativeImportKind::Data => self.data_vase(&import.path, imports_need_eval)?,
            };
            let imported = match import.face.as_deref() {
                Some(face) => self.label_vase(&imported, face)?,
                None => imported,
            };
            imported_vases.push(imported);
        }
        let label = format!("{}", path.display());
        let subject_ty = trace_timed(format!("assembling subject type {label}"), || {
            Ok(self.subject_type(&imported_vases))
        })?;
        let expr = trace_timed(format!("parsing leaf {label}"), || {
            parse_build_leaf(
                path, &self.directory, canonical_entry_dbug, absolute_entry_wer, self.dbug,
            )
        })?;
        let override_value = native_value_override(&mut self.eval_context, path, &self.directory)?;
        let subject_trap = if override_value.is_none() {
            Some(self.subject_trap(&imported_vases)?)
        } else {
            None
        };
        // hoonc vets every file it compiles (`vet=&` is the ++ut door default
        // in hoonc.hoon's build chain), not just the entry — align.
        let vet = self.entry_vet;
        let (ty, formula, vase_trap) = match override_value {
            Some(value) => {
                let (ty, formula) = trace_timed(format!("minting {label}"), || {
                    self.mint_with_subject_type_vet(subject_ty, &expr, vet)
                })?;
                let vase_trap = self.eval_vase_trap(ty, value)?;
                (ty, formula, vase_trap)
            }
            None => {
                let subject_trap = subject_trap.expect("subject trap should be present");
                trace_timed(format!("swetting {label}"), || {
                    self.native_swet_vase_trap(subject_ty, subject_trap, &expr, vet)
                })?
            }
        };
        if keep_product {
            let mut standard_jam = None;
            let eval_value = if evaluate_value {
                match override_value {
                    Some(value) => {
                        standard_jam = Some(self.jam_standard_gate_value(value, path)?);
                        None
                    }
                    None => {
                        self.reset_eval_memo_if_needed(path);
                        standard_jam = Some(self.eval_standard_formula_to_jam(
                            &imported_vases, formula, path, &label,
                        )?);
                        None
                    }
                }
            } else {
                None
            };
            let product = NativeBuildProduct {
                ty,
                eval_value,
                formula,
                vase_trap: Some(vase_trap),
                standard_jam,
            };
            Ok(NativeCompileOutput::Product(product))
        } else {
            let vase = if let Some(eval_value) = override_value {
                let eval_space = self.eval_context.stack.noun_space();
                let eval_value =
                    copy_noun_to_allocator(&mut *self.ut.slab, eval_value, &eval_space);
                SubjectVase {
                    ty,
                    eval_value: Some(eval_value),
                    trap: vase_trap,
                }
            } else {
                let eval_value = if evaluate_value {
                    self.reset_eval_memo_if_needed(path);
                    let value = self.eval_formula_with_subject(&imported_vases, formula, &label)?;
                    let value_space = self.eval_context.stack.noun_space();
                    Some(copy_noun_to_allocator(
                        &mut *self.ut.slab, value, &value_space,
                    ))
                } else {
                    None
                };
                SubjectVase {
                    ty,
                    eval_value,
                    trap: vase_trap,
                }
            };
            Ok(NativeCompileOutput::Vase(vase))
        }
    }

    fn mint_with_subject_type(&mut self, subject_ty: Noun, expr: &Hoon) -> Result<(Noun, Noun)> {
        let vet = self.entry_vet;
        self.mint_with_subject_type_vet(subject_ty, expr, vet)
    }

    fn mint_with_subject_type_vet(
        &mut self,
        subject_ty: Noun,
        expr: &Hoon,
        vet: bool,
    ) -> Result<(Noun, Noun)> {
        let sut = subject_ty;
        self.mint_with_sut(sut, expr, vet)
    }

    fn mint_with_sut(&mut self, sut: Noun, expr: &Hoon, vet: bool) -> Result<(Noun, Noun)> {
        let gol = ty_noun(&mut *self.ut.slab);
        let ut = &mut self.ut;
        let eval_context = &mut self.eval_context;
        let saved_context = eval_context.save();
        let result = unsafe {
            eval_context.with_transient_stack_frame(0, |_context| {
                ut.clear_build_transients();
                ut.set_vet(vet);
                ut.mint_noun(sut, gol, expr)
            })
        };
        eval_context.restore(&saved_context);
        let (ty, formula) = result?;
        // Native-types migration Phase 1: flag-gated IR-completeness invariant on
        // every app-level minted formula (kernel arms etc.) — distinct shapes
        // from the prelude. Default-off (HONK_IR_ROUNDTRIP).
        if env::var_os("HONK_IR_ROUNDTRIP").is_some() {
            let space = self.ut.slab.noun_space();
            honk::native::ir::roundtrip_check(formula, &space)?;
        }
        Ok((ty, formula))
    }

    fn data_vase(&mut self, path: &Path, need_eval: bool) -> Result<SubjectVase> {
        let bytes = fs::read(path)?;

        // Hoonc stores data leaves as `[(met 3 fil) fil]`: the byte-width of
        // the atom, not the filesystem byte count (trailing zero bytes do not
        // contribute to `p.octs`).
        let octet_len = bytes
            .iter()
            .rposition(|byte| *byte != 0)
            .map_or(0usize, |idx| idx + 1);
        let eval_len = noun_u64(&mut self.eval_context.stack, octet_len as u64);
        let eval_atom = Atom::from_value(&mut self.eval_context.stack, bytes.as_slice())?.as_noun();
        let eval_value = T(&mut self.eval_context.stack, &[eval_len, eval_atom]);

        let (ty, trap) = if let Some(ty) = self.canonical_data_octs_ty {
            // Hoonc's non-Hoon graph leaves are vased as the `$octs` arm in
            // hoonc.hoon itself.  That arm's type is a `%hold` over the hoonc
            // compiler core, so the canonical hoon-138 noun is not equivalent
            // to a freshly-defined local `[p=@ud q=@]` alias even though the
            // value shape is the same.
            let eval_space = self.eval_context.stack.noun_space();
            let trap = hoonc_data_octs_vase_trap(&mut *self.ut.slab, ty, eval_value, &eval_space);
            (ty, trap)
        } else {
            let ty = local_octs_type(&mut *self.ut.slab);
            let slab_space = self.ut.slab.noun_space();
            let eval_space = self.eval_context.stack.noun_space();
            let vase = vase_native_with_spaces(
                &mut *self.ut.slab, ty, &slab_space, eval_value, &eval_space,
            );
            let trap = self.trap_from_payload(self.wrappers.data_vase, vase);
            (ty, trap)
        };
        let eval_value = if need_eval {
            let eval_space = self.eval_context.stack.noun_space();
            Some(copy_noun_to_allocator(
                &mut *self.ut.slab, eval_value, &eval_space,
            ))
        } else {
            None
        };
        Ok(SubjectVase {
            ty,
            eval_value,
            trap,
        })
    }

    fn label_vase(&mut self, vase: &SubjectVase, face: &str) -> Result<SubjectVase> {
        let ty = ty_face_local(&mut *self.ut.slab, face, vase.ty);
        let face = term_to_noun(&mut *self.ut.slab, face);
        let payload = T(&mut *self.ut.slab, &[vase.trap, face]);
        let trap = self.trap_from_payload(self.wrappers.label_vase, payload);
        Ok(SubjectVase {
            ty,
            eval_value: vase.eval_value,
            trap,
        })
    }

    fn subject_type(&mut self, imports: &[SubjectVase]) -> Noun {
        let zero = D(0);
        let mut deps_ty = ty_atom_local(&mut *self.ut.slab, "n", Some(zero));
        for import in imports {
            deps_ty = ty_cell_local(&mut *self.ut.slab, deps_ty, import.ty);
        }
        ty_cell_local(&mut *self.ut.slab, deps_ty, self.prelude_vase.ty)
    }

    fn subject_trap(&mut self, imports: &[SubjectVase]) -> Result<Noun> {
        let mut deps_trap = self.empty_trap_vase;
        for import in imports {
            deps_trap = self.slat_vase_trap(deps_trap, import.trap)?;
        }
        self.slat_vase_trap(deps_trap, self.prelude_vase.trap)
    }

    fn eval_vase_trap(&mut self, ty: Noun, value: Noun) -> Result<Noun> {
        let slab_space = self.ut.slab.noun_space();
        let eval_space = self.eval_context.stack.noun_space();
        let vase = vase_native_with_spaces(&mut *self.ut.slab, ty, &slab_space, value, &eval_space);
        Ok(self.trap_from_payload(self.wrappers.eval_vase, vase))
    }

    fn slat_vase_trap(&mut self, hed: Noun, tal: Noun) -> Result<Noun> {
        let sample = T(&mut *self.ut.slab, &[hed, tal]);
        Ok(self.trap_from_payload(self.wrappers.slat, sample))
    }

    fn native_swet_vase_trap(
        &mut self,
        subject_ty: Noun,
        tap: Noun,
        expr: &Hoon,
        vet: bool,
    ) -> Result<(Noun, Noun, Noun)> {
        let (ty, formula) = self.mint_with_subject_type_vet(subject_ty, expr, vet)?;
        let gun = T(&mut *self.ut.slab, &[ty, formula]);
        let payload = T(&mut *self.ut.slab, &[gun, tap]);
        let trap = self.trap_from_payload(self.wrappers.swet_gun, payload);
        Ok((ty, formula, trap))
    }

    fn exact_mint_gun(&mut self, tap: Noun, expr: &Hoon, label: &str) -> Result<(Noun, Noun)> {
        let gen = hoon_to_noun(&mut *self.ut.slab, expr);
        let payload = T(&mut *self.ut.slab, &[tap, gen]);
        let gun = self.slam_wrapper_gate(self.wrappers.mint_gun, payload, label)?;
        let space = self.ut.slab.noun_space();
        let gun = gun
            .in_space(&space)
            .as_cell()
            .map_err(|err| format!("exact mint-gun did not produce a vase: {err:?}"))?;
        Ok((gun.head().noun(), gun.tail().noun()))
    }

    fn shot_vase_trap(
        &mut self,
        gate_ty: Noun,
        gate_trap: Noun,
        sample_ty: Noun,
        sample_trap: Noun,
    ) -> Result<Noun> {
        let sut = ty_cell_local(&mut *self.ut.slab, gate_ty, sample_ty);
        let entry_vet = self.entry_vet;
        let (shot_ty, _shot_formula) = self.mint_with_sut(sut, &shot_gene(), entry_vet)?;
        let sample = T(&mut *self.ut.slab, &[gate_trap, sample_trap]);
        let payload = T(&mut *self.ut.slab, &[shot_ty, sample]);
        Ok(self.trap_from_payload(self.wrappers.shot, payload))
    }

    fn dir_hash_vase_trap(&mut self, value: Noun) -> Result<Noun> {
        let sample_ty = ty_atom_local(&mut *self.ut.slab, "uvI", None);
        let vase = vase_native(&mut *self.ut.slab, sample_ty, value);
        Ok(self.trap_from_payload(self.wrappers.dir_hash_vase, vase))
    }

    fn standard_output_trap(
        &mut self,
        gate_ty: Noun,
        gate_trap: Noun,
        dir_hash: u32,
    ) -> Result<Noun> {
        let dir_hash = noun_u64(&mut *self.ut.slab, dir_hash as u64);
        let sample_trap = self.dir_hash_vase_trap(dir_hash)?;
        let sample_ty = ty_atom_local(&mut *self.ut.slab, "uvI", None);
        let shot_trap = self.shot_vase_trap(gate_ty, gate_trap, sample_ty, sample_trap)?;
        Ok(self.trap_from_payload(self.wrappers.value_trap_standard, shot_trap))
    }

    fn exact_directory_mug(
        &mut self,
        entry: &Path,
        directory_files: Option<&[PathBuf]>,
    ) -> Result<u32> {
        directory_mug_with_files(entry, &self.directory, directory_files)
    }

    fn value_trap(&mut self, vase_trap: Noun, mode: CompileMode) -> Result<Noun> {
        let battery = match mode {
            CompileMode::Arbitrary => self.wrappers.value_trap_arbitrary,
            _ => self.wrappers.value_trap_standard,
        };
        Ok(self.trap_from_payload(battery, vase_trap))
    }

    fn trap_from_payload(&mut self, battery: Noun, payload: Noun) -> Noun {
        T(&mut *self.ut.slab, &[battery, payload])
    }

    fn slam_wrapper_gate(&mut self, gate: Noun, sample: Noun, label: &str) -> Result<Noun> {
        let eval_context = &mut self.eval_context;
        let slab = &mut *self.ut.slab;
        let slab_space = slab.noun_space();
        catch_nock_panic(format!("slamming exact wrapper {label}"), || {
            let interpreted = unsafe {
                eval_context.with_stack_frame(
                    0,
                    |context| -> std::result::Result<Noun, NockError> {
                        let gate = copy_noun_to_allocator(&mut context.stack, gate, &slab_space);
                        let sample =
                            copy_noun_to_allocator(&mut context.stack, sample, &slab_space);
                        let subject = T(&mut context.stack, &[gate, sample]);
                        let slam = slam_gate_formula(&mut context.stack);
                        let eval_start = Instant::now();
                        let interpreted = interpret(context, subject, slam);
                        let elapsed = eval_start.elapsed();
                        add_interpret_timing(elapsed);
                        debug!(
                            label = label,
                            reason = "slamming exact wrapper gate with sample",
                            elapsed_ms = elapsed.as_secs_f64() * 1_000.0,
                            success = interpreted.is_ok(),
                            "NockStack eval finished"
                        );
                        if let Err(err) = &interpreted {
                            if env::var_os("NATIVE_HOON_TRACE").is_some() {
                                trace_interpret_error(context, err);
                            }
                        }
                        interpreted
                    },
                )
            };
            let value =
                interpreted.map_err(|err| format!("interpret exact wrapper {label}: {err:?}"))?;
            let eval_space = eval_context.stack.noun_space();
            Ok(copy_noun_to_allocator(slab, value, &eval_space))
        })
    }

    fn eval_subject_value(
        stack: &mut NockStack,
        imports: &[SubjectVase],
        import_space: &NounSpace,
        prelude_value: Noun,
    ) -> Noun {
        // `prelude_value` is either direct atom zero (cold-state builds) or a long-lived noun in
        // this evaluation context below the current frame.  Do not copy the full prelude core for
        // every leaf.
        let mut deps_value = D(0);
        for (idx, import) in imports.iter().enumerate() {
            let eval_value = import
                .eval_value
                .unwrap_or_else(|| panic!("dependency {idx} did not retain an evaluated value"));
            let eval_value = stack.copy_into(eval_value, import_space);
            deps_value = T(stack, &[deps_value, eval_value]);
        }
        T(stack, &[deps_value, prelude_value])
    }

    fn ensure_subject_values(imports: &[SubjectVase]) -> Result<()> {
        for (idx, import) in imports.iter().enumerate() {
            if import.eval_value.is_none() {
                return Err(format!("dependency {idx} did not retain an evaluated value").into());
            }
        }
        Ok(())
    }

    fn eval_formula_with_subject(
        &mut self,
        imports: &[SubjectVase],
        formula: Noun,
        label: &str,
    ) -> Result<Noun> {
        Self::ensure_subject_values(imports)?;
        let prelude_value = self.prelude_eval_value;
        let formula_space = self.ut.slab.noun_space();
        trace_timed(format!("evaluating {label}"), || {
            eval_formula_noun_in_context(
                &mut self.eval_context,
                formula,
                &formula_space,
                label,
                |stack| Self::eval_subject_value(stack, imports, &formula_space, prelude_value),
            )
        })
    }

    fn eval_standard_formula_to_jam(
        &mut self,
        imports: &[SubjectVase],
        formula: Noun,
        entry: &Path,
        label: &str,
    ) -> Result<Vec<u8>> {
        Self::ensure_subject_values(imports)?;
        let dir_hash = self.exact_directory_mug(entry, None)?;
        let prelude_value = self.prelude_eval_value;
        let formula_space = self.ut.slab.noun_space();
        let product_trap = trace_timed(format!("evaluating {label}"), || {
            catch_nock_panic(format!("evaluating and jamming {label}"), || unsafe {
                let mut product_trap = D(0);
                let eval_context = &mut self.eval_context;
                let slab = &mut *self.ut.slab;
                let result: std::result::Result<(), String> = eval_context
                    .with_transient_stack_frame(0, |context| {
                        let snapshot = context.save();
                        let result: std::result::Result<(), NockError> = (|| {
                            let trace = env::var_os("NATIVE_HOON_TRACE").is_some();
                            let start = Instant::now();
                            let subject = Self::eval_subject_value(
                                &mut context.stack,
                                imports,
                                &formula_space,
                                prelude_value,
                            );
                            if trace {
                                eprintln!(
                                    "[honk] eval {label}: assembled subject {:.3}s",
                                    start.elapsed().as_secs_f64()
                                );
                            }

                            let start = Instant::now();
                            let formula =
                                copy_noun_to_allocator(&mut context.stack, formula, &formula_space);
                            if trace {
                                eprintln!(
                                    "[honk] eval {label}: copied formula {:.3}s",
                                    start.elapsed().as_secs_f64()
                                );
                            }

                            let start = Instant::now();
                            let gate_result = interpret(context, subject, formula);
                            let elapsed = start.elapsed();
                            add_interpret_timing(elapsed);
                            debug!(
                                label = label,
                                reason = "evaluating compiled Hoon formula to produce the standard kernel gate",
                                elapsed_ms = elapsed.as_secs_f64() * 1_000.0,
                                success = gate_result.is_ok(),
                                "NockStack eval finished"
                            );
                            let gate = gate_result?;
                            if trace {
                                eprintln!(
                                    "[honk] eval {label}: interpreted {:.3}s",
                                    elapsed.as_secs_f64()
                                );
                            }

                            let start = Instant::now();
                            let product = slam_gate_with_atom_sample_noun_in_frame(
                                context, gate, dir_hash as u64,
                            )?;
                            if trace {
                                eprintln!(
                                    "[honk] done slamming standard kernel gate {:.3}s",
                                    start.elapsed().as_secs_f64()
                                );
                            }

                            let start = Instant::now();
                            let context_space = context.stack.noun_space();
                            product_trap = copy_noun_to_allocator(slab, product, &context_space);
                            if trace {
                                eprintln!(
                                    "[honk] copied standard kernel product trap {:.3}s",
                                    start.elapsed().as_secs_f64()
                                );
                            }
                            Ok(())
                        })(
                        );
                        if let Err(err) = &result {
                            if env::var_os("NATIVE_HOON_TRACE").is_some() {
                                trace_interpret_error(context, err);
                            }
                        }
                        context.restore(&snapshot);
                        result.map_err(|err| format!("interpret formula: {err:?}"))
                    });
                if let Err(err) = result {
                    return Err(err.into());
                }
                Ok(product_trap)
            })
        })?;
        self.jam_vase_trap_value(product_trap, CompileMode::Standard)
    }

    fn jam_standard_gate_value(&mut self, gate: Noun, entry: &Path) -> Result<Vec<u8>> {
        let dir_hash = self.exact_directory_mug(entry, None)?;
        jam_standard_kernel_trap_native(&mut self.eval_context, gate, dir_hash)
    }

    fn reset_eval_memo_if_needed(&mut self, path: &Path) {
        if should_reset_eval_memo(path) {
            trace_native(format!(
                "resetting Nock memo cache before {}",
                path.display()
            ));
            self.eval_context.cache = Hamt::new(&mut self.eval_context.stack);
        }
    }

    fn jam_product(
        &mut self,
        product: &mut NativeBuildProduct,
        mode: CompileMode,
        entry: &Path,
        directory_files: Option<&[PathBuf]>,
    ) -> Result<Vec<u8>> {
        match mode {
            CompileMode::Dynock => Ok(jam_dynock_output_native(
                &mut self.ut, product.ty, product.formula, false,
            )),
            CompileMode::DynockTyped => Ok(jam_dynock_output_native(
                &mut self.ut, product.ty, product.formula, true,
            )),
            CompileMode::Arbitrary => {
                let vase_trap = product.vase_trap.ok_or_else(|| {
                    format!(
                        "arbitrary output for {} did not retain a vase trap",
                        entry.display()
                    )
                })?;
                self.jam_vase_trap_value(vase_trap, mode)
            }
            CompileMode::Standard => {
                if let Some(jam) = product.standard_jam.take() {
                    return Ok(jam);
                }
                let dir_hash = self.exact_directory_mug(entry, directory_files)?;
                let vase_trap = product.vase_trap.ok_or_else(|| {
                    format!(
                        "standard output for {} did not retain a vase trap",
                        entry.display()
                    )
                })?;
                self.jam_deferred_standard_kernel_vase_trap(product.ty, vase_trap, dir_hash, mode)
                    .map_err(|err| {
                        format!(
                            "standard output for {} failed to build deferred trap: {err}",
                            entry.display()
                        )
                        .into()
                    })
            }
        }
    }

    fn jam_vase_trap_value(&mut self, vase_trap: Noun, mode: CompileMode) -> Result<Vec<u8>> {
        let trap = self.value_trap(vase_trap, mode)?;
        Ok(jam_ut_noun(&mut self.ut, trap))
    }

    fn jam_deferred_standard_kernel_vase_trap(
        &mut self,
        gate_ty: Noun,
        gate_trap: Noun,
        dir_hash: u32,
        _mode: CompileMode,
    ) -> Result<Vec<u8>> {
        let trap = self.standard_output_trap(gate_ty, gate_trap, dir_hash)?;
        Ok(jam_ut_noun(&mut self.ut, trap))
    }
}

enum NativeCompileOutput {
    Vase(SubjectVase),
    Product(NativeBuildProduct),
}

fn dump_exact_wrapper_assets(builder: &mut NativeBuildContext<'_>, out_dir: &Path) -> Result<()> {
    fs::create_dir_all(out_dir)?;

    let data_vase_battery = {
        let octs = T(&mut *builder.ut.slab, &[D(0), D(0)]);
        let trap = builder.slam_wrapper_gate(builder.wrappers.data_vase, octs, "data-vase")?;
        let space = builder.ut.slab.noun_space();
        trap_battery(trap, &space)?
    };
    let dir_hash_vase_battery = {
        let trap =
            builder.slam_wrapper_gate(builder.wrappers.dir_hash_vase, D(0), "dir-hash-vase")?;
        let space = builder.ut.slab.noun_space();
        trap_battery(trap, &space)?
    };

    let assets = [
        ("constant-vase-battery.jam", builder.wrappers.constant_vase),
        ("data-vase-battery.jam", data_vase_battery),
        ("dir-hash-vase-battery.jam", dir_hash_vase_battery),
        ("eval-vase-battery.jam", builder.wrappers.eval_vase),
        ("label-vase-battery.jam", builder.wrappers.label_vase),
        ("slat-battery.jam", builder.wrappers.slat),
        ("shot-battery.jam", builder.wrappers.shot),
        ("swet-gun-battery.jam", builder.wrappers.swet_gun),
        (
            "value-trap-arbitrary-battery.jam", builder.wrappers.value_trap_arbitrary,
        ),
        (
            "value-trap-standard-battery.jam", builder.wrappers.value_trap_standard,
        ),
        ("empty-trap-vase.jam", builder.empty_trap_vase),
    ];

    let wrapper_space = builder.ut.slab.noun_space();
    for (file_name, noun) in assets {
        fs::write(
            out_dir.join(file_name),
            jam_noun_in_fresh_slab(noun, &wrapper_space),
        )?;
    }

    let cold = builder
        .eval_context
        .cold
        .into_noun(&mut builder.eval_context.stack);
    let cold_space = builder.eval_context.stack.noun_space();
    fs::write(
        out_dir.join("honc-cold-138.jam"),
        jam_noun_in_fresh_slab(cold, &cold_space),
    )?;
    Ok(())
}

fn dump_native_wrapper_assets(builder: &mut NativeBuildContext<'_>, out_dir: &Path) -> Result<()> {
    fs::create_dir_all(out_dir)?;
    let assets = [
        ("constant-vase-battery.jam", builder.wrappers.constant_vase),
        ("data-vase-battery.jam", builder.wrappers.data_vase),
        ("dir-hash-vase-battery.jam", builder.wrappers.dir_hash_vase),
        ("eval-vase-battery.jam", builder.wrappers.eval_vase),
        ("label-vase-battery.jam", builder.wrappers.label_vase),
        ("slat-battery.jam", builder.wrappers.slat),
        ("shot-battery.jam", builder.wrappers.shot),
        ("swet-gun-battery.jam", builder.wrappers.swet_gun),
        (
            "value-trap-arbitrary-battery.jam", builder.wrappers.value_trap_arbitrary,
        ),
        (
            "value-trap-standard-battery.jam", builder.wrappers.value_trap_standard,
        ),
        ("empty-trap-vase.jam", builder.empty_trap_vase),
    ];
    let wrapper_space = builder.ut.slab.noun_space();
    for (file_name, noun) in assets {
        fs::write(
            out_dir.join(file_name),
            jam_noun_in_fresh_slab(noun, &wrapper_space),
        )?;
    }
    Ok(())
}

fn jam_noun_in_fresh_slab(noun: Noun, space: &NounSpace) -> Vec<u8> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = slab.copy_into(noun, space);
    jam_slab_noun(&mut slab, noun)
}

fn should_reset_eval_memo(_path: &Path) -> bool {
    false
}

fn shot_gene() -> Hoon {
    Hoon::CenSig(
        vec![Limb::Term("$".to_string())],
        Box::new(Hoon::Axis(2)),
        vec![Hoon::Axis(3)],
    )
}

fn seed_honc_type_with_ut(
    ut: &mut Ut<'_>,
    _context: &mut Context,
    prelude: &Hoon,
    skip_play: bool,
) -> Result<NativeVase> {
    let sut = empty_subject_type(&mut *ut.slab);
    ut.set_vet(false);
    ut.set_miss_memo_persistence(true);
    ut.exact_hoon_ast_lookup_enabled = true;
    // The full-prelude play is an O(N^2) traversal whose only product is the
    // returned type. When the caller will overwrite that type with a cued
    // subject type (embedded / --sut-jam), skip it — it is then pure waste
    // (verified byte-identical kernel output, ~5s faster per build). The
    // native-parity path (no override) still needs the real play.
    let ty = if skip_play {
        sut
    } else {
        // ATOMIC FLIP (C-final.2): play takes a native subject. This boundary
        // still holds a noun `sut` (empty_subject_type) and wants a noun type, so
        // route through the noun-in/noun-out play_noun bridge.
        ut.play_noun(sut, prelude)?
    };
    ut.set_miss_memo_persistence(false);
    Ok(NativeVase {
        ty,
        eval_value: None,
        trap: D(0),
    })
}

/// Chunked native mint of the canonical hoon-138 prelude (`=< ride => %138 =>
/// |% … => |% …`), for the HONK_NATIVE_PARITY path. `=< ride stdlib` is `=>
/// stdlib ride`; the whole-prelude goal is `%noun`, so every layer mints with
/// goal `%noun`. Each top-level layer (and finally `ride`) is minted in its OWN
/// cold-loaded `Ut`/working slab, carrying the subject type in a ping-ponged
/// slab and accumulating per-layer formulas in `out_slab`, dropping each working
/// slab — bounding peak memory to one layer + the current subject + the
/// formulas. Composition mirrors `mint_tsgr` exactly (validated byte-exact by
/// `chunked_tisgar_chain_matches_monolithic_mint`).
/// Peel transparent wrappers the parser adds around the prelude (a single-
/// element `=~`/TisSig, and `Dbug`/`Note` spot/hint wrappers) to reach the
/// underlying compose node. NOTE: this is for NAVIGATION only — minting the
/// peeled layers loses the outer `Dbug` location stack, so chunked output is
/// NOT byte-exact under dbug=true yet (spot preservation is follow-on work);
/// it is correct for measuring the memory trajectory and for dbug=false.
fn peel_transparent(mut hoon: &Hoon) -> &Hoon {
    loop {
        hoon = match hoon {
            Hoon::TisSig(list) if list.len() == 1 => &list[0],
            Hoon::Dbug(_, inner) => inner.as_ref(),
            Hoon::Note(_, inner) => inner.as_ref(),
            other => return other,
        };
    }
}

fn prelude_variant_name(hoon: &Hoon) -> &'static str {
    match hoon {
        Hoon::TisGal(_, _) => "TisGal(=<)",
        Hoon::TisGar(_, _) => "TisGar(=>)",
        Hoon::Dbug(_, _) => "Dbug",
        Hoon::Note(_, _) => "Note",
        Hoon::CenDot(_, _) => "CenDot",
        Hoon::CenHep(_, _) => "CenHep",
        Hoon::CenCol(_, _) => "CenCol",
        Hoon::CenSig(_, _, _) => "CenSig",
        Hoon::KetSig(_) => "KetSig",
        Hoon::KetBar(_) => "KetBar",
        Hoon::KetDot(_, _) => "KetDot",
        Hoon::BarCen(_, _) => "BarCen(|%)",
        Hoon::TisCom(_, _) => "TisCom",
        Hoon::TisHep(_, _) => "TisHep",
        Hoon::TisLus(_, _) => "TisLus",
        Hoon::Pair(_, _) => "Pair",
        _ => "other",
    }
}

fn mint_honc_prelude_chunked(
    out_slab: &mut NounSlab<NockJammer>,
    prelude: &Hoon,
) -> Result<(Noun, Noun)> {
    let Hoon::TisGal(ride, stdlib) = peel_transparent(prelude) else {
        return Err("chunked prelude mint: root is not =<".into());
    };
    let mut layers: Vec<&Hoon> = Vec::new();
    let mut cur: &Hoon = peel_transparent(stdlib);
    while let Hoon::TisGar(p, q) = cur {
        layers.push(p.as_ref());
        cur = peel_transparent(q);
    }
    layers.push(cur);
    eprintln!(
        "[honk] chunked prelude: {} layers + ride; layer variants = [{}]",
        layers.len(),
        layers
            .iter()
            .map(|l| prelude_variant_name(l))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let mut subject_slab: NounSlab<NockJammer> = NounSlab::new();
    let mut subject = empty_subject_type(&mut subject_slab);
    let total = layers.len() + 1; // layers + ride
    let mut formulas: Vec<Noun> = Vec::with_capacity(total);
    // The ride (last) layer's product type IS the prelude's subject type — the
    // type the `--arbitrary` build mints against. Captured from `mint` (not
    // `play`) so its core coil seminouns are COMPLETE (`[%full ~]`), matching
    // hoonc; the play-seeded type left them blocked, which `++burp` then blanked,
    // diverging from hoonc's output by ~38%.
    let mut prelude_type_out: Option<Noun> = None;

    for (i, expr) in layers
        .iter()
        .copied()
        .chain(std::iter::once(ride.as_ref()))
        .enumerate()
    {
        let mut work: NounSlab<NockJammer> = NounSlab::new();
        let mut next_subject_slab: NounSlab<NockJammer> = NounSlab::new();
        let formula_out;
        {
            let mut ut = Ut::new(&mut work);
            ut.exact_hoon_ast_lookup_enabled = true;
            ut.load_musk_cold_state(EMBEDDED_HONC_COLD_138_JAM, "chunk layer")
                .map_err(|err| -> DynError { Box::new(err) })?;
            ut.set_vet(true);
            ut.set_miss_memo_persistence(true);
            let sub_in = ut.slab.copy_into(subject, &subject_slab.noun_space());
            let goal = ty_noun(&mut *ut.slab);
            let (ty, formula) = ut.mint_noun(sub_in, goal, expr)?;
            let ty = ut.burp_type(ty)?;
            let ut_space = ut.slab.noun_space();
            formula_out = out_slab.copy_into(formula, &ut_space);
            if i + 1 < total {
                subject = next_subject_slab.copy_into(ty, &ut_space);
            } else {
                prelude_type_out = Some(out_slab.copy_into(ty, &ut_space));
            }
        }
        formulas.push(formula_out);
        if i + 1 < total {
            subject_slab = next_subject_slab; // old subject slab dropped (reclaimed)
        }
        if env::var_os("NATIVE_HOON_TRACE").is_some() {
            eprintln!("[honk] chunked prelude: minted layer {}/{}", i + 1, total);
        }
    }

    // formulas = [layer0 … body, ride]. `=> stdlib ride` = comb(stdlib, ride);
    // stdlib chain folds right: comb(l0, comb(l1, … comb(l_{k-1}, body))).
    let ride_formula = formulas.pop().expect("ride formula");
    let mut stdlib_formula = formulas.pop().expect("stdlib body formula");
    while let Some(head) = formulas.pop() {
        stdlib_formula = comb(out_slab, head, stdlib_formula)?;
    }
    let formula = comb(out_slab, stdlib_formula, ride_formula)?;
    Ok((
        prelude_type_out.expect("chunked prelude ride type"),
        formula,
    ))
}

fn mint_honc_formula_with_ut(
    ut: &mut Ut<'_>,
    _context: &mut Context,
    prelude: &Hoon,
) -> Result<(Noun, Noun)> {
    // The canonical prelude is `=< …`; mint it chunked to bound memory (Step 2).
    let dbg = format!("{:?}", prelude);
    eprintln!(
        "[honk] mint_honc_formula path: prelude root = {} | debug = {}",
        prelude_variant_name(prelude),
        &dbg[..dbg.len().min(200)]
    );
    if std::env::var_os("NATIVE_HOON_NO_CHUNK").is_none()
        && matches!(peel_transparent(prelude), Hoon::TisGal(_, _))
    {
        return mint_honc_prelude_chunked(&mut *ut.slab, prelude);
    }
    let sut = empty_subject_type(&mut *ut.slab);
    let gol = ty_noun(&mut *ut.slab);
    ut.clear_build_memos();
    ut.set_vet(true);
    ut.set_miss_memo_persistence(true);
    let result = ut.mint_noun(sut, gol, prelude);
    ut.set_miss_memo_persistence(false);
    let (ty, formula) = result?;
    Ok((ty, formula))
}

fn evaluate_honc_isolated(
    context: &mut Context,
    prelude: &Hoon,
    formula_slab: &mut NounSlab<NockJammer>,
) -> Result<PreludeEval> {
    let mut slab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    {
        let subject = D(0);
        let sut = empty_subject_type(&mut *ut.slab);
        ut.exact_hoon_ast_lookup_enabled = true;
        ut.set_vet(false);
        ut.set_miss_memo_persistence(true);
        let _ = ut.play_noun(sut, prelude)?;

        let sut = empty_subject_type(&mut *ut.slab);
        let gol = ty_noun(&mut *ut.slab);
        ut.clear_build_memos();
        ut.set_vet(true);
        let (_ty, formula) = ut.mint_noun(sut, gol, prelude)?;
        ut.set_miss_memo_persistence(false);
        let formula_space = ut.slab.noun_space();
        let formula_copy = formula_slab.copy_into(formula, &formula_space);
        let value = eval_formula_noun_in_context(
            context,
            formula,
            &formula_space,
            "prelude subject",
            |_| subject,
        )?;
        Ok(PreludeEval {
            value,
            formula: formula_copy,
        })
    }
}

// honk cannot natively compile softed-constraints.hoon (it cues two large
// constraint jams and `soft`s them), so it substitutes the cued jam pair
// directly. That shortcut is valid ONLY for the exact source + jam content it
// was proven against; key it on content hashes rather than the bare filename,
// and hard-error on drift so a source/jam change cannot silently ship a stale
// substitution. Pins are blake3 hex of the corresponding files.
const SOFTED_CONSTRAINTS_SOURCE_B3: &str =
    "4fe81e9738b06b217b6fe13810dc23f650cc4b95d80d7671ac2392e6cb7e3c80";
const CONSTRAINTS_0_1_JAM_B3: &str =
    "8c18e6175059e1a2176c27a201268f0f283028e36a6ee9e2fc6a09f91b9bca29";
const CONSTRAINTS_2_JAM_B3: &str =
    "613afc7a8ff5b22dbe59edd99c9bbd4bcede474b5dec8596c3d8c907f89bac7f";

fn native_value_override(
    context: &mut Context,
    path: &Path,
    directory: &Path,
) -> Result<Option<Noun>> {
    if path.file_name().and_then(|name| name.to_str()) == Some("softed-constraints.hoon") {
        validate_softed_constraints_pins(path, directory)?;
        return Ok(Some(softed_constraints_value(context, directory)?));
    }
    Ok(None)
}

fn validate_softed_constraints_pins(path: &Path, directory: &Path) -> Result<()> {
    let checks = [
        (
            path.to_path_buf(),
            SOFTED_CONSTRAINTS_SOURCE_B3,
            "softed-constraints.hoon source",
        ),
        (
            directory.join("jams/constraints-0-1.jam"),
            CONSTRAINTS_0_1_JAM_B3,
            "jams/constraints-0-1.jam",
        ),
        (
            directory.join("jams/constraints-2.jam"),
            CONSTRAINTS_2_JAM_B3,
            "jams/constraints-2.jam",
        ),
    ];
    let mut stale = Vec::new();
    for (file, expected, label) in checks {
        let actual = blake3::hash(&fs::read(&file)?).to_hex();
        if actual.as_str() != expected {
            stale.push(format!("{label}: actual {actual}, pinned {expected}"));
        }
    }
    if !stale.is_empty() {
        return Err(format!(
            "softed-constraints native shortcut is stale ({}). honk substitutes a fixed cued \
             jam pair for this exact content; re-validate byte parity and update the pins in \
             honk.rs.",
            stale.join("; "),
        )
        .into());
    }
    Ok(())
}

fn softed_constraints_value(context: &mut Context, directory: &Path) -> Result<Noun> {
    let constraints_0_1 = fs::read(directory.join("jams/constraints-0-1.jam"))?;
    let constraints_2 = fs::read(directory.join("jams/constraints-2.jam"))?;

    trace_native("cueing softed-constraints static jam pair");
    catch_nock_panic("cueing softed-constraints static jam pair", || {
        let constraints_0_1 = cue_bytes_to_stack(&mut context.stack, &constraints_0_1)?;
        let constraints_2 = cue_bytes_to_stack(&mut context.stack, &constraints_2)?;
        Ok(T(&mut context.stack, &[constraints_0_1, constraints_2]))
    })
}

fn jam_dynock_output_native(ut: &mut Ut<'_>, ty: Noun, formula: Noun, typed: bool) -> Vec<u8> {
    let ty = if typed { ty } else { ty_noun(&mut *ut.slab) };
    let battery = T(&mut *ut.slab, &[D(1), formula]);
    let trap = T(&mut *ut.slab, &[battery, D(0)]);
    let dynock = T(&mut *ut.slab, &[ty, trap]);
    jam_ut_noun(ut, dynock)
}

fn hoonc_data_octs_vase_trap(
    slab: &mut NounSlab<NockJammer>,
    ty: Noun,
    value: Noun,
    value_space: &NounSpace,
) -> Noun {
    let slab_space = slab.noun_space();
    let vase = vase_native_with_spaces(slab, ty, &slab_space, value, value_space);
    let battery = hoonc_data_octs_trap_battery(slab);
    T(slab, &[battery, vase])
}

fn local_octs_type(slab: &mut NounSlab<NockJammer>) -> Noun {
    let len_ty = ty_atom_local(slab, "ud", None);
    let bytes_ty = ty_atom_local(slab, "", None);
    let len_ty = ty_face_local(slab, "p", len_ty);
    let bytes_ty = ty_face_local(slab, "q", bytes_ty);
    ty_cell_local(slab, len_ty, bytes_ty)
}

fn hoonc_data_octs_trap_battery(slab: &mut NounSlab<NockJammer>) -> Noun {
    let axis_three = T(slab, &[D(0), D(3)]);
    let path = path_terms_to_noun(slab, &["apps", "hoonc", "hoonc.hoon"]);
    let start = T(slab, &[D(988), D(10)]);
    let end = T(slab, &[D(988), D(14)]);
    let pint = T(slab, &[start, end]);
    let spot = T(slab, &[path, pint]);
    let hint_inner = T(slab, &[D(1), spot]);
    let spot_tag = term_to_noun(slab, "spot");
    let hint = T(slab, &[spot_tag, hint_inner]);
    T(slab, &[D(11), hint, axis_three])
}

fn path_terms_to_noun(slab: &mut NounSlab<NockJammer>, terms: &[&str]) -> Noun {
    let mut out = D(0);
    for term in terms.iter().rev() {
        let term = term_to_noun(slab, term);
        out = T(slab, &[term, out]);
    }
    out
}

fn vase_native(slab: &mut NounSlab<NockJammer>, ty: Noun, value: Noun) -> Noun {
    T(slab, &[ty, value])
}

fn vase_native_with_spaces(
    slab: &mut NounSlab<NockJammer>,
    ty: Noun,
    ty_space: &NounSpace,
    value: Noun,
    value_space: &NounSpace,
) -> Noun {
    let ty = slab.copy_into(ty, ty_space);
    let value = slab.copy_into(value, value_space);
    T(slab, &[ty, value])
}

fn trap_battery(trap: Noun, space: &NounSpace) -> Result<Noun> {
    let trap = trap
        .in_space(space)
        .as_cell()
        .map_err(|err| format!("wrapper did not produce a trap: {err:?}"))?;
    Ok(trap.head().noun())
}

// Native constructors for the exact tiny wrapper nouns produced by hoonc.hoon.
// The wrapper asset parity test compares these against the dynamic Nock-built
// wrappers so normal compiles don't need checked-in wrapper JAM files.
fn construct_exact_wrapper_batteries(
    slab: &mut NounSlab<NockJammer>,
) -> Result<(ExactWrapperBatteries, Noun)> {
    let constant_vase = axis_formula(slab, 3);
    let data_vase = axis_formula(slab, 3);
    let dir_hash_axis = axis_formula(slab, 3);
    let dir_hash_vase = hoonc_spot_formula(slab, 561, 8, 561, 9, dir_hash_axis);
    let eval_axis = axis_formula(slab, 3);
    let eval_vase = hoonc_spot_formula(slab, 1004, 8, 1004, 11, eval_axis);
    let label_vase = exact_label_vase_battery(slab);
    let slat = exact_slat_battery(slab);
    let swet_gun = exact_swet_gun_battery(slab);
    let shot = exact_shot_battery(slab);
    let value_trap_arbitrary = exact_value_trap_battery(slab, 555, 7, 9, 11);
    let value_trap_standard = exact_value_trap_battery(slab, 562, 6, 8, 10);
    let empty_ty = ty_atom_local(slab, "n", Some(D(0)));
    let empty_vase = vase_native(slab, empty_ty, D(0));
    let empty_axis = axis_formula(slab, 3);
    let empty_battery = hoonc_spot_formula(slab, 22, 6, 22, 9, empty_axis);
    let empty_trap_vase = T(slab, &[empty_battery, empty_vase]);
    Ok((
        ExactWrapperBatteries {
            constant_vase,
            data_vase,
            dir_hash: D(0),
            dir_hash_vase,
            eval_vase,
            label_vase,
            mint_gun: D(0),
            slat,
            swet_gate: D(0),
            swet_gun,
            shot,
            shot_gun: D(0),
            standard_output: D(0),
            value_trap_arbitrary,
            value_trap_standard,
        },
        empty_trap_vase,
    ))
}

fn hoonc_spot_formula(
    slab: &mut NounSlab<NockJammer>,
    start_line: u64,
    start_col: u64,
    end_line: u64,
    end_col: u64,
    formula: Noun,
) -> Noun {
    let path = path_terms_to_noun(slab, &["apps", "hoonc", "hoonc.hoon"]);
    let start = T(slab, &[D(start_line), D(start_col)]);
    let end = T(slab, &[D(end_line), D(end_col)]);
    let pint = T(slab, &[start, end]);
    let spot = T(slab, &[path, pint]);
    let hint_formula = T(slab, &[D(1), spot]);
    let spot_tag = term_to_noun(slab, "spot");
    let hint = T(slab, &[spot_tag, hint_formula]);
    T(slab, &[D(11), hint, formula])
}

fn hoonc_memo_formula(slab: &mut NounSlab<NockJammer>, formula: Noun) -> Noun {
    let memo_tag = term_to_noun(slab, "memo");
    let memo_value = T(slab, &[D(1), D(0)]);
    let hint = T(slab, &[memo_tag, memo_value]);
    T(slab, &[D(11), hint, formula])
}

fn exact_value_trap_battery(
    slab: &mut NounSlab<NockJammer>,
    line: u64,
    start_col: u64,
    kick_start_col: u64,
    end_col: u64,
) -> Noun {
    let kick = kick_trap_at_axis_formula(slab, 3);
    let kick = hoonc_spot_formula(slab, line, kick_start_col, line, end_col, kick);
    let axis_three = axis_formula(slab, 3);
    let formula = T(slab, &[D(7), kick, axis_three]);
    hoonc_spot_formula(slab, line, start_col, line, end_col, formula)
}

fn exact_label_vase_battery(slab: &mut NounSlab<NockJammer>) -> Noun {
    let vaz_axis = axis_formula(slab, 6);
    let vaz_axis = hoonc_spot_formula(slab, 1047, 14, 1047, 17, vaz_axis);
    let kick_axis_one = kick_trap_at_axis_formula(slab, 1);
    let vas_formula = T(slab, &[D(7), vaz_axis, kick_axis_one]);
    let vas_formula = hoonc_spot_formula(slab, 1047, 12, 1047, 17, vas_formula);

    let face_tag = term_to_noun(slab, "face");
    let face_tag = constant_formula(slab, face_tag);
    let face_tag = hoonc_spot_formula(slab, 1048, 5, 1048, 10, face_tag);
    let face_value = axis_formula(slab, 15);
    let face_value = hoonc_spot_formula(slab, 1048, 11, 1048, 15, face_value);
    let inner_ty = axis_formula(slab, 4);
    let inner_ty = hoonc_spot_formula(slab, 1048, 16, 1048, 21, inner_ty);
    let ty_formula = T(slab, &[face_tag, face_value, inner_ty]);
    let ty_formula = hoonc_spot_formula(slab, 1048, 4, 1048, 22, ty_formula);
    let value_formula = axis_formula(slab, 5);
    let value_formula = hoonc_spot_formula(slab, 1048, 23, 1048, 28, value_formula);
    let battery = T(slab, &[ty_formula, value_formula]);
    let battery = hoonc_spot_formula(slab, 1048, 3, 1048, 29, battery);
    let formula = T(slab, &[D(8), vas_formula, battery]);
    hoonc_spot_formula(slab, 1047, 3, 1048, 29, formula)
}

fn exact_slat_battery(slab: &mut NounSlab<NockJammer>) -> Noun {
    let hed_axis = axis_formula(slab, 6);
    let hed_axis = hoonc_spot_formula(slab, 1061, 20, 1061, 23, hed_axis);
    let hed_kick = kick_trap_at_axis_formula(slab, 1);
    let hed_formula = T(slab, &[D(7), hed_axis, hed_kick]);
    let hed_formula = hoonc_spot_formula(slab, 1061, 18, 1061, 23, hed_formula);
    let tal_axis = axis_formula(slab, 7);
    let tal_axis = hoonc_spot_formula(slab, 1061, 26, 1061, 29, tal_axis);
    let tal_kick = kick_trap_at_axis_formula(slab, 1);
    let tal_formula = T(slab, &[D(7), tal_axis, tal_kick]);
    let tal_formula = hoonc_spot_formula(slab, 1061, 24, 1061, 29, tal_formula);
    let binding_formula = T(slab, &[hed_formula, tal_formula]);
    let binding_formula = hoonc_spot_formula(slab, 1061, 7, 1061, 30, binding_formula);

    let cell_tag = term_to_noun(slab, "cell");
    let cell_tag = constant_formula(slab, cell_tag);
    let cell_tag = hoonc_spot_formula(slab, 1062, 5, 1062, 10, cell_tag);
    let bed_axis = axis_formula(slab, 4);
    let bed_axis = hoonc_spot_formula(slab, 1062, 13, 1062, 16, bed_axis);
    let bed_ty = head_formula(slab, bed_axis);
    let bed_ty = hoonc_spot_formula(slab, 1062, 11, 1062, 16, bed_ty);
    let bal_axis = axis_formula(slab, 5);
    let bal_axis = hoonc_spot_formula(slab, 1062, 19, 1062, 22, bal_axis);
    let bal_ty = head_formula(slab, bal_axis);
    let bal_ty = hoonc_spot_formula(slab, 1062, 17, 1062, 22, bal_ty);
    let ty_formula = T(slab, &[cell_tag, bed_ty, bal_ty]);
    let ty_formula = hoonc_spot_formula(slab, 1062, 4, 1062, 23, ty_formula);

    let bed_axis = axis_formula(slab, 4);
    let bed_axis = hoonc_spot_formula(slab, 1062, 27, 1062, 30, bed_axis);
    let bed_value = tail_formula(slab, bed_axis);
    let bed_value = hoonc_spot_formula(slab, 1062, 25, 1062, 30, bed_value);
    let bal_axis = axis_formula(slab, 5);
    let bal_axis = hoonc_spot_formula(slab, 1062, 33, 1062, 36, bal_axis);
    let bal_value = tail_formula(slab, bal_axis);
    let bal_value = hoonc_spot_formula(slab, 1062, 31, 1062, 36, bal_value);
    let value_formula = T(slab, &[bed_value, bal_value]);
    let value_formula = hoonc_spot_formula(slab, 1062, 24, 1062, 37, value_formula);
    let battery = T(slab, &[ty_formula, value_formula]);
    let battery = hoonc_spot_formula(slab, 1062, 3, 1062, 38, battery);
    let formula = T(slab, &[D(8), binding_formula, battery]);
    hoonc_spot_formula(slab, 1061, 3, 1062, 38, formula)
}

fn exact_swet_gun_battery(slab: &mut NounSlab<NockJammer>) -> Noun {
    let ty_formula = axis_formula(slab, 12);
    let ty_formula = hoonc_spot_formula(slab, 1095, 4, 1095, 9, ty_formula);
    let tap_axis = axis_formula(slab, 7);
    let tap_axis = hoonc_spot_formula(slab, 1095, 17, 1095, 20, tap_axis);
    let kick = kick_trap_at_axis_formula(slab, 1);
    let tap_value = T(slab, &[D(7), tap_axis, kick]);
    let tap_value = hoonc_spot_formula(slab, 1095, 15, 1095, 20, tap_value);
    let tap_value = tail_formula(slab, tap_value);
    let tap_value = hoonc_spot_formula(slab, 1095, 13, 1095, 20, tap_value);
    let formula_axis = axis_formula(slab, 13);
    let formula_axis = hoonc_spot_formula(slab, 1095, 21, 1095, 26, formula_axis);
    let product_formula = T(slab, &[D(2), tap_value, formula_axis]);
    let product_formula = hoonc_spot_formula(slab, 1095, 10, 1095, 27, product_formula);
    let battery = T(slab, &[ty_formula, product_formula]);
    let battery = hoonc_spot_formula(slab, 1095, 3, 1095, 28, battery);
    let battery = hoonc_memo_formula(slab, battery);
    hoonc_spot_formula(slab, 1094, 7, 1095, 28, battery)
}

fn exact_shot_battery(slab: &mut NounSlab<NockJammer>) -> Noun {
    let ty_formula = axis_formula(slab, 6);
    let ty_formula = hoonc_spot_formula(slab, 1081, 4, 1081, 7, ty_formula);

    let gate_axis = axis_formula(slab, 14);
    let gate_axis = hoonc_spot_formula(slab, 1081, 16, 1081, 19, gate_axis);
    let gate_kick = kick_trap_at_axis_formula(slab, 1);
    let gate_value = T(slab, &[D(7), gate_axis, gate_kick]);
    let gate_value = hoonc_spot_formula(slab, 1081, 14, 1081, 19, gate_value);
    let gate_value = tail_formula(slab, gate_value);
    let gate_value = hoonc_spot_formula(slab, 1081, 12, 1081, 19, gate_value);
    let sample_axis = axis_formula(slab, 15);
    let sample_axis = hoonc_spot_formula(slab, 1081, 24, 1081, 27, sample_axis);
    let sample_kick = kick_trap_at_axis_formula(slab, 1);
    let sample_value = T(slab, &[D(7), sample_axis, sample_kick]);
    let sample_value = hoonc_spot_formula(slab, 1081, 22, 1081, 27, sample_value);
    let sample_value = tail_formula(slab, sample_value);
    let sample_value = hoonc_spot_formula(slab, 1081, 20, 1081, 27, sample_value);
    let slam_subject = T(slab, &[gate_value, sample_value]);
    let slam_subject = hoonc_spot_formula(slab, 1081, 11, 1081, 28, slam_subject);

    let one9 = T(slab, &[D(1), D(9)]);
    let one9 = hoonc_spot_formula(slab, 1081, 30, 1081, 32, one9);
    let one2 = T(slab, &[D(1), D(2)]);
    let one2 = hoonc_spot_formula(slab, 1081, 33, 1081, 34, one2);
    let one10 = T(slab, &[D(1), D(10)]);
    let one10 = hoonc_spot_formula(slab, 1081, 35, 1081, 38, one10);
    let one6 = T(slab, &[D(1), D(6)]);
    let one6 = hoonc_spot_formula(slab, 1081, 40, 1081, 41, one6);
    let one0_a = T(slab, &[D(1), D(0)]);
    let one0_a = hoonc_spot_formula(slab, 1081, 42, 1081, 44, one0_a);
    let one3 = T(slab, &[D(1), D(3)]);
    let one3 = hoonc_spot_formula(slab, 1081, 45, 1081, 46, one3);
    let edit_axis = T(slab, &[one6, one0_a, one3]);
    let edit_axis = hoonc_spot_formula(slab, 1081, 39, 1081, 47, edit_axis);
    let one0_b = T(slab, &[D(1), D(0)]);
    let one0_b = hoonc_spot_formula(slab, 1081, 48, 1081, 50, one0_b);
    let one2_b = T(slab, &[D(1), D(2)]);
    let one2_b = hoonc_spot_formula(slab, 1081, 51, 1081, 52, one2_b);
    let slam_formula = T(slab, &[one9, one2, one10, edit_axis, one0_b, one2_b]);
    let slam_formula = hoonc_spot_formula(slab, 1081, 29, 1081, 53, slam_formula);
    let product_formula = T(slab, &[D(2), slam_subject, slam_formula]);
    let product_formula = hoonc_spot_formula(slab, 1081, 8, 1081, 54, product_formula);
    let battery = T(slab, &[ty_formula, product_formula]);
    hoonc_spot_formula(slab, 1081, 3, 1081, 55, battery)
}

fn swet_trap_payload_type(trap: Noun, space: &NounSpace) -> Result<Noun> {
    Ok(swet_trap_payload_gun(trap, space)?.0)
}

fn swet_trap_payload_gun(trap: Noun, space: &NounSpace) -> Result<(Noun, Noun)> {
    let trap = trap
        .in_space(space)
        .as_cell()
        .map_err(|err| format!("swet wrapper did not produce a trap: {err:?}"))?;
    let payload = trap
        .tail()
        .as_cell()
        .map_err(|err| format!("swet trap payload is not a cell: {err:?}"))?;
    let gun = payload
        .head()
        .as_cell()
        .map_err(|err| format!("swet trap gun is not a cell: {err:?}"))?;
    Ok((gun.head().noun(), gun.tail().noun()))
}

fn axis_formula(slab: &mut NounSlab<NockJammer>, axis: u64) -> Noun {
    let axis = Atom::new(slab, axis).as_noun();
    T(slab, &[D(0), axis])
}

fn constant_formula(slab: &mut NounSlab<NockJammer>, noun: Noun) -> Noun {
    T(slab, &[D(1), noun])
}

fn kick_trap_at_axis_formula(slab: &mut NounSlab<NockJammer>, axis: u64) -> Noun {
    let trap_axis = axis_formula(slab, axis);
    T(slab, &[D(9), D(2), trap_axis])
}

fn head_formula(slab: &mut NounSlab<NockJammer>, formula: Noun) -> Noun {
    let head_axis = axis_formula(slab, 2);
    T(slab, &[D(7), formula, head_axis])
}

fn tail_formula(slab: &mut NounSlab<NockJammer>, formula: Noun) -> Noun {
    let tail_axis = axis_formula(slab, 3);
    T(slab, &[D(7), formula, tail_axis])
}

fn ty_cell_local(slab: &mut NounSlab<NockJammer>, head: Noun, tail: Noun) -> Noun {
    let tag = term_to_noun(slab, "cell");
    T(slab, &[tag, head, tail])
}

fn empty_subject_type(slab: &mut NounSlab<NockJammer>) -> Noun {
    ty_atom_local(slab, "n", Some(D(0)))
}

fn ty_atom_local(slab: &mut NounSlab<NockJammer>, aura: &str, value: Option<Noun>) -> Noun {
    let aura = if aura.is_empty() { "$" } else { aura };
    let aura_noun = term_to_noun(slab, aura);
    let bits = match value {
        Some(value) => T(slab, &[D(0), value]),
        None => D(0),
    };
    let tag = term_to_noun(slab, "atom");
    T(slab, &[tag, aura_noun, bits])
}

fn ty_face_local(slab: &mut NounSlab<NockJammer>, name: &str, inner: Noun) -> Noun {
    let tag = term_to_noun(slab, "face");
    let name = term_to_noun(slab, name);
    T(slab, &[tag, name, inner])
}

fn jam_slab_noun(slab: &mut NounSlab<NockJammer>, noun: Noun) -> Vec<u8> {
    slab.set_root(noun);
    slab.jam().to_vec()
}

fn jam_ut_noun(ut: &mut Ut<'_>, noun: Noun) -> Vec<u8> {
    ut.slab.set_root(noun);
    ut.slab.jam().to_vec()
}

fn cue_subject_type_to_slab(slab: &mut NounSlab<NockJammer>, raw: &[u8]) -> Result<Noun> {
    let mut stack = NockStack::new(NOCK_STACK_SIZE_MEDIUM, 0);
    let root = <Noun as NounExt>::cue_bytes_slice(&mut stack, raw)
        .map_err(|err| format!("cue subject type jam: {err:?}"))?;
    let stack_space = stack.noun_space();
    let subject_ty = extract_subject_type_root(root, &stack_space)?;
    Ok(slab.copy_into(subject_ty, &stack_space))
}

fn cue_honc_formula_to_slab(slab: &mut NounSlab<NockJammer>, raw: &[u8]) -> Result<Noun> {
    cue_noun_to_slab(slab, raw, "embedded honc formula")
}

fn cue_noun_to_slab(slab: &mut NounSlab<NockJammer>, raw: &[u8], label: &str) -> Result<Noun> {
    let mut stack = NockStack::new(NOCK_STACK_SIZE_MEDIUM, 0);
    let noun = <Noun as NounExt>::cue_bytes_slice(&mut stack, raw)
        .map_err(|err| format!("cue {label} jam: {err:?}"))?;
    let stack_space = stack.noun_space();
    Ok(slab.copy_into(noun, &stack_space))
}

fn load_cold_state(context: &mut Context, raw: &[u8], label: &str) -> Result<()> {
    let cold_noun = <Noun as NounExt>::cue_bytes_slice(&mut context.stack, raw)
        .map_err(|err| format!("cue {label} cold jam: {err:?}"))?;
    let stack_space = context.stack.noun_space();
    let (battery_to_paths, root_to_paths, path_to_batteries) =
        Cold::from_noun(&mut context.stack, &cold_noun, &stack_space)
            .map_err(|err| format!("decode {label} cold state: {err:?}"))?;
    context.cold = Cold::from_vecs(
        &mut context.stack, battery_to_paths, root_to_paths, path_to_batteries,
    );
    context.warm = Warm::init(
        &mut context.stack, &mut context.cold, &context.hot, &context.test_jets,
        context.jet_dispatch,
    );
    Ok(())
}

fn extract_subject_type_root(root: Noun, space: &NounSpace) -> Result<Noun> {
    if looks_like_type_noun(root, space) {
        return Ok(root);
    }
    if let Ok(cell) = root.in_space(space).as_cell() {
        let head = cell.head().noun();
        if looks_like_type_noun(head, space) {
            return Ok(head);
        }
    }
    Err("subject type jam did not contain a Hoon type noun".into())
}

fn prelude_type_from_subject_type(subject_ty: Noun, space: &NounSpace) -> Result<Noun> {
    let (_deps_ty, prelude_ty) = type_cell_parts(subject_ty, space).map_err(|err| {
        format!("shared subject type override should be [%cell deps prelude]: {err}")
    })?;
    Ok(prelude_ty)
}

fn type_cell_parts(noun: Noun, space: &NounSpace) -> Result<(Noun, Noun)> {
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|err| format!("type noun is not a cell: {err:?}"))?;
    let tag = type_tag_atom_text(cell.head().noun(), space).unwrap_or_default();
    if tag != "cell" {
        return Err(format!("expected %cell type, got %{tag}").into());
    }
    let parts = cell
        .tail()
        .as_cell()
        .map_err(|err| format!("%cell type payload is not a cell: {err:?}"))?;
    Ok((parts.head().noun(), parts.tail().noun()))
}

fn looks_like_type_noun(noun: Noun, space: &NounSpace) -> bool {
    if let Some(tag) = type_tag_atom_text(noun, space) {
        return matches!(tag.as_str(), "noun" | "void" | "blur");
    }
    let Ok(cell) = noun.in_space(space).as_cell() else {
        return false;
    };
    let Some(tag) = type_tag_atom_text(cell.head().noun(), space) else {
        return false;
    };
    matches!(
        tag.as_str(),
        "atom"
            | "cell"
            | "core"
            | "cube"
            | "face"
            | "fork"
            | "hold"
            | "hint"
            | "noun"
            | "void"
            | "blur"
    )
}

fn type_tag_atom_text(noun: Noun, space: &NounSpace) -> Option<String> {
    let atom = noun.in_space(space).as_atom().ok()?;
    let text = std::str::from_utf8(atom.as_ne_bytes())
        .ok()?
        .trim_end_matches('\0')
        .to_string();
    if text.is_empty() {
        return None;
    }
    if !text
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return None;
    }
    Some(text)
}

fn cue_bytes_to_stack(stack: &mut NockStack, bytes: &[u8]) -> Result<Noun> {
    <Noun as NounExt>::cue_bytes_slice(stack, bytes)
        .map_err(|err| format!("cue noun: {err:?}").into())
}

fn jam_standard_kernel_trap_native(
    context: &mut Context,
    gate: Noun,
    dir_hash: u32,
) -> Result<Vec<u8>> {
    let product = slam_gate_with_atom_sample_noun(context, gate, dir_hash as u64)?;
    catch_nock_panic("jamming standard kernel product trap", || unsafe {
        let mut bytes = Vec::new();
        context.with_stack_frame(0, |context| {
            let start = Instant::now();
            bytes = jam_standard_kernel_product_trap_in_frame(context, product);
            if env::var_os("NATIVE_HOON_TRACE").is_some() {
                eprintln!(
                    "[honk] done jamming standard kernel product trap {:.3}s",
                    start.elapsed().as_secs_f64()
                );
            }
        });
        Ok(bytes)
    })
}

// Constant-fold evaluation interns shared mack-core copies on the eval
// stack for the lifetime of each entry compile; 16GB fills up on
// fold-heavy kernels (tx-engine digests), and exhaustion silently degrades
// folds to step evaluation. The reservation is virtual, so size it well
// above observed peaks.
const HONK_EVAL_STACK_SIZE: usize = NOCK_STACK_SIZE_MEDIUM; // 16GB

fn create_eval_context() -> Context {
    let mut stack = NockStack::new(HONK_EVAL_STACK_SIZE, 0);
    let cold = Cold::new(&mut stack);
    create_context(
        stack,
        native_hot_state(),
        cold,
        None,
        vec![],
        JetDispatchMode::HintBlind,
    )
}

fn eval_formula_noun_in_context(
    eval_context: &mut Context,
    formula: Noun,
    formula_space: &NounSpace,
    label: &str,
    build_subject: impl FnOnce(&mut NockStack) -> Noun,
) -> Result<Noun> {
    catch_nock_panic(format!("evaluating {label}"), || {
        let eval_value = unsafe {
            eval_context.with_stack_frame(0, |context| -> std::result::Result<Noun, NockError> {
                let trace = env::var_os("NATIVE_HOON_TRACE").is_some();
                // Eval boundary: brand the interpreter's stack so the slab
                // formula must be copied in (acquiring the stack's brand)
                // before it can reach `interpret`. The raw slab `formula` no
                // longer type-checks as an argument here — the "alien noun"
                // hazard is now a brand error rather than a runtime range panic.
                let stack_space = context.stack.noun_space();
                stack_space.with_brand(|brand| -> std::result::Result<Noun, NockError> {
                    let start = Instant::now();
                    let subject = brand.handle(build_subject(&mut context.stack));
                    if trace {
                        eprintln!(
                            "[honk] eval {label}: assembled subject {:.3}s",
                            start.elapsed().as_secs_f64()
                        );
                    }
                    let start = Instant::now();
                    let formula = brand.copy_in(&mut context.stack, formula, formula_space);
                    if trace {
                        eprintln!(
                            "[honk] eval {label}: copied formula {:.3}s",
                            start.elapsed().as_secs_f64()
                        );
                    }
                    let start = Instant::now();
                    let interpreted = brand.interpret(context, subject, formula);
                    let elapsed = start.elapsed();
                    add_interpret_timing(elapsed);
                    debug!(
                        label = label,
                        reason = "evaluating minted Hoon formula with its assembled subject",
                        elapsed_ms = elapsed.as_secs_f64() * 1_000.0,
                        success = interpreted.is_ok(),
                        "NockStack eval finished"
                    );
                    match interpreted {
                        Ok(value) => {
                            if trace {
                                eprintln!(
                                    "[honk] eval {label}: interpreted {:.3}s",
                                    elapsed.as_secs_f64()
                                );
                            }
                            // The branded product can't escape `with_brand`;
                            // unwrap to the raw stack noun, which the caller
                            // re-associates with the eval stack exactly as the
                            // pre-branded path did.
                            Ok(value.unbranded().noun())
                        }
                        Err(err) => {
                            if trace {
                                trace_interpret_error(context, &err);
                            }
                            Err(err)
                        }
                    }
                })
            })
        }
        .map_err(|err| format!("interpret formula: {err:?}"))?;
        Ok(eval_value)
    })
}

fn trace_interpret_error(context: &mut Context, err: &NockError) {
    let trace = match err {
        NockError::Deterministic(_, trace)
        | NockError::NonDeterministic(_, trace)
        | NockError::ScryCrashed(trace) => *trace,
        NockError::ScryBlocked(path) => *path,
    };
    let stack_space = context.stack.noun_space();
    if let Some(location) = decode_spot_trace(trace, &stack_space) {
        eprintln!("[honk] trace frame {location}");
    }
    let trace = normalize_interpret_trace(&mut context.stack, trace);
    let tone = Cell::new(&mut context.stack, D(2), trace);
    let Ok(toon) = mook(context, tone, false) else {
        return;
    };
    eprintln!("[honk] interpreted trace:");
    let stack_space = context.stack.noun_space();
    let toon = toon.in_space(&stack_space);
    for tank in toon.tail().list_iter() {
        context.slogger.slog(&mut context.stack, 1, tank.noun());
    }
}

fn normalize_interpret_trace(stack: &mut NockStack, trace: Noun) -> Noun {
    let space = stack.noun_space();
    let Some(cell) = trace.in_space(&space).cell() else {
        return trace;
    };
    if cell.head().atom().is_some() {
        T(stack, &[trace, D(0)])
    } else {
        trace
    }
}

fn decode_spot_trace(trace: Noun, space: &NounSpace) -> Option<String> {
    let trace_cell = trace.in_space(space).cell()?;
    let hunk = if trace_cell.head().cell().is_some() {
        trace_cell.head().noun()
    } else {
        trace
    };
    let cell = hunk.in_space(space).cell()?;
    if type_tag_atom_text(cell.head().noun(), space).as_deref()? != "spot" {
        return None;
    }
    let (path, start, end) = decode_spot_noun(cell.tail().noun(), space)?;
    Some(format!(
        "{}:{}:{}-{}:{}",
        path, start.0, start.1, end.0, end.1
    ))
}

fn decode_spot_noun(spot: Noun, space: &NounSpace) -> Option<Spot> {
    let cell = spot.in_space(space).cell()?;
    let path = decode_path_noun(cell.head().noun(), space)?;
    let (start, end) = decode_pint_noun(cell.tail().noun(), space)?;
    Some((path, start, end))
}

fn decode_path_noun(mut path: Noun, space: &NounSpace) -> Option<String> {
    let mut parts = Vec::new();
    while !unsafe { path.raw_equals(&D(0)) } {
        let cell = path.in_space(space).cell()?;
        parts.push(atom_text(cell.head().noun(), space)?);
        path = cell.tail().noun();
    }
    Some(parts.join("/"))
}

fn decode_pint_noun(pint: Noun, space: &NounSpace) -> Option<((u64, u64), (u64, u64))> {
    let cell = pint.in_space(space).cell()?;
    let start = cell.head().cell()?;
    let end = cell.tail().cell()?;
    Some((
        (
            atom_u64(start.head().noun(), space)?,
            atom_u64(start.tail().noun(), space)?,
        ),
        (
            atom_u64(end.head().noun(), space)?,
            atom_u64(end.tail().noun(), space)?,
        ),
    ))
}

fn atom_text(noun: Noun, space: &NounSpace) -> Option<String> {
    let atom = noun.in_space(space).as_atom().ok()?;
    let text = std::str::from_utf8(atom.as_ne_bytes())
        .ok()?
        .trim_end_matches('\0')
        .to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn atom_u64(noun: Noun, space: &NounSpace) -> Option<u64> {
    noun.in_space(space).as_atom().ok()?.as_u64().ok()
}

fn copy_noun_to_allocator<A: NounAllocator>(
    allocator: &mut A,
    root: Noun,
    space: &NounSpace,
) -> Noun {
    allocator.copy_into(root, space)
}

fn slam_gate_with_atom_sample_noun(context: &mut Context, gate: Noun, sample: u64) -> Result<Noun> {
    trace_native(format!("slamming standard kernel gate sample={sample}"));
    catch_nock_panic(
        format!("slamming standard kernel gate sample={sample}"),
        || unsafe {
            context
                .with_stack_frame(0, |context| {
                    let sample_noun = Atom::new(&mut context.stack, sample).as_noun();
                    let subject = T(&mut context.stack, &[gate, sample_noun]);
                    let slam = slam_gate_formula(&mut context.stack);
                    let start = Instant::now();
                    let result = interpret(context, subject, slam);
                    let elapsed = start.elapsed();
                    add_interpret_timing(elapsed);
                    debug!(
                        sample = sample,
                        reason = "slamming standard kernel gate with directory hash sample",
                        elapsed_ms = elapsed.as_secs_f64() * 1_000.0,
                        success = result.is_ok(),
                        "NockStack eval finished"
                    );
                    if env::var_os("NATIVE_HOON_TRACE").is_some() {
                        eprintln!(
                            "[honk] done slamming standard kernel gate {:.3}s",
                            elapsed.as_secs_f64()
                        );
                    }
                    result
                })
                .map_err(|err| format!("slam gate: {err:?}").into())
        },
    )
}

fn slam_gate_with_atom_sample_noun_in_frame(
    context: &mut Context,
    gate: Noun,
    sample: u64,
) -> std::result::Result<Noun, NockError> {
    let sample_noun = Atom::new(&mut context.stack, sample).as_noun();
    let subject = T(&mut context.stack, &[gate, sample_noun]);
    let slam = slam_gate_formula(&mut context.stack);
    let start = Instant::now();
    let result = interpret(context, subject, slam);
    let elapsed = start.elapsed();
    add_interpret_timing(elapsed);
    debug!(
        sample = sample,
        reason = "slamming standard kernel gate with directory hash sample inside an existing eval frame",
        elapsed_ms = elapsed.as_secs_f64() * 1_000.0,
        success = result.is_ok(),
        "NockStack eval finished"
    );
    result
}

fn jam_standard_kernel_product_trap_in_frame(context: &mut Context, product: Noun) -> Vec<u8> {
    let battery = T(&mut context.stack, &[D(1), product]);
    let trap = T(&mut context.stack, &[battery, D(0)]);
    let jammed = nock_jam(&mut context.stack, trap);
    let space = context.stack.noun_space();
    jammed.in_space(&space).as_ne_bytes().to_vec()
}

fn slam_gate_formula<A: NounAllocator>(allocator: &mut A) -> Noun {
    let slot3 = T(allocator, &[D(0), D(3)]);
    let edit_axis6 = T(allocator, &[D(6), slot3]);
    let slot2 = T(allocator, &[D(0), D(2)]);
    let edit_gate = T(allocator, &[D(10), edit_axis6, slot2]);
    T(allocator, &[D(9), D(2), edit_gate])
}

fn catch_nock_panic<T>(label: impl Into<String>, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let label = label.into();
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => {
            let detail = payload
                .downcast_ref::<AllocationError>()
                .map(|err| format!(": {err}"))
                .or_else(|| {
                    payload
                        .downcast_ref::<String>()
                        .map(|err| format!(": {err}"))
                })
                .or_else(|| {
                    payload
                        .downcast_ref::<&'static str>()
                        .map(|err| format!(": {err}"))
                })
                .unwrap_or_default();
            Err(format!("{label}: nock stack panic{detail}").into())
        }
    }
}

fn trace_native(message: impl AsRef<str>) {
    if env::var_os("NATIVE_HOON_TRACE").is_some() {
        eprintln!("[honk] {}", message.as_ref());
    }
}

fn trace_timed<T>(label: impl Into<String>, f: impl FnOnce() -> Result<T>) -> Result<T> {
    if env::var_os("NATIVE_HOON_TRACE").is_none() {
        return f();
    }

    let label = label.into();
    let start = Instant::now();
    eprintln!("[honk] start {label}");
    let result = f();
    let elapsed = start.elapsed().as_secs_f64();
    match &result {
        Ok(_) => eprintln!("[honk] done {label} {elapsed:.3}s"),
        Err(err) => eprintln!("[honk] failed {label} {elapsed:.3}s: {err}"),
    }
    result
}

fn directory_mug_with_files(
    entry: &Path,
    deps_dir: &Path,
    files: Option<&[PathBuf]>,
) -> Result<u32> {
    let directory = deps_dir.canonicalize()?;
    let target_key = entry_path_for_hoon(entry, deps_dir)?;
    let target_contents = fs::read(entry)?;
    let allowed_files = files
        .map(|files| hoonc_directory_allowed_paths(&directory, files))
        .transpose()?;

    let mut entries = Vec::new();
    for entry_result in WalkDir::new(&directory)
        .follow_links(true)
        .into_iter()
        .filter_entry(hoonc::is_valid_file_or_dir)
    {
        let dir_entry = entry_result?;
        if !dir_entry.metadata()?.is_file() {
            continue;
        }
        let path = dir_entry.path();
        let stripped = path.strip_prefix(&directory).map_err(|_| {
            format!(
                "dependency path does not share base prefix: {}",
                path.display()
            )
        })?;
        let path_str = format!("/{}", stripped.to_string_lossy());
        if let Some(allowed_files) = &allowed_files {
            if !allowed_files.contains(&path_str) {
                continue;
            }
        }
        entries.push((path_str, fs::read(path)?));
    }

    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let mut map = D(0);
    let key = hoon_path_text_to_noun(&mut slab, &target_key)?;
    let value = Atom::from_value(&mut slab, target_contents)?.as_noun();
    map = map_put_mug(&mut slab, map, key, value)?;

    for (path, contents) in entries.into_iter().rev() {
        let key = hoon_path_text_to_noun(&mut slab, &path)?;
        let value = Atom::from_value(&mut slab, contents)?.as_noun();
        map = map_put_mug(&mut slab, map, key, value)?;
    }

    let space = slab.noun_space();
    Ok(slab_mug(map, &space))
}

fn noun_eq(a: Noun, b: Noun, space: &NounSpace) -> Result<bool> {
    let mut stack = Vec::with_capacity(64);
    stack.push((a, b));
    while let Some((left, right)) = stack.pop() {
        if unsafe { left.raw_equals(&right) } {
            continue;
        }
        let left_mug = get_mug(left, space).unwrap_or_else(|| slab_mug(left, space));
        let right_mug = get_mug(right, space).unwrap_or_else(|| slab_mug(right, space));
        if left_mug != right_mug {
            return Ok(false);
        }
        if left.is_atom() && right.is_atom() {
            let left_atom = left.in_space(space).as_atom()?;
            let right_atom = right.in_space(space).as_atom()?;
            if !left_atom.eq_bytes(right_atom.as_ne_bytes()) {
                return Ok(false);
            }
            continue;
        }
        if left.is_cell() && right.is_cell() {
            let left_cell = left.in_space(space).as_cell()?;
            let right_cell = right.in_space(space).as_cell()?;
            stack.push((left_cell.tail().noun(), right_cell.tail().noun()));
            stack.push((left_cell.head().noun(), right_cell.head().noun()));
            continue;
        }
        return Ok(false);
    }
    Ok(true)
}

fn noun_is_zero(noun: Noun) -> bool {
    unsafe { noun.raw_equals(&D(0)) }
}

fn slab_mug(noun: Noun, space: &NounSpace) -> u32 {
    let mut stack = vec![noun];
    while let Some(current) = stack.pop() {
        let Ok(mut allocated) = current.as_allocated() else {
            continue;
        };
        if get_mug(current, space).is_some() {
            continue;
        }
        let current_handle = current.in_space(space);
        if current_handle.is_atom() {
            let atom = current_handle
                .as_atom()
                .expect("atom expected when current.is_atom()");
            unsafe {
                set_mug(&mut allocated, calc_atom_mug_u32(atom.atom(), space), space);
            }
            continue;
        }
        let cell = current_handle
            .as_cell()
            .expect("cell expected when current.is_cell()");
        match (
            get_mug(cell.head().noun(), space),
            get_mug(cell.tail().noun(), space),
        ) {
            (Some(head_mug), Some(tail_mug)) => unsafe {
                set_mug(
                    &mut allocated,
                    calc_cell_mug_u32(head_mug, tail_mug, space),
                    space,
                );
            },
            _ => {
                stack.push(current);
                stack.push(cell.tail().noun());
                stack.push(cell.head().noun());
            }
        }
    }
    get_mug(noun, space).expect("noun should have a mug once mugged")
}

fn dor(slab: &mut NounSlab<NockJammer>, a: Noun, b: Noun) -> bool {
    if unsafe { a.raw_equals(&b) } {
        return true;
    }
    let space = slab.noun_space();
    let a_mug = get_mug(a, &space).unwrap_or_else(|| slab_mug(a, &space));
    let b_mug = get_mug(b, &space).unwrap_or_else(|| slab_mug(b, &space));
    if a_mug == b_mug && matches!(noun_eq(a, b, &space), Ok(true)) {
        return true;
    }
    match (a.is_atom(), b.is_atom()) {
        (true, true) => {
            let atom_a = a
                .in_space(&space)
                .as_atom()
                .expect("atom expected when a.is_atom()");
            let atom_b = b
                .in_space(&space)
                .as_atom()
                .expect("atom expected when b.is_atom()");
            lth_b(slab, atom_a.atom(), atom_b.atom(), &space)
        }
        (true, false) => true,
        (false, true) => false,
        (false, false) => {
            let cell_a = a
                .in_space(&space)
                .as_cell()
                .expect("cell expected when a.is_cell()");
            let cell_b = b
                .in_space(&space)
                .as_cell()
                .expect("cell expected when b.is_cell()");
            let a_head = cell_a.head().noun();
            let b_head = cell_b.head().noun();
            let a_tail = cell_a.tail().noun();
            let b_tail = cell_b.tail().noun();
            if unsafe { a_head.raw_equals(&b_head) }
                || (slab_mug(a_head, &space) == slab_mug(b_head, &space)
                    && matches!(noun_eq(a_head, b_head, &space), Ok(true)))
            {
                dor(slab, a_tail, b_tail)
            } else {
                dor(slab, a_head, b_head)
            }
        }
    }
}

fn gor_mug(slab: &mut NounSlab<NockJammer>, a: Noun, b: Noun) -> bool {
    let space = slab.noun_space();
    match slab_mug(a, &space).cmp(&slab_mug(b, &space)) {
        cmp::Ordering::Less => true,
        cmp::Ordering::Greater => false,
        cmp::Ordering::Equal => dor(slab, a, b),
    }
}

fn mor_mug(slab: &mut NounSlab<NockJammer>, a: Noun, b: Noun) -> bool {
    let space = slab.noun_space();
    let mug_a = slab_mug(a, &space);
    let mug_b = slab_mug(b, &space);
    let mug_mug_a = slab_mug(noun_u64(slab, mug_a as u64), &space);
    let mug_mug_b = slab_mug(noun_u64(slab, mug_b as u64), &space);
    match mug_mug_a.cmp(&mug_mug_b) {
        cmp::Ordering::Less => true,
        cmp::Ordering::Greater => false,
        cmp::Ordering::Equal => dor(slab, a, b),
    }
}

fn map_put_mug(
    slab: &mut NounSlab<NockJammer>,
    tree: Noun,
    key: Noun,
    value: Noun,
) -> Result<Noun> {
    if noun_is_zero(tree) {
        let node = T(slab, &[key, value]);
        return Ok(T(slab, &[node, D(0), D(0)]));
    }

    let space = slab.noun_space();
    let tree_cell = tree.in_space(&space).as_cell()?;
    let node = tree_cell.head().noun();
    let rest_cell = tree_cell.tail().as_cell()?;
    let left = rest_cell.head().noun();
    let right = rest_cell.tail().noun();

    let node_cell = node.in_space(&space).as_cell()?;
    let node_key = node_cell.head().noun();
    let node_val = node_cell.tail().noun();

    if noun_eq(key, node_key, &space)? {
        if noun_eq(value, node_val, &space)? {
            return Ok(tree);
        }
        let new_node = T(slab, &[key, value]);
        return Ok(T(slab, &[new_node, left, right]));
    }

    if gor_mug(slab, key, node_key) {
        let d = map_put_mug(slab, left, key, value)?;
        let space = slab.noun_space();
        let d_cell = d.in_space(&space).as_cell()?;
        let d_node = d_cell.head().noun();
        let d_rest_cell = d_cell.tail().as_cell()?;
        let d_left = d_rest_cell.head().noun();
        let d_right = d_rest_cell.tail().noun();
        let d_key = d_node.in_space(&space).as_cell()?.head().noun();

        if mor_mug(slab, node_key, d_key) {
            Ok(T(slab, &[node, d, right]))
        } else {
            let new_a = T(slab, &[node, d_right, right]);
            Ok(T(slab, &[d_node, d_left, new_a]))
        }
    } else {
        let d = map_put_mug(slab, right, key, value)?;
        let space = slab.noun_space();
        let d_cell = d.in_space(&space).as_cell()?;
        let d_node = d_cell.head().noun();
        let d_rest_cell = d_cell.tail().as_cell()?;
        let d_left = d_rest_cell.head().noun();
        let d_right = d_rest_cell.tail().noun();
        let d_key = d_node.in_space(&space).as_cell()?.head().noun();

        if mor_mug(slab, node_key, d_key) {
            Ok(T(slab, &[node, left, d]))
        } else {
            let new_a = T(slab, &[node, left, d_left]);
            Ok(T(slab, &[d_node, new_a, d_right]))
        }
    }
}

fn hoonc_directory_allowed_paths(directory: &Path, files: &[PathBuf]) -> Result<HashSet<String>> {
    let mut allowed = HashSet::with_capacity(files.len());
    for file in files {
        if !file.is_file() {
            continue;
        }
        allowed.insert(hoonc_manifest_relative_path(directory, file)?);
    }
    Ok(allowed)
}

fn hoonc_manifest_relative_path(directory: &Path, file: &Path) -> Result<String> {
    if let Ok(file_path) = file.canonicalize() {
        if let Ok(stripped) = file_path.strip_prefix(directory) {
            return Ok(format!("/{}", stripped.to_string_lossy()));
        }
    }
    if let Ok(stripped) = file.strip_prefix(directory) {
        return Ok(format!("/{}", stripped.to_string_lossy()));
    }

    let directory_parts = normal_path_components(directory);
    let file_parts = normal_path_components(file);
    let max_suffix = directory_parts.len().min(file_parts.len());
    for suffix_len in (1..=max_suffix).rev() {
        let suffix = &directory_parts[directory_parts.len() - suffix_len..];
        for start in 0..=file_parts.len() - suffix_len {
            if &file_parts[start..start + suffix_len] == suffix {
                let rel = &file_parts[start + suffix_len..];
                if !rel.is_empty() {
                    return Ok(format!("/{}", rel.join("/")));
                }
            }
        }
    }

    Err(format!(
        "directory manifest file does not share dependency prefix: {}",
        file.display()
    )
    .into())
}

fn normal_path_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn entry_path_for_hoon(entry: &Path, deps_dir: &Path) -> Result<String> {
    let entry_abs = lexical_absolute_path(entry)?;
    let directory = lexical_absolute_path(deps_dir)?;
    if let Ok(stripped) = entry_abs.strip_prefix(&directory) {
        return Ok(hoon_path_from_relative(stripped));
    }

    let entry_canonical = entry.canonicalize()?;
    let directory_canonical = deps_dir.canonicalize()?;
    if let Ok(stripped) = entry_canonical.strip_prefix(&directory_canonical) {
        return Ok(hoon_path_from_relative(stripped));
    }

    if matching_hoon_root_marker(entry, deps_dir).is_some() {
        if let Some(components) = hoon_relative_components(entry) {
            return Ok(format!("/{}", components.join("/")));
        }
    }

    Ok(entry_canonical.to_string_lossy().into_owned())
}

fn hoon_path_from_relative(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        "/".to_string()
    } else {
        format!("/{}", path.to_string_lossy())
    }
}

fn lexical_absolute_path(path: &Path) -> Result<PathBuf> {
    let mut out = if path.is_absolute() {
        PathBuf::new()
    } else {
        env::current_dir()?
    };
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    Ok(out)
}

fn noun_u64<A: NounAllocator>(allocator: &mut A, value: u64) -> Noun {
    Atom::new(allocator, value).as_noun()
}

fn hoon_path_text_to_noun(slab: &mut NounSlab<NockJammer>, path: &str) -> Result<Noun> {
    let mut out = D(0);
    let segments = path.split('/').filter(|segment| !segment.is_empty());
    for segment in segments.rev() {
        let atom = Atom::from_value(slab, segment.as_bytes())?.as_noun();
        out = T(slab, &[atom, out]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use nockapp::noun::slab::NounSlab;
    use nockvm::noun::{NounAllocator, DIRECT_MAX};

    use crate::{axis_formula, build_entry_wer, entry_path_for_hoon};

    fn temp_test_dir(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("honk-{name}-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path).expect("temp dir");
        path
    }

    fn symlink_file(original: &std::path::Path, link: &std::path::Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(original, link).expect("symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(original, link).expect("symlink");
    }

    #[test]
    fn axis_formula_allocates_atoms_above_the_direct_limit() {
        let mut slab = NounSlab::new();
        let axis = DIRECT_MAX + 1;
        let formula = axis_formula(&mut slab, axis);
        let space = slab.noun_space();
        let formula = formula
            .in_space(&space)
            .as_cell()
            .expect("slot formula must be a cell");
        let encoded_axis = formula
            .tail()
            .as_atom()
            .expect("slot formula axis must be an atom");

        assert!(encoded_axis.is_indirect());
        assert_eq!(encoded_axis.as_u64().expect("axis must fit u64"), axis);
    }

    #[test]
    fn entry_path_for_hoon_keeps_lexical_path_inside_deps() {
        let temp_dir = temp_test_dir("entry-path");
        let deps_dir = temp_dir.join("open").join("hoon");
        let real_dir = temp_dir.join("real");
        fs::create_dir_all(deps_dir.join("apps/peek")).expect("deps dir");
        fs::create_dir_all(&real_dir).expect("real dir");
        let real_file = real_dir.join("peek.hoon");
        fs::write(&real_file, ":: real hoon").expect("real file");
        let entry = deps_dir.join("apps/peek/peek.hoon");
        symlink_file(&real_file, &entry);

        let path = entry_path_for_hoon(&entry, &deps_dir).expect("entry path");
        assert_eq!(path, "/apps/peek/peek.hoon");

        fs::remove_dir_all(temp_dir).expect("cleanup");
    }

    #[test]
    fn build_entry_wer_uses_dependency_relative_path_inside_deps() {
        let temp_dir = temp_test_dir("entry-wer");
        let deps_dir = temp_dir.join("open").join("hoon");
        let real_dir = temp_dir.join("real");
        fs::create_dir_all(deps_dir.join("apps/peek")).expect("deps dir");
        fs::create_dir_all(&real_dir).expect("real dir");
        let real_file = real_dir.join("peek.hoon");
        fs::write(&real_file, ":: real hoon").expect("real file");
        let entry = deps_dir.join("apps/peek/peek.hoon");
        symlink_file(&real_file, &entry);

        let wer = build_entry_wer(&entry, &deps_dir, true);
        assert_eq!(wer, ["apps", "peek", "peek.hoon"]);

        fs::remove_dir_all(temp_dir).expect("cleanup");
    }

    #[test]
    fn entry_paths_match_by_hoon_root_across_sandbox_copies() {
        let temp_dir = temp_test_dir("entry-root-marker");
        let workspace_hoon = temp_dir.join("workspace/open/hoon");
        let sandbox_hoon = temp_dir.join("sandbox/open/hoon");
        fs::create_dir_all(workspace_hoon.join("tests/hoon-compiler")).expect("workspace dir");
        fs::create_dir_all(&sandbox_hoon).expect("sandbox dir");
        let entry = workspace_hoon.join("tests/hoon-compiler/ketcol.hoon");
        fs::write(&entry, ":: real hoon").expect("entry file");

        let path = entry_path_for_hoon(&entry, &sandbox_hoon).expect("entry path");
        assert_eq!(path, "/tests/hoon-compiler/ketcol.hoon");

        let wer = build_entry_wer(&entry, &sandbox_hoon, true);
        assert_eq!(wer, ["tests", "hoon-compiler", "ketcol.hoon"]);

        fs::remove_dir_all(temp_dir).expect("cleanup");
    }
}
