use std::any::Any;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use nockvm::mem::NockStack;
use nockvm::noun::{Cell, D};
use nockvm::pma::{Pma, PmaCopy, PmaError};

const OLD_WORDS: usize = 32;
const NEW_MIN_WORDS: usize = 64;
const FILL_CELLS: u64 = 10;
const CELL_WORDS: usize = 3;
const STACK_WORDS: usize = 128;
const DISABLE_GROWTH_ENV: &str = "NOCK_PMA_DISABLE_RESIZE_FOR_REGRESSION";
const PMA_MAGIC: u64 = u64::from_le_bytes(*b"NOCKPMA1");
const PMA_LEGACY_VERSION: u64 = 1;
const PMA_LEGACY_TRAILER_BYTES: u64 = 32;

pub(crate) fn run_regression() -> Result<(), Box<dyn Error>> {
    nockvm::check_endian();
    let workspace = Workspace::new("pma-resize-exhaustion-regression")?;
    run(workspace.path())
}

fn run(workspace: &Path) -> Result<(), Box<dyn Error>> {
    let fixture = workspace.join("fixture.pma");
    let raw_canary = workspace.join("raw-canary.pma");
    let resized_candidate = workspace.join("resized-candidate.pma");

    println!(
        "creating exhausted fixture: old_words={OLD_WORDS} fill_cells={FILL_CELLS} cell_words={CELL_WORDS}"
    );
    make_exhausted_fixture(&fixture)?;
    print_file_metadata("fixture", &fixture)?;
    assert_eq!(
        Pma::read_file_metadata(&fixture)?.version,
        PMA_LEGACY_VERSION,
        "fixture must exercise the shipped fixed-size v1 trailer"
    );

    fs::copy(&fixture, &raw_canary)?;
    let _disable_growth = EnvVarGuard::set(DISABLE_GROWTH_ENV, OsStr::new("1"));
    assert_raw_open_still_reproduces_oom(&raw_canary)?;
    drop(_disable_growth);

    fs::copy(&fixture, &resized_candidate)?;
    assert_resized_open_accepts_next_cell(&resized_candidate)?;

    println!("resized-open accepted next cell");
    Ok(())
}

fn copy_one_cell_to_pma(pma: &mut Pma, head: u64, tail: u64) {
    let mut stack = NockStack::new(STACK_WORDS, 0);
    let mut noun = Cell::new(&mut stack, D(head), D(tail)).as_noun();
    unsafe { noun.copy_to_pma(&stack, pma) };
}

fn make_exhausted_fixture(path: &Path) -> Result<(), Box<dyn Error>> {
    let mut pma = Pma::new(OLD_WORDS, path.to_path_buf())?;
    for i in 0..FILL_CELLS {
        copy_one_cell_to_pma(&mut pma, i, i + 1);
    }
    assert_eq!(
        pma.alloc_offset(),
        FILL_CELLS as usize * CELL_WORDS,
        "fixture should consume exactly ten CellMemory allocations"
    );
    assert_eq!(
        pma.free_words(),
        2,
        "fixture must have exactly two free words to reproduce the incident-class OOM"
    );
    pma.sync_all()?;
    pma.sync_file()?;
    write_legacy_v1_trailer(path, OLD_WORDS as u64, pma.alloc_offset() as u64)?;
    Ok(())
}

fn write_legacy_v1_trailer(
    path: &Path,
    data_words: u64,
    alloc_words: u64,
) -> Result<(), Box<dyn Error>> {
    let mut file = fs::OpenOptions::new().read(true).write(true).open(path)?;
    file.set_len(data_words * 8 + PMA_LEGACY_TRAILER_BYTES)?;
    file.seek(SeekFrom::Start(data_words * 8))?;
    for word in [PMA_MAGIC, PMA_LEGACY_VERSION, data_words, alloc_words] {
        file.write_all(&word.to_le_bytes())?;
    }
    file.sync_all()?;
    Ok(())
}

