use std::collections::HashSet;
use std::env::current_dir;
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{ColorChoice, Parser};
use nockapp::driver::Operation;
use nockapp::kernel::boot::{self, default_boot_cli, Cli as BootCli};
use nockapp::noun::slab::{Jammer, NockJammer, NounSlab};
use nockapp::one_punch::OnePunchWire;
use nockapp::wire::Wire;
use nockapp::{system_data_dir, AtomExt, Noun};
use nockvm::ext::NounExt;
use nockvm::interpreter::{self, Context};
use nockvm::noun::{Atom, NounAllocator, NounSpace, D, T};
use nockvm_macros::tas;
use tempfile::{NamedTempFile, TempDir};
use tokio::fs::{self, File};
use tokio::io::AsyncReadExt;
use tracing::{debug, info, instrument};
use walkdir::{DirEntry, WalkDir};

pub const OUT_JAM_NAME: &str = "out.jam";

pub type Error = Box<dyn std::error::Error>;

pub static KERNEL_JAM: &[u8] = include_bytes!("../bootstrap/hoonc.jam");
pub static PREWARM_STATE_JAM: &[u8] = include_bytes!("../bootstrap/hoonc-prewarm.jam");
pub static HOON_138_HOON: &[u8] = include_bytes!("../hoon/hoon-138.hoon");
pub static HOON_TXT: &[u8] = HOON_138_HOON;

#[derive(Clone, Parser, Debug)]
#[command(about = "Tests various poke types for the kernel", author = "zorp", version, color = ColorChoice::Auto)]
pub struct HoonCli {
    #[command(flatten)]
    pub boot: BootCli,

    //  TODO: REPRODUCIBILITY:
    //  make entry path relative to the dependency directory
    //  we may have to go back to requiring that the entry exists in the dependency directory
    #[arg(help = "Path to file to compile")]
    pub entry: std::path::PathBuf,

    #[arg(help = "Path to root of dependency directory", default_value = "hoon")]
    pub directory: std::path::PathBuf,

    #[arg(
        long,
        help = "Build raw, without file hash injection",
        default_value = "false",
        conflicts_with_all = ["dynock", "dynock_typed"]
    )]
    pub arbitrary: bool,

    #[arg(
        long,
        help = "Emit minimal dynock artifact [type (trap nock)]",
        default_value = "false",
        conflicts_with_all = ["arbitrary", "dynock_typed"]
    )]
    pub dynock: bool,

    #[arg(
        long,
        help = "Emit typed dynock artifact [inferred-type (trap nock)]",
        default_value = "false",
        conflicts_with_all = ["arbitrary", "dynock"]
    )]
    pub dynock_typed: bool,

    #[arg(long, help = "Output file path", default_value = None)]
    pub output: Option<std::path::PathBuf>,

    #[arg(
        long,
        help = "Only parse and write the parse-cache Hoon AST jam to --output",
        default_value = "false",
        requires = "output",
        conflicts_with_all = ["arbitrary", "dynock", "dynock_typed"]
    )]
    pub parse_only_ast_jam: bool,
}

pub async fn hoonc_data_dir() -> PathBuf {
    let hoonc_data_dir = system_data_dir().join("hoonc");
    if !hoonc_data_dir.exists() {
        fs::create_dir_all(&hoonc_data_dir)
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
    }
    hoonc_data_dir
}

fn dir_has_regular_files(path: &Path) -> bool {
    path.exists()
        && std::fs::read_dir(path)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .any(|entry| entry.file_type().map(|ft| ft.is_file()).unwrap_or(false))
            })
            .unwrap_or(false)
}

fn has_existing_hoonc_durability_state(hoonc_data_dir: &Path) -> bool {
    let checkpoints_dir = hoonc_data_dir.join("checkpoints");
    let pma_dir = hoonc_data_dir.join("pma");

    dir_has_regular_files(&checkpoints_dir)
        || dir_has_regular_files(&pma_dir)
        || hoonc_data_dir.join("event-log.sqlite3").exists()
        || hoonc_data_dir.join("event-log.sqlite3-wal").exists()
        || hoonc_data_dir.join("event-log.sqlite3-shm").exists()
}

