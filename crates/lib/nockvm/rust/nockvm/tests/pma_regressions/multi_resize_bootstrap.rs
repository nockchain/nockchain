use std::any::Any;
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use nockvm::mem::NockStack;
use nockvm::noun::{Cell, Noun, NounSpace, D};
use nockvm::pma::{Pma, PmaCopy};

const INITIAL_PMA_WORDS: usize = 1024;
const CELL_WORDS: usize = 3;
const TARGET_PMA_WORDS: usize = INITIAL_PMA_WORDS * 10;
const LIST_CELLS: usize = (TARGET_PMA_WORDS / CELL_WORDS) + 1;
const EXPECTED_ALLOC_WORDS: usize = LIST_CELLS * CELL_WORDS;
const STACK_WORDS: usize = EXPECTED_ALLOC_WORDS * 2 + 1024;
const MIN_GROWTH_EVENTS: usize = 2;
const MAX_REASONABLE_FINAL_WORDS: usize = INITIAL_PMA_WORDS * 32;
const GROWTH_EVENTS_ENV: &str = "NOCK_PMA_GROWTH_EVENTS_PATH";

pub(crate) fn run_regression() -> Result<(), Box<dyn Error>> {
    nockvm::check_endian();
    run()
}

fn run() -> Result<(), Box<dyn Error>> {
    assert_eq!(
        std::mem::size_of::<nockvm::noun::CellMemory>(),
        CELL_WORDS * std::mem::size_of::<u64>(),
        "regression assumes one direct-atom list cell copies to exactly {CELL_WORDS} PMA words"
    );

    let workspace = Workspace::new("pma-multi-resize-bootstrap-regression")?;
    let pma_path = workspace.path().join("bootstrap.pma");
    let growth_events_path = workspace.path().join("growth-events.log");
    let _growth_events_guard = EnvVarGuard::set(GROWTH_EVENTS_ENV, growth_events_path.as_os_str());

    println!(
        "building bootstrap noun: initial_pma_words={INITIAL_PMA_WORDS} target_words={TARGET_PMA_WORDS} list_cells={LIST_CELLS} expected_alloc_words={EXPECTED_ALLOC_WORDS} stack_words={STACK_WORDS}"
    );
    let mut stack = NockStack::new(STACK_WORDS, 0);
    let source_root = build_direct_atom_list(&mut stack, LIST_CELLS);

    let mut pma = Pma::new(INITIAL_PMA_WORDS, pma_path.clone())?;
    let mut pma_root = source_root;
    println!(
        "copying bootstrap noun into PMA: path={} size_words={} alloc_words={} free_words={}",
        pma_path.display(),
        pma.size_words(),
        pma.alloc_offset(),
        pma.free_words()
    );

    let copy_result = catch_unwind_message(|| unsafe {
        pma_root.copy_to_pma(&stack, &mut pma);
    });
    if let Err(message) = copy_result {
        return Err(std::io::Error::other(format!(
            "bootstrap copy failed before automatic multi-growth could complete: {message}"
        ))
        .into());
    }

    let growth_events = read_growth_events(&growth_events_path)?;
    println!(
        "bootstrap copy completed: final_size_words={} alloc_words={} free_words={} growth_events={}",
        pma.size_words(),
        pma.alloc_offset(),
        pma.free_words(),
        growth_events.len()
    );
    for event in &growth_events {
        println!("pma growth event: {event}");
    }

    if growth_events.len() < MIN_GROWTH_EVENTS {
        return Err(std::io::Error::other(format!(
            "expected at least {MIN_GROWTH_EVENTS} automatic PMA growth events while copying a noun more than 10x the initial capacity, got {}. Implement growable PMA instrumentation by appending one line per growth to ${GROWTH_EVENTS_ENV}.",
            growth_events.len()
        ))
        .into());
    }
    if pma.size_words() < EXPECTED_ALLOC_WORDS {
        return Err(std::io::Error::other(format!(
            "PMA did not grow enough for copied noun: final_size_words={} expected_alloc_words={EXPECTED_ALLOC_WORDS}",
            pma.size_words()
        ))
        .into());
    }
    if pma.size_words() > MAX_REASONABLE_FINAL_WORDS {
        return Err(std::io::Error::other(format!(
            "PMA grew to an unreasonable current capacity: final_size_words={} max_reasonable_words={MAX_REASONABLE_FINAL_WORDS}. The regression expects on-demand growth, not materializing a huge max reservation as current file capacity.",
            pma.size_words()
        ))
        .into());
    }
    if pma.alloc_offset() != EXPECTED_ALLOC_WORDS {
        return Err(std::io::Error::other(format!(
            "copied noun used unexpected PMA words: alloc_words={} expected_alloc_words={EXPECTED_ALLOC_WORDS}",
            pma.alloc_offset()
        ))
        .into());
    }

    verify_pma_list(pma_root, &pma, LIST_CELLS)?;
    pma.sync_all()?;
    pma.sync_file()?;
    assert_reasonable_file_len(&pma_path, pma.size_words())?;
    drop(pma);

    let reopened = Pma::open(pma_path.clone())?;
    println!(
        "reopened PMA: path={} size_words={} alloc_words={} free_words={}",
        pma_path.display(),
        reopened.size_words(),
        reopened.alloc_offset(),
        reopened.free_words()
    );
    verify_pma_list(pma_root, &reopened, LIST_CELLS)?;
    assert_eq!(
        reopened.alloc_offset(),
        EXPECTED_ALLOC_WORDS,
        "reopened PMA should preserve allocation cursor"
    );
    assert_reasonable_file_len(&pma_path, reopened.size_words())?;

    println!(
        "PMA automatically grew multiple times while bootstrapping a noun over 10x initial capacity, and final PMA noun content verified"
    );
    Ok(())
}