fn assert_raw_open_still_reproduces_oom(path: &Path) -> Result<(), Box<dyn Error>> {
    let mut pma = Pma::open(path.to_path_buf())?;
    print_pma_state("raw-open before canary", &pma);
    assert_eq!(pma.size_words(), OLD_WORDS);
    assert_eq!(pma.alloc_offset(), FILL_CELLS as usize * CELL_WORDS);
    assert_eq!(pma.free_words(), 2);

    let panic = catch_unwind_message(|| copy_one_cell_to_pma(&mut pma, 100, 101));
    match panic {
        Ok(()) => Err(std::io::Error::other(
            "raw Pma::open unexpectedly allowed the exhausted old slab to accept another cell",
        )
        .into()),
        Err(message) => {
            if !message.contains("PMA is full") || !message.contains("available: 2") {
                return Err(std::io::Error::other(format!(
                    "raw-open canary panicked with the wrong message: {message}"
                ))
                .into());
            }
            println!("raw-open canary reproduced PMA OOM: {message}");
            Ok(())
        }
    }
}

fn assert_resized_open_accepts_next_cell(path: &Path) -> Result<(), Box<dyn Error>> {
    let mut pma = open_or_resize_pma(path.to_path_buf(), NEW_MIN_WORDS)?;
    print_pma_state("production resize/open before next cell", &pma);
    assert_eq!(
        pma.alloc_offset(),
        FILL_CELLS as usize * CELL_WORDS,
        "resize/open must preserve the used prefix and allocation cursor"
    );

    if pma.size_words() < NEW_MIN_WORDS {
        let panic = catch_unwind_message(|| copy_one_cell_to_pma(&mut pma, 200, 201));
        let message = match panic {
            Ok(()) => "undersized PMA unexpectedly accepted the next cell".to_string(),
            Err(message) => message,
        };
        return Err(std::io::Error::other(format!(
            "production resize/open path returned an undersized PMA: requested_min_words={NEW_MIN_WORDS} actual_words={} next_cell_result={message}",
            pma.size_words()
        ))
        .into());
    }

    assert!(
        pma.free_words() >= NEW_MIN_WORDS - (FILL_CELLS as usize * CELL_WORDS),
        "resized PMA should have enough free words for future allocations"
    );

    let copy = catch_unwind_message(|| copy_one_cell_to_pma(&mut pma, 200, 201));
    if let Err(message) = copy {
        return Err(std::io::Error::other(format!(
            "resized-open still panicked while copying the next cell: {message}"
        ))
        .into());
    }

    assert_eq!(
        pma.alloc_offset(),
        (FILL_CELLS as usize + 1) * CELL_WORDS,
        "next cell should allocate at the preserved cursor"
    );
    print_pma_state("production resize/open after next cell", &pma);
    Ok(())
}

fn open_or_resize_pma(path: PathBuf, min_words: usize) -> Result<Pma, PmaError> {
    Pma::open_with_min(path, min_words)
}

fn print_file_metadata(label: &str, path: &Path) -> Result<(), Box<dyn Error>> {
    let metadata = Pma::read_file_metadata(path)?;
    println!(
        "{label}: path={} version={} data_words={} alloc_words={} free_words={} reserved_words={} apparent_file_bytes={}",
        path.display(),
        metadata.version,
        metadata.data_words,
        metadata.alloc_words,
        metadata.data_words.saturating_sub(metadata.alloc_words),
        metadata.reserved_words,
        metadata.apparent_file_bytes
    );
    Ok(())
}

fn print_pma_state(label: &str, pma: &Pma) {
    println!(
        "{label}: size_words={} alloc_offset={} free_words={}",
        pma.size_words(),
        pma.alloc_offset(),
        pma.free_words()
    );
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

struct Workspace {
    path: PathBuf,
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &OsStr) -> Self {
        let previous = std::env::var_os(key);
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