/// Builds and interprets a Hoon generator.
///
/// This function:
/// 1. Builds the specified Hoon generator into a jam
/// 2. Decodes the jam into a Nock noun
/// 3. Interprets the noun with a kick operation to run the generator
///
/// # Parameters
/// - `context`: The Nock interpreter context
/// - `path`: Path to the Hoon generator file
///
/// # Returns
/// - A noun
pub async fn build_and_kick_jam(
    context: &mut Context,
    path: &Path,
    deps_dir: PathBuf,
    out_dir: Option<PathBuf>,
) -> Noun {
    let jam = build_jam(path, deps_dir, out_dir, true, false, false, false)
        .await
        .expect("failed to build page");
    debug!("Built jam");
    let generator_trap =
        Noun::cue_bytes_slice(&mut context.stack, &jam).expect("invalid generator jam");

    let kick = T(&mut context.stack, &[D(9), D(2), D(0), D(1)]);
    debug!("Kicking trap");
    interpreter::interpret(context, generator_trap, kick).unwrap_or_else(|_| {
        panic!(
            "Panicked at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    })
}

pub async fn kick_and_save_generator(
    context: &mut Context,
    path: &Path,
    deps_dir: PathBuf,
    out_dir: Option<PathBuf>,
) -> Result<(), Error> {
    let temp_dir = tempfile::TempDir::new()?;
    let out_path = temp_dir.path().join("out.jam");
    let kicked = build_and_kick_jam(context, path, deps_dir, Some(out_path)).await;
    let jammed = kicked.jam_self(&mut context.stack);

    if out_dir.is_some() {
        let file_name = path
            .file_stem()
            .unwrap_or_else(|| OsStr::new("generator"))
            .to_string_lossy()
            .to_string();
        let output_file = out_dir
            .clone()
            .unwrap_or_else(|| current_dir().expect("Failed to get current directory"))
            .join(format!("{}.jam", file_name));

        if let Some(parent) = output_file.parent() {
            fs::create_dir_all(parent).await?;
        }

        fs::write(&output_file, jammed).await?;

        info!("Generator saved to: {}", output_file.display());
    }
    Ok(())
}
/// Builds a jam (serialized Nock noun) from a Hoon source file
///
/// This function:
/// 1. Locates the source file relative to the hoon directory
/// 2. Creates a temporary directory for build artifacts
/// 3. Initializes a Nock app with the hoonc build system
/// 4. Builds the source file and returns the resulting jam as bytes
///
/// # Parameters
/// - `entry`: Path to the Hoon source file, relative to the hoon directory
/// - `deps_dir`: Path to the dependencies directory
/// - `out_dir`: Optional path to the output directory
/// - `arbitrary`: Whether to build with arbitrary mode enabled
/// - `dynock`: Whether to emit minimal dynock output ([type (trap nock)])
/// - `dynock_typed`: Whether to emit typed dynock output ([inferred-type (trap nock)])
/// - `new`: Whether to force a clean build
///
/// # Returns
/// - A Result containing either the jam bytes or a hoonc error
pub async fn build_jam(
    entry: &Path,
    deps_dir: PathBuf,
    out_dir: Option<PathBuf>,
    arbitrary: bool,
    dynock: bool,
    dynock_typed: bool,
    new: bool,
) -> Result<Vec<u8>, Error> {
    info!("Dependencies directory: {:?}", deps_dir);
    info!("Entry file: {:?}", entry);
    let (nockapp, out_path) = initialize_with_default_cli(
        entry.to_path_buf(),
        deps_dir,
        out_dir,
        arbitrary,
        dynock,
        dynock_typed,
        new,
    )
    .await?;
    info!("Output path: {:?}", out_path);
    run_build(nockapp, Some(out_path.clone())).await
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

fn jam_copied_noun(noun: Noun, space: &NounSpace) -> Vec<u8> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let root = slab.copy_into(noun, space);
    slab.set_root(root);
    slab.jam().to_vec()
}

fn expect_cell(noun: Noun, context: &str, space: &NounSpace) -> Result<(Noun, Noun), Error> {
    let cell = noun.in_space(space).as_cell().map_err(|_| -> Error {
        Box::new(std::io::Error::other(format!("{context}: expected cell")))
    })?;
    Ok((cell.head().noun(), cell.tail().noun()))
}

fn tuple3_opt(noun: Noun, space: &NounSpace) -> Option<(Noun, Noun, Noun)> {
    let cell = noun.in_space(space).as_cell().ok()?;
    let first = cell.head().noun();
    let rest = cell.tail().as_cell().ok()?;
    Some((first, rest.head().noun(), rest.tail().noun()))
}

fn tuple4_opt(noun: Noun, space: &NounSpace) -> Option<(Noun, Noun, Noun, Noun)> {
    let cell = noun.in_space(space).as_cell().ok()?;
    let first = cell.head().noun();
    let rest = cell.tail().as_cell().ok()?;
    let second = rest.head().noun();
    let rest = rest.tail().as_cell().ok()?;
    Some((first, second, rest.head().noun(), rest.tail().noun()))
}

fn atom_string(noun: Noun, space: &NounSpace) -> Option<String> {
    let atom = noun.in_space(space).as_atom().ok()?;
    let text = atom.into_string().ok()?;
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn path_noun_to_string(noun: Noun, space: &NounSpace) -> Result<String, Error> {
    let mut parts = Vec::new();
    let mut cursor = noun;
    loop {
        if cursor
            .in_space(space)
            .as_atom()
            .ok()
            .and_then(|atom| atom.as_u64().ok())
            == Some(0)
        {
            break;
        }
        let (head, tail) = expect_cell(cursor, "path", space)?;
        let segment = atom_string(head, space).ok_or_else(|| -> Error {
            Box::new(std::io::Error::other("path segment decode failed"))
        })?;
        parts.push(segment);
        cursor = tail;
    }
    Ok(format!("/{}", parts.join("/")))
}

fn pile_hoon(pil: Noun, space: &NounSpace) -> Result<Noun, Error> {
    let (_, rest) = expect_cell(pil, "pile", space)?;
    let (_, rest) = expect_cell(rest, "pile", space)?;
    let (_, rest) = expect_cell(rest, "pile", space)?;
    let (_, rest) = expect_cell(rest, "pile", space)?;
    let (_, hoon) = expect_cell(rest, "pile", space)?;
    Ok(hoon)
}

fn noun_is_zero(noun: Noun, space: &NounSpace) -> bool {
    noun.in_space(space)
        .as_atom()
        .ok()
        .and_then(|atom| atom.as_u64().ok())
        .map(|value| value == 0)
        .unwrap_or(false)
}

fn map_find_entry_by_path(
    map: Noun,
    target: &str,
    space: &NounSpace,
) -> Option<(Noun, Noun, Noun)> {
    if noun_is_zero(map, space) {
        return None;
    }
    let map_cell = map.in_space(space).as_cell().ok()?;
    let node = map_cell.head().noun();
    let branches = map_cell.tail().as_cell().ok()?;
    let left = branches.head().noun();
    let right = branches.tail().noun();

    let node_cell = node.in_space(space).as_cell().ok()?;
    let key = node_cell.head().noun();
    let value = node_cell.tail().noun();
    let (path, pil, deps) = tuple3_opt(value, space)?;
    let path_string = path_noun_to_string(path, space).ok()?;
    if path_string == target {
        return Some((key, pil, deps));
    }

    map_find_entry_by_path(left, target, space)
        .or_else(|| map_find_entry_by_path(right, target, space))
}

fn is_state3_tag(noun: Noun, space: &NounSpace) -> bool {
    noun.in_space(space)
        .as_atom()
        .ok()
        .and_then(|atom| atom.as_u64().ok())
        .map(|value| value == 3)
        .unwrap_or_else(|| atom_string(noun, space).as_deref() == Some("3"))
}

fn parse_cache_from_state(state: Noun, space: &NounSpace) -> Option<Noun> {
    let mut stack = vec![state];
    let mut seen = HashSet::new();
    while let Some(noun) = stack.pop() {
        let raw = unsafe { noun.as_raw() };
        if !seen.insert(raw) {
            continue;
        }
        let Ok(cell) = noun.in_space(space).as_cell() else {
            continue;
        };
        if is_state3_tag(cell.head().noun(), space) {
            let (_, rest) = expect_cell(noun, "state", space).ok()?;
            let (_, rest) = expect_cell(rest, "state", space).ok()?;
            let (_, parse_cache) = expect_cell(rest, "state", space).ok()?;
            return Some(parse_cache);
        }
        stack.push(cell.head().noun());
        stack.push(cell.tail().noun());
    }
    None
}

fn parsed_hoon_from_state(
    state: Noun,
    targets: &[String],
    space: &NounSpace,
) -> Result<Noun, Error> {
    let suffix_targets = targets
        .iter()
        .map(|target| target.trim_start_matches('/').to_string())
        .collect::<Vec<_>>();
    let mut stack = vec![state];
    let mut seen = std::collections::HashSet::new();
    while let Some(noun) = stack.pop() {
        let raw = unsafe { noun.as_raw() };
        if !seen.insert(raw) {
            continue;
        }

        if let Some((path, pil, _deps)) = tuple3_opt(noun, space) {
            if let Ok(path_string) = path_noun_to_string(path, space) {
                if targets.iter().any(|target| target == &path_string)
                    || suffix_targets
                        .iter()
                        .any(|suffix| path_string.ends_with(suffix))
                {
                    if let Ok(hoon) = pile_hoon(pil, space) {
                        return Ok(hoon);
                    }
                }
            }
        }

        if let Some((path, _fil, pil, _deps)) = tuple4_opt(noun, space) {
            if let Ok(path_string) = path_noun_to_string(path, space) {
                if targets.iter().any(|target| target == &path_string)
                    || suffix_targets
                        .iter()
                        .any(|suffix| path_string.ends_with(suffix))
                {
                    if let Ok(hoon) = pile_hoon(pil, space) {
                        return Ok(hoon);
                    }
                }
            }
        }

        if let Ok(cell) = noun.in_space(space).as_cell() {
            stack.push(cell.head().noun());
            stack.push(cell.tail().noun());
        }
    }

    Err(Box::new(std::io::Error::other(format!(
        "parsed hoon not found in state for any of {:?}",
        targets
    ))))
}

fn build_parse_targets(entry: &Path, deps_dir: &Path) -> Result<Vec<String>, Error> {
    let entry_string = entry_path_for_hoon(entry, deps_dir)?;
    let entry_abs = entry.canonicalize()?.to_string_lossy().replace('\\', "/");
    let mut targets = Vec::new();
    push_unique_target(&mut targets, entry_string);
    push_unique_target(&mut targets, entry_abs);
    for alias in content_equivalent_dependency_targets(entry, deps_dir)? {
        push_unique_target(&mut targets, alias);
    }
    if let Some(file_name) = entry.file_name().and_then(|name| name.to_str()) {
        push_unique_target(&mut targets, file_name.to_string());
    }
    Ok(targets)
}

fn push_unique_target(targets: &mut Vec<String>, target: String) {
    if !targets.iter().any(|candidate| candidate == &target) {
        targets.push(target);
    }
}

fn content_equivalent_dependency_targets(
    entry: &Path,
    deps_dir: &Path,
) -> Result<Vec<String>, Error> {
    let entry_contents = std::fs::read(entry)?;
    let deps_abs = absolute_path(deps_dir)?;
    let mut targets = Vec::new();
    for entry_result in WalkDir::new(&deps_abs)
        .follow_links(true)
        .into_iter()
        .filter_entry(is_valid_file_or_dir)
    {
        let dependency = entry_result?;
        if !dependency.metadata()?.is_file() {
            continue;
        }
        if std::fs::read(dependency.path())? != entry_contents {
            continue;
        }
        let Ok(relative) = dependency.path().strip_prefix(&deps_abs) else {
            continue;
        };
        push_unique_target(&mut targets, hoon_path_from_relative(relative));
    }
    Ok(targets)
}

async fn parse_state_with_hoonc(
    entry: &Path,
    deps_dir: &Path,
    new: bool,
) -> Result<NounSlab<NockJammer>, Error> {
    let nockapp_home = TempDir::new()?;
    let home_guard = EnvVarGuard::set("NOCKAPP_HOME", &nockapp_home.path().to_string_lossy());
    let _ = &home_guard;
    let prewarm_guard = EnvVarGuard::set("HOONC_DISABLE_PREWARM", "1");
    let _ = &prewarm_guard;

    let boot_cli = default_boot_cli(new);
    let (mut nockapp, _out_path) = initialize_hoonc_(
        entry.to_path_buf(),
        deps_dir.to_path_buf(),
        false,
        false,
        false,
        None,
        boot_cli,
    )
    .await?;

    let entry_string = entry_path_for_hoon(entry, deps_dir)?;
    let entry_contents = fs::read(entry).await?;

    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let entry_path_noun = Atom::from_value(&mut slab, entry_string)?.as_noun();
    let entry_contents_noun = Atom::from_value(&mut slab, entry_contents)?.as_noun();
    let directory_noun = build_directory_noun(&mut slab, deps_dir).await?;
    let parse_poke = T(
        &mut slab,
        &[D(tas!(b"parse")), entry_path_noun, entry_contents_noun, directory_noun],
    );
    slab.set_root(parse_poke);

    nockapp.poke(OnePunchWire::Poke.to_wire(), slab).await?;
    let checkpoint = nockapp.checkpoint().await?;
    Ok(checkpoint.state)
}

pub async fn parse_cache_ast_jam(
    entry: PathBuf,
    deps_dir: PathBuf,
    new: bool,
) -> Result<Vec<u8>, Error> {
    let state_slab = parse_state_with_hoonc(&entry, &deps_dir, new).await?;
    let state_space = state_slab.noun_space();
    let state_root = unsafe { *state_slab.root() };
    let entry_path = entry_path_for_hoon(&entry, &deps_dir)?;
    if let Some(parse_cache) = parse_cache_from_state(state_root, &state_space) {
        if let Some((_, pil, _)) = map_find_entry_by_path(parse_cache, &entry_path, &state_space) {
            return Ok(jam_copied_noun(pile_hoon(pil, &state_space)?, &state_space));
        }
    }
    let targets = build_parse_targets(&entry, &deps_dir)?;
    let hoon = parsed_hoon_from_state(state_root, &targets, &state_space)?;
    Ok(jam_copied_noun(hoon, &state_space))
}

pub async fn export_parse_cache_ast_jam_if_missing(
    entry: PathBuf,
    deps_dir: PathBuf,
    out_path: PathBuf,
    new: bool,
) -> Result<PathBuf, Error> {
    if out_path.exists() {
        return Ok(out_path);
    }
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let jam = parse_cache_ast_jam(entry, deps_dir, new).await?;
    fs::write(&out_path, jam).await?;
    Ok(out_path)
}

pub async fn initialize_hoonc(cli: HoonCli) -> Result<(nockapp::NockApp, PathBuf), Error> {
    initialize_hoonc_(
        cli.entry,
        cli.directory,
        cli.arbitrary,
        cli.dynock,
        cli.dynock_typed,
        cli.output,
        cli.boot.clone(),
    )
    .await
}

pub async fn initialize_hoonc_with_cli<J: Jammer + Send + 'static>(
    cli: HoonCli,
) -> Result<(nockapp::NockApp<J>, PathBuf), Error> {
    initialize_hoonc_inner(
        cli.entry,
        cli.directory,
        cli.arbitrary,
        cli.dynock,
        cli.dynock_typed,
        cli.output,
        cli.boot.clone(),
    )
    .await
}

pub async fn initialize_with_default_cli(
    entry: std::path::PathBuf,
    deps_dir: std::path::PathBuf,
    out: Option<std::path::PathBuf>,
    arbitrary: bool,
    dynock: bool,
    dynock_typed: bool,
    new: bool,
) -> Result<(nockapp::NockApp, PathBuf), Error> {
    let cli = default_boot_cli(new);
    initialize_hoonc_(entry, deps_dir, arbitrary, dynock, dynock_typed, out, cli).await
}

async fn build_directory_noun(
    slab: &mut NounSlab<NockJammer>,
    deps_dir: &Path,
) -> Result<Noun, Error> {
    let directory = canonicalize_and_string(deps_dir);
    let mut directory_noun = D(0);
    let walker = WalkDir::new(&directory).follow_links(true).into_iter();

    for entry_result in walker.filter_entry(is_valid_file_or_dir) {
        let entry = entry_result?;
        if !entry.metadata()?.is_file() {
            continue;
        }

        let path_str = entry
            .path()
            .to_str()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "dependency path contains invalid UTF-8",
                )
            })?
            .strip_prefix(&directory)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "dependency path does not share base prefix",
                )
            })?;
        debug!("Path: {:?}", path_str);

        let path_cord = Atom::from_value(slab, path_str)?.as_noun();
        let mut contents_vec: Vec<u8> = vec![];
        let mut file = File::open(entry.path()).await?;
        file.read_to_end(&mut contents_vec).await?;
        let contents = Atom::from_value(slab, contents_vec)?.as_noun();

        let entry_cell = T(slab, &[path_cord, contents]);
        directory_noun = T(slab, &[entry_cell, directory_noun]);
    }

    Ok(directory_noun)
}