fn build_direct_atom_list(stack: &mut NockStack, len: usize) -> Noun {
    let mut noun = D(0);
    for value in (0..len).rev() {
        noun = Cell::new(stack, D(value as u64), noun).as_noun();
    }
    noun
}

fn verify_pma_list(root: Noun, pma: &Pma, expected_len: usize) -> Result<(), Box<dyn Error>> {
    let space = NounSpace::pma_only(pma);
    let mut cursor = root.in_space(&space);
    for expected in 0..expected_len {
        let cell = cursor.as_cell().map_err(|err| {
            std::io::Error::other(format!(
                "expected PMA list cell at index {expected}, got non-cell: {err:?}"
            ))
        })?;
        let actual = cell.head().as_atom()?.as_u64()?;
        if actual != expected as u64 {
            return Err(std::io::Error::other(format!(
                "PMA list content mismatch at index {expected}: actual={actual} expected={expected}"
            ))
            .into());
        }
        cursor = cell.tail();
    }
    let terminator = cursor.as_atom()?.as_u64()?;
    if terminator != 0 {
        return Err(std::io::Error::other(format!(
            "PMA list terminator mismatch: actual={terminator} expected=0"
        ))
        .into());
    }
    Ok(())
}

fn read_growth_events(path: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(fs::read_to_string(path)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

fn assert_reasonable_file_len(path: &Path, current_words: usize) -> Result<(), Box<dyn Error>> {
    let file_len = fs::metadata(path)?.len();
    let expected_capacity_bytes = (current_words as u64)
        .checked_mul(8)
        .ok_or_else(|| std::io::Error::other("current PMA words overflow bytes"))?;
    let max_reasonable_len = expected_capacity_bytes
        .checked_add(1024 * 1024)
        .ok_or_else(|| std::io::Error::other("reasonable PMA file length overflowed"))?;
    if file_len > max_reasonable_len {
        return Err(std::io::Error::other(format!(
            "PMA file apparent length is too large for current capacity: file_len_bytes={file_len} current_words={current_words} expected_capacity_bytes={expected_capacity_bytes}. The slab file should materialize current capacity, not maximum virtual reservation."
        ))
        .into());
    }
    Ok(())
}

fn catch_unwind_message<F>(f: F) -> Result<(), String>
where
    F: FnOnce(),
{
    let hook = panic::take_hook();
    let captured = Arc::new(Mutex::new(None));
    let captured_for_hook = Arc::clone(&captured);
    panic::set_hook(Box::new(move |info| {
        let message = info
            .payload_as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| panic_message(info.payload()));
        *captured_for_hook.lock().expect("panic hook lock poisoned") = Some(message);
    }));

    let result = panic::catch_unwind(AssertUnwindSafe(f));
    panic::set_hook(hook);

    match result {
        Ok(()) => Ok(()),
        Err(payload) => Err(captured
            .lock()
            .expect("panic hook lock poisoned")
            .take()
            .unwrap_or_else(|| panic_message(&*payload))),
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else {
        "<non-string panic payload>".to_string()
    }
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &std::ffi::OsStr) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            std::env::set_var(self.key, previous);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

struct Workspace {
    path: PathBuf,
}

impl Workspace {
    fn new(label: &str) -> Result<Self, Box<dyn Error>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!("{label}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
