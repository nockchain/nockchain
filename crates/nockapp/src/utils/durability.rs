use std::fs::File;
use std::io;
#[cfg(target_os = "macos")]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use tokio::fs::File as TokioFile;
use tracing::{debug, warn};

static FSYNC_DISABLED: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_fsync_disabled(disabled: bool) {
    FSYNC_DISABLED.store(disabled, Ordering::Relaxed);
}

pub(crate) fn fsync_disabled() -> bool {
    FSYNC_DISABLED.load(Ordering::Relaxed)
}

pub(crate) fn sync_all(file: &File, context: &str, path: Option<&Path>) -> io::Result<()> {
    sync_std_file(file, SyncKind::All, context, path)
}

pub(crate) fn sync_data(file: &File, context: &str, path: Option<&Path>) -> io::Result<()> {
    sync_std_file(file, SyncKind::Data, context, path)
}

pub(crate) async fn sync_all_async(
    file: &TokioFile,
    context: &str,
    path: Option<&Path>,
) -> io::Result<()> {
    if fsync_disabled() {
        log_sync_skipped("fsync", context, path);
        return Ok(());
    }
    let strategy = async_sync_strategy(file).await?;
    let op = sync_op_name(SyncKind::All, strategy);
    log_sync_start(op, context, path);
    let start = Instant::now();
    let result = sync_tokio_file(file, SyncKind::All, strategy).await;
    log_sync_result(op, context, path, start.elapsed(), &result);
    result
}

pub(crate) fn sync_parent_dir(path: &Path, context: &str) -> io::Result<()> {
    #[cfg(unix)]
    {
        if let Some(parent) = path.parent() {
            let parent_file = File::open(parent)?;
            return sync_all(&parent_file, context, Some(parent));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

pub(crate) fn sync_path_data(path: &Path, context: &str) -> io::Result<()> {
    let file = File::options().read(true).write(true).open(path)?;
    sync_data(&file, context, Some(path))
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8], context: &str) -> io::Result<()> {
    let tmp_path = tmp_path(path);
    std::fs::write(&tmp_path, bytes)?;
    let tmp_file = File::open(&tmp_path)?;
    sync_all(&tmp_file, context, Some(&tmp_path))?;
    std::fs::rename(&tmp_path, path)?;
    sync_parent_dir(path, context)?;
    Ok(())
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".tmp");
    PathBuf::from(os)
}

#[derive(Copy, Clone)]
enum SyncKind {
    All,
    Data,
}

#[derive(Copy, Clone)]
enum SyncStrategy {
    Portable,
    #[cfg(target_os = "macos")]
    FullFlush,
}

fn sync_std_file(
    file: &File,
    kind: SyncKind,
    context: &str,
    path: Option<&Path>,
) -> io::Result<()> {
    if fsync_disabled() {
        log_sync_skipped(default_sync_op_name(kind), context, path);
        return Ok(());
    }

    let strategy = sync_strategy(file)?;
    let op = sync_op_name(kind, strategy);
    run_sync(op, context, path, || {
        sync_std_file_with_strategy(file, kind, strategy)
    })
}

fn sync_std_file_with_strategy(
    file: &File,
    kind: SyncKind,
    strategy: SyncStrategy,
) -> io::Result<()> {
    match kind {
        SyncKind::All => file.sync_all()?,
        SyncKind::Data => file.sync_data()?,
    }

    #[cfg(target_os = "macos")]
    if matches!(strategy, SyncStrategy::FullFlush) {
        full_fsync_file(file)?;
    }

    #[cfg(not(target_os = "macos"))]
    let _ = strategy;

    Ok(())
}

async fn sync_tokio_file(
    file: &TokioFile,
    kind: SyncKind,
    strategy: SyncStrategy,
) -> io::Result<()> {
    match kind {
        SyncKind::All => file.sync_all().await?,
        SyncKind::Data => file.sync_data().await?,
    }

    #[cfg(target_os = "macos")]
    if matches!(strategy, SyncStrategy::FullFlush) {
        full_fsync_fd(file.as_raw_fd())?;
    }

    #[cfg(not(target_os = "macos"))]
    let _ = strategy;

    Ok(())
}

fn default_sync_op_name(kind: SyncKind) -> &'static str {
    match kind {
        SyncKind::All => "fsync",
        SyncKind::Data => "fdatasync",
    }
}

#[cfg(target_os = "macos")]
fn sync_op_name(kind: SyncKind, strategy: SyncStrategy) -> &'static str {
    match (kind, strategy) {
        (SyncKind::All, SyncStrategy::Portable) => "fsync",
        (SyncKind::Data, SyncStrategy::Portable) => "fdatasync",
        (SyncKind::All, SyncStrategy::FullFlush) => "fsync+fullfsync",
        (SyncKind::Data, SyncStrategy::FullFlush) => "fdatasync+fullfsync",
    }
}