async fn initialize_hoonc_inner<J: Jammer + Send + 'static>(
    entry: std::path::PathBuf,
    deps_dir: std::path::PathBuf,
    arbitrary: bool,
    dynock: bool,
    dynock_typed: bool,
    out: Option<std::path::PathBuf>,
    boot_cli: BootCli,
) -> Result<(nockapp::NockApp<J>, PathBuf), Error> {
    let mode_count = (arbitrary as u8) + (dynock as u8) + (dynock_typed as u8);
    if mode_count > 1 {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--arbitrary, --dynock, and --dynock-typed are mutually exclusive",
        )));
    }
    debug!("Dependencies directory: {:?}", deps_dir);
    debug!("Entry file: {:?}", entry);
    let data_dir = system_data_dir();
    let mut boot_cli = boot_cli;
    if let Ok(raw) = std::env::var("HOONC_NOCK_STACK_SIZE") {
        let normalized = raw.trim().to_ascii_lowercase();
        boot_cli.stack_size = match normalized.as_str() {
            "tiny" => boot::NockStackSize::Tiny,
            "small" => boot::NockStackSize::Small,
            "normal" => boot::NockStackSize::Normal,
            "medium" => boot::NockStackSize::Medium,
            "large" => boot::NockStackSize::Large,
            "huge" => boot::NockStackSize::Huge,
            _ => {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "invalid HOONC_NOCK_STACK_SIZE={raw:?} (expected tiny|small|normal|medium|large|huge)"
                    ),
                )));
            }
        };
    }
    let disable_prewarm = std::env::var("HOONC_DISABLE_PREWARM").is_ok();
    let hoonc_data_dir = data_dir.join("hoonc");
    let has_existing_durability_state = has_existing_hoonc_durability_state(&hoonc_data_dir);

    let should_use_prewarm = !disable_prewarm
        && boot_cli.state_jam.is_none()
        && (boot_cli.new || !has_existing_durability_state);

    // Keep the prewarm tempfile alive for the duration of this function when used.
    let mut _prewarm_state_file: Option<NamedTempFile> = None;
    if should_use_prewarm {
        let mut tmp = NamedTempFile::new().map_err(|err| -> Error {
            Box::new(std::io::Error::other(format!(
                "prewarm tempfile create failed: {err}"
            )))
        })?;
        tmp.write_all(PREWARM_STATE_JAM).map_err(|err| -> Error {
            Box::new(std::io::Error::other(format!(
                "prewarm tempfile write failed: {err}"
            )))
        })?;
        boot_cli.new = true;
        boot_cli.state_jam = Some(tmp.path().to_string_lossy().into_owned());
        _prewarm_state_file = Some(tmp);
    }
    let mut nockapp = boot::setup::<J>(KERNEL_JAM, boot_cli.clone(), &[], "hoonc", Some(data_dir))
        .await
        .map_err(|err| -> Error {
            Box::new(std::io::Error::other(format!("boot setup failed: {err}")))
        })?;
    nockapp.add_io_driver(nockapp::file_driver()).await;
    nockapp.add_io_driver(nockapp::exit_driver()).await;

    let mut boot_slab = NounSlab::new();
    let hoon_cord = Atom::from_value(&mut boot_slab, HOON_TXT)
        .unwrap_or_else(|_| {
            panic!(
                "Panicked at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        })
        .as_noun();
    let bootstrap_poke = T(&mut boot_slab, &[D(tas!(b"boot")), hoon_cord]);
    boot_slab.set_root(bootstrap_poke);

    // It's OK to do a raw poke for boot because it doesn't yield any effects that need to be processed.
    // We do a raw poke here to ensure boot is done before we start the build poke.
    let _boot_result = nockapp
        .poke(OnePunchWire::Poke.to_wire(), boot_slab)
        .await?;
    let mut slab: NounSlab<NockJammer> = NounSlab::new();

    let entry_string = entry_path_for_hoon(&entry, &deps_dir)?;
    let entry_path = Atom::from_value(&mut slab, entry_string)?.as_noun();

    let directory_noun =
        build_directory_noun(&mut slab, &deps_dir)
            .await
            .map_err(|err| -> Error {
                Box::new(std::io::Error::other(format!(
                    "build directory noun failed for {}: {err}",
                    deps_dir.display()
                )))
            })?;

    let entry_contents = {
        let mut contents_vec: Vec<u8> = vec![];
        let mut file = File::open(&entry).await.map_err(|err| -> Error {
            Box::new(std::io::Error::other(format!(
                "entry open failed for {}: {err}",
                entry.display()
            )))
        })?;
        file.read_to_end(&mut contents_vec)
            .await
            .map_err(|err| -> Error {
                Box::new(std::io::Error::other(format!(
                    "entry read failed for {}: {err}",
                    entry.display()
                )))
            })?;
        Atom::from_value(&mut slab, contents_vec)?.as_noun()
    };

    let out_path_string = if let Some(path) = &out {
        let parent = if path.is_dir() {
            path
        } else {
            &current_dir().expect("Failed to get current directory")
        };
        let filename = if path.is_dir() {
            OsStr::new(OUT_JAM_NAME)
        } else {
            path.file_name().unwrap_or_else(|| OsStr::new(OUT_JAM_NAME))
        };
        let parent_canonical = canonicalize_and_string(parent);
        format!("{}/{}", parent_canonical, filename.to_string_lossy())
    } else {
        let parent_dir = current_dir().expect("Failed to get current directory");
        format!("{}/{}", canonicalize_and_string(&parent_dir), OUT_JAM_NAME)
    };
    debug!("Output path: {:?}", out_path_string);
    let out_path = Atom::from_value(&mut slab, out_path_string.clone())?.as_noun();

    // Keep the `%build` cause ABI stable for older kernel snapshots by encoding
    // dynock_typed through the existing two mode bits:
    //   arb=1,dyn=1 => dynock_typed
    //   arb=1,dyn=0 => arbitrary
    //   arb=0,dyn=1 => dynock
    //   arb=0,dyn=0 => standard
    let encoded_arbitrary = arbitrary || dynock_typed;
    let encoded_dynock = dynock || dynock_typed;
    let arbitrary_flag = if encoded_arbitrary { D(0) } else { D(1) };
    let dynock_flag = if encoded_dynock { D(0) } else { D(1) };

    // Older bundled kernels only understand legacy `%build` payloads that do
    // not include the dynock bit. Use that shape when dynock is not requested
    // so bootstrapping can proceed without immediately regenerating bootstrap
    // jams. Dynock modes still use the extended payload.
    let poke = if encoded_dynock {
        T(
            &mut slab,
            &[
                D(tas!(b"build")),
                entry_path,
                entry_contents,
                directory_noun,
                arbitrary_flag,
                dynock_flag,
                out_path,
            ],
        )
    } else {
        T(
            &mut slab,
            &[
                D(tas!(b"build")),
                entry_path,
                entry_contents,
                directory_noun,
                arbitrary_flag,
                out_path,
            ],
        )
    };
    slab.set_root(poke);
    // The build poke yields effects (principally the file write effect), so we need to embed the poke
    // as a one_punch IO driver so that nockapp.run() can process the effects.
    nockapp
        .add_io_driver(nockapp::one_punch_driver(slab, Operation::Poke))
        .await;
    Ok((nockapp, out_path_string.into()))
}

pub async fn initialize_hoonc_with_jammer<J: Jammer + Send + 'static>(
    entry: std::path::PathBuf,
    deps_dir: std::path::PathBuf,
    arbitrary: bool,
    dynock: bool,
    dynock_typed: bool,
    out: Option<std::path::PathBuf>,
    boot_cli: BootCli,
) -> Result<(nockapp::NockApp<J>, PathBuf), Error> {
    initialize_hoonc_inner(
        entry, deps_dir, arbitrary, dynock, dynock_typed, out, boot_cli,
    )
    .await
}

pub async fn initialize_hoonc_(
    entry: std::path::PathBuf,
    deps_dir: std::path::PathBuf,
    arbitrary: bool,
    dynock: bool,
    dynock_typed: bool,
    out: Option<std::path::PathBuf>,
    boot_cli: BootCli,
) -> Result<(nockapp::NockApp, PathBuf), Error> {
    initialize_hoonc_with_jammer::<NockJammer>(
        entry, deps_dir, arbitrary, dynock, dynock_typed, out, boot_cli,
    )
    .await
}

const BLACKLISTED_DIRS: &[&str] = &["packages", "node_modules", ".git", "target"];

pub fn is_valid_file_or_dir(entry: &DirEntry) -> bool {
    let metadata = entry.metadata().unwrap_or_else(|_| {
        panic!(
            "Panicked at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    });

    let is_dir = metadata.is_dir();
    let file_name = entry.file_name().to_str().unwrap_or("");

    // Skip blacklisted directories
    if is_dir && BLACKLISTED_DIRS.contains(&file_name) {
        return false;
    }

    // Whitelist valid file extensions
    let is_valid_file = entry
        .file_name()
        .to_str()
        .map(|s| {
            s.ends_with(".jock")
                || s.ends_with(".hoon")
                || s.ends_with(".txt")
                || s.ends_with(".jam")
                || s.ends_with(".html")
                || s.ends_with(".css")
                || s.ends_with(".js")
                || s.ends_with(".jpg")
                || s.ends_with(".png")
                || s.ends_with(".gif")
        })
        .unwrap_or(false);

    is_dir || is_valid_file
}

fn entry_path_for_hoon(entry: &Path, deps_dir: &Path) -> Result<String, Error> {
    let entry_abs = absolute_path(entry)?;
    let deps_abs = absolute_path(deps_dir)?;
    if let Ok(rel) = entry_abs.strip_prefix(&deps_abs) {
        return Ok(hoon_path_from_relative(rel));
    }

    let entry_canonical = entry.canonicalize()?;
    let deps_canonical = deps_dir.canonicalize()?;
    if let Ok(rel) = entry_canonical.strip_prefix(&deps_canonical) {
        Ok(hoon_path_from_relative(rel))
    } else {
        Ok(entry_canonical.to_string_lossy().into_owned())
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, Error> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(current_dir()?.join(path))
    }
}

fn hoon_path_from_relative(path: &Path) -> String {
    let rel_str = path.to_string_lossy();
    if rel_str.starts_with('/') {
        rel_str.to_string()
    } else {
        format!("/{rel_str}")
    }
}

#[cfg(test)]
mod durability_state_tests {
    use super::has_existing_hoonc_durability_state;

    #[test]
    fn detects_existing_pma_state_without_checkpoints() {
        let temp = tempfile::tempdir().expect("tempdir");
        let hoonc_data_dir = temp.path().join("hoonc");
        std::fs::create_dir_all(hoonc_data_dir.join("pma")).expect("create pma dir");
        std::fs::write(hoonc_data_dir.join("pma").join("0.pma"), b"pma").expect("write pma");

        assert!(has_existing_hoonc_durability_state(&hoonc_data_dir));
    }

    #[test]
    fn detects_existing_event_log_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let hoonc_data_dir = temp.path().join("hoonc");
        std::fs::create_dir_all(&hoonc_data_dir).expect("create hoonc dir");
        std::fs::write(hoonc_data_dir.join("event-log.sqlite3"), b"sqlite")
            .expect("write event log");

        assert!(has_existing_hoonc_durability_state(&hoonc_data_dir));
    }

    #[test]
    fn empty_hoonc_data_dir_does_not_block_prewarm() {
        let temp = tempfile::tempdir().expect("tempdir");
        let hoonc_data_dir = temp.path().join("hoonc");
        std::fs::create_dir_all(&hoonc_data_dir).expect("create hoonc dir");

        assert!(!has_existing_hoonc_durability_state(&hoonc_data_dir));
    }
}
#[instrument]
pub fn canonicalize_and_string(path: &std::path::Path) -> String {
    let path = path.canonicalize().expect("Failed to canonicalize path");
    let path = path.to_str().expect("Failed to convert path to string");
    path.to_string()
}

/// Run the build and verify the output file, used to build files outside of cli.
pub async fn run_build(
    mut nockapp: nockapp::NockApp,
    out_path: Option<PathBuf>,
) -> Result<Vec<u8>, Error> {
    nockapp.run().await?;
    let out_path = out_path.unwrap_or_else(|| {
        std::env::current_dir()
            .unwrap_or_else(|_| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })
            .join(OUT_JAM_NAME)
    });
    Ok(fs::read(out_path).await?)
}

#[cfg(test)]
mod tests {
    use std::fs;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn entry_path_for_hoon_keeps_symlink_path_inside_deps() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let deps_dir = temp_dir.path().join("deps");
        let real_dir = temp_dir.path().join("real");
        fs::create_dir_all(deps_dir.join("tests")).expect("deps tests dir");
        fs::create_dir_all(&real_dir).expect("real dir");
        let real_file = real_dir.join("hoon-138.hoon");
        fs::write(&real_file, ":: real hoon").expect("real file");
        let symlink_file = deps_dir.join("tests").join("hoon_138.hoon");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_file, &symlink_file).expect("symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&real_file, &symlink_file).expect("symlink");

        let path = super::entry_path_for_hoon(&symlink_file, &deps_dir).expect("entry path");
        assert_eq!(path, "/tests/hoon_138.hoon");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn build_parse_targets_includes_duplicate_dependency_content() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let deps_dir = temp_dir.path().join("deps");
        fs::create_dir_all(deps_dir.join("tests")).expect("deps tests dir");
        let entry = deps_dir.join("tests").join("entry.hoon");
        let alias = deps_dir.join("tests").join("alias.hoon");
        fs::write(&entry, ":: same hoon").expect("entry file");
        fs::write(&alias, ":: same hoon").expect("alias file");

        let targets = super::build_parse_targets(&entry, &deps_dir).expect("parse targets");
        assert!(targets.iter().any(|target| target == "/tests/entry.hoon"));
        assert!(targets.iter().any(|target| target == "/tests/alias.hoon"));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn entry_path_for_hoon_falls_back_to_canonical_external_path() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let deps_dir = temp_dir.path().join("deps");
        let external_dir = temp_dir.path().join("external");
        fs::create_dir_all(&deps_dir).expect("deps dir");
        fs::create_dir_all(&external_dir).expect("external dir");
        let external_file = external_dir.join("entry.hoon");
        fs::write(&external_file, ":: external hoon").expect("external file");

        let path = super::entry_path_for_hoon(&external_file, &deps_dir).expect("entry path");
        let external = external_file
            .canonicalize()
            .expect("canonical external")
            .to_string_lossy()
            .to_string();
        assert_eq!(path, external);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_canonicalize_and_string() {
        // Create a temp dir that will definitely exist
        let temp_dir = std::env::temp_dir();

        // Use canonicalize_and_string on the temp dir
        let result = super::canonicalize_and_string(&temp_dir);

        // Compare with direct canonicalization
        let canonical = temp_dir.canonicalize().unwrap_or_else(|_| {
            panic!(
                "Panicked at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        assert_eq!(
            result,
            canonical.to_str().unwrap_or_else(|| {
                panic!(
                    "Panicked at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            })
        );
    }
}