#[cfg(not(target_os = "macos"))]
fn sync_op_name(kind: SyncKind, _strategy: SyncStrategy) -> &'static str {
    default_sync_op_name(kind)
}

#[cfg(target_os = "macos")]
fn sync_strategy(file: &File) -> io::Result<SyncStrategy> {
    let metadata = file.metadata()?;
    Ok(if metadata.is_file() {
        SyncStrategy::FullFlush
    } else {
        SyncStrategy::Portable
    })
}

#[cfg(not(target_os = "macos"))]
fn sync_strategy(_file: &File) -> io::Result<SyncStrategy> {
    Ok(SyncStrategy::Portable)
}

#[cfg(target_os = "macos")]
async fn async_sync_strategy(file: &TokioFile) -> io::Result<SyncStrategy> {
    let metadata = file.metadata().await?;
    Ok(if metadata.is_file() {
        SyncStrategy::FullFlush
    } else {
        SyncStrategy::Portable
    })
}

#[cfg(not(target_os = "macos"))]
async fn async_sync_strategy(_file: &TokioFile) -> io::Result<SyncStrategy> {
    Ok(SyncStrategy::Portable)
}

fn run_sync<F>(op: &'static str, context: &str, path: Option<&Path>, f: F) -> io::Result<()>
where
    F: FnOnce() -> io::Result<()>,
{
    log_sync_start(op, context, path);
    let start = Instant::now();
    let result = f();
    log_sync_result(op, context, path, start.elapsed(), &result);
    result
}

#[cfg(target_os = "macos")]
fn full_fsync_file(file: &File) -> io::Result<()> {
    full_fsync_fd(file.as_raw_fd())
}

#[cfg(target_os = "macos")]
fn full_fsync_fd(fd: std::os::fd::RawFd) -> io::Result<()> {
    let rc = unsafe { libc::fcntl(fd, libc::F_FULLFSYNC) };
    if rc == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn log_sync_start(op: &str, context: &str, path: Option<&Path>) {
    match path {
        Some(path) => debug!(
            sync_op = op,
            sync_context = context,
            path = %path.display(),
            "durability sync start"
        ),
        None => debug!(
            sync_op = op,
            sync_context = context,
            "durability sync start"
        ),
    }
}

fn log_sync_skipped(op: &str, context: &str, path: Option<&Path>) {
    match path {
        Some(path) => debug!(
            sync_op = op,
            sync_context = context,
            path = %path.display(),
            "durability sync skipped (fsync disabled)"
        ),
        None => debug!(
            sync_op = op,
            sync_context = context,
            "durability sync skipped (fsync disabled)"
        ),
    }
}

fn log_sync_result(
    op: &str,
    context: &str,
    path: Option<&Path>,
    elapsed: std::time::Duration,
    result: &io::Result<()>,
) {
    let elapsed_ms = duration_ms(elapsed);
    match (path, result) {
        (Some(path), Ok(())) => debug!(
            sync_op = op,
            sync_context = context,
            path = %path.display(),
            elapsed_ms,
            "durability sync done"
        ),
        (None, Ok(())) => debug!(
            sync_op = op,
            sync_context = context,
            elapsed_ms,
            "durability sync done"
        ),
        (Some(path), Err(err)) => warn!(
            sync_op = op,
            sync_context = context,
            path = %path.display(),
            elapsed_ms,
            error = %err,
            "durability sync failed"
        ),
        (None, Err(err)) => warn!(
            sync_op = op,
            sync_context = context,
            elapsed_ms,
            error = %err,
            "durability sync failed"
        ),
    }
}

fn duration_ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{sync_all, sync_parent_dir, sync_path_data, write_atomic};

    #[test]
    fn sync_all_accepts_read_only_regular_files() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("sync-all.bin");
        fs::write(&path, b"abc").expect("write test file");

        let file = std::fs::File::open(&path).expect("open test file");
        sync_all(&file, "durability_test_sync_all", Some(&path)).expect("sync file");
    }

    #[test]
    fn sync_path_data_accepts_regular_files() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("sync-data.bin");
        fs::write(&path, b"abc").expect("write test file");

        sync_path_data(&path, "durability_test_sync_data").expect("sync path data");
    }

    #[test]
    fn sync_parent_dir_accepts_directory_handles() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("child.bin");
        fs::write(&path, b"abc").expect("write child file");

        sync_parent_dir(&path, "durability_test_sync_parent_dir").expect("sync parent dir");
    }

    #[test]
    fn write_atomic_syncs_file_and_parent_dir() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("atomic.bin");

        write_atomic(&path, b"payload", "durability_test_write_atomic").expect("write atomic");

        assert_eq!(fs::read(&path).expect("read atomic file"), b"payload");
    }
}
