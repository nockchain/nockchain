//! Byte-preserving copies that leave zero-filled ranges unallocated where supported.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

const BUFFER_BYTES: usize = 1024 * 1024;
const ZERO_BLOCK_BYTES: usize = 4096;

/// Copy a stable regular file, preserving its logical length and permissions.
///
/// Extent discovery skips existing holes. Every data extent is also checked for
/// zero blocks, so materialized zeros and coarse extent reports remain sparse.
/// A temporary file beside `dst` keeps failed copies from replacing `dst`.
/// The caller must sync the completed file and its directory before publication.
pub(crate) fn copy(src: &Path, dst: &Path) -> io::Result<()> {
    copy_with_extents(src, dst, true)
}

fn copy_with_extents(src: &Path, dst: &Path, use_extents: bool) -> io::Result<()> {
    let mut source = File::open(src)?;
    let metadata = source.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sparse copy source must be a regular file",
        ));
    }
    match fs::metadata(dst) {
        Ok(destination_metadata) => {
            if same_file(src, &metadata, dst, &destination_metadata)? {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "sparse copy source and destination are the same file",
                ));
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }

    let parent = dst
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::Builder::new()
        .prefix(".sparse-copy-")
        .tempfile_in(parent)?;
    copy_contents(
        &mut source,
        temporary.as_file_mut(),
        metadata.len(),
        use_extents,
    )?;
    temporary
        .as_file()
        .set_permissions(metadata.permissions())?;
    temporary.persist(dst).map_err(|err| err.error)?;
    Ok(())
}

#[cfg(unix)]
fn same_file(
    _src: &Path,
    source: &fs::Metadata,
    _dst: &Path,
    destination: &fs::Metadata,
) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    Ok(source.dev() == destination.dev() && source.ino() == destination.ino())
}

#[cfg(not(unix))]
fn same_file(
    src: &Path,
    _source: &fs::Metadata,
    dst: &Path,
    _destination: &fs::Metadata,
) -> io::Result<bool> {
    // The destination is replaced, never truncated, so another hard link cannot
    // damage the opened source even where portable file identities are absent.
    Ok(fs::canonicalize(src)? == fs::canonicalize(dst)?)
}

fn copy_contents(
    source: &mut File,
    destination: &mut File,
    len: u64,
    mut use_extents: bool,
) -> io::Result<()> {
    let mut buffer = vec![0; BUFFER_BYTES];
    let mut offset = 0;
    while offset < len {
        let (start, end) = if use_extents {
            match next_data_extent(source, offset, len) {
                Ok(Some(extent)) => extent,
                Ok(None) => break,
                Err(err) if err.kind() == io::ErrorKind::Unsupported => {
                    use_extents = false;
                    (offset, len)
                }
                Err(err) => return Err(err),
            }
        } else {
            (offset, len)
        };
        copy_range(source, destination, start, end, &mut buffer)?;
        offset = end;
    }
    // A source truncated inside a hole may produce SEEK_DATA/ENXIO rather than
    // a short read. Reject it before publishing a silently zero-padded copy.
    let current_len = source.metadata()?.len();
    if current_len != len {
        return Err(io::Error::new(
            if current_len < len {
                io::ErrorKind::UnexpectedEof
            } else {
                io::ErrorKind::InvalidData
            },
            "sparse copy source length changed during copy",
        ));
    }
    destination.set_len(len)
}

fn copy_range(
    source: &mut File,
    destination: &mut File,
    mut offset: u64,
    end: u64,
    buffer: &mut [u8],
) -> io::Result<()> {
    source.seek(SeekFrom::Start(offset))?;
    while offset < end {
        let count = (end - offset).min(buffer.len() as u64) as usize;
        source.read_exact(&mut buffer[..count])?;
        let mut run_start = None;
        for (block, bytes) in buffer[..count].chunks(ZERO_BLOCK_BYTES).enumerate() {
            let start = block * ZERO_BLOCK_BYTES;
            if bytes.iter().any(|&byte| byte != 0) {
                run_start.get_or_insert(start);
            } else if let Some(run) = run_start.take() {
                destination.seek(SeekFrom::Start(offset + run as u64))?;
                destination.write_all(&buffer[run..start])?;
            }
        }
        if let Some(run) = run_start {
            destination.seek(SeekFrom::Start(offset + run as u64))?;
            destination.write_all(&buffer[run..count])?;
        }
        offset += count as u64;
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn next_data_extent(source: &File, offset: u64, len: u64) -> io::Result<Option<(u64, u64)>> {
    use std::os::fd::AsRawFd;

    fn seek(source: &File, offset: u64, whence: libc::c_int) -> io::Result<Option<u64>> {
        let offset = libc::off_t::try_from(offset)
            .map_err(|_| io::Error::new(io::ErrorKind::Unsupported, "file offset exceeds off_t"))?;
        loop {
            // SAFETY: source owns a live file descriptor; lseek does not access
            // memory and its signed offset has been checked above.
            let result = unsafe { libc::lseek(source.as_raw_fd(), offset, whence) };
            if result >= 0 {
                return Ok(Some(result as u64));
            }
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::ENXIO) => return Ok(None),
                Some(code)
                    if code == libc::EINVAL || code == libc::ENOTSUP || code == libc::ENOSYS =>
                {
                    return Err(io::Error::new(io::ErrorKind::Unsupported, err));
                }
                _ => return Err(err),
            }
        }
    }

    let Some(start) = seek(source, offset, libc::SEEK_DATA)? else {
        return Ok(None);
    };
    let end = seek(source, start, libc::SEEK_HOLE)?.unwrap_or(len);
    if start < offset || start >= len || end <= start || end > len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid sparse copy extent or source length changed",
        ));
    }
    Ok(Some((start, end)))
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn next_data_extent(_source: &File, _offset: u64, _len: u64) -> io::Result<Option<(u64, u64)>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "extent discovery unavailable",
    ))
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;

    use tempfile::TempDir;

    use super::*;

    fn fixture(path: &Path, len: u64, materialized: bool) -> File {
        let mut file = File::create(path).expect("create source");
        if materialized {
            let zeros = vec![0; BUFFER_BYTES];
            let mut remaining = len;
            while remaining > 0 {
                let count = remaining.min(zeros.len() as u64) as usize;
                file.write_all(&zeros[..count]).expect("materialize zeros");
                remaining -= count as u64;
            }
        } else {
            file.set_len(len).expect("size source");
        }
        for (offset, bytes) in [
            (0, b"used prefix".as_slice()),
            (len / 2 + 17, b"nonzero free tail".as_slice()),
            (len - 64, b"PMA footer at original EOF".as_slice()),
        ] {
            file.seek(SeekFrom::Start(offset)).expect("seek fixture");
            file.write_all(bytes).expect("write fixture");
        }
        file.sync_all().expect("sync fixture");
        file
    }

    fn assert_equal_files(source: &Path, destination: &Path) {
        assert_eq!(
            fs::metadata(source).unwrap().len(),
            fs::metadata(destination).unwrap().len()
        );
        let mut source = File::open(source).unwrap();
        let mut destination = File::open(destination).unwrap();
        let mut source_bytes = vec![0; BUFFER_BYTES];
        let mut destination_bytes = vec![0; BUFFER_BYTES];
        loop {
            let count = source.read(&mut source_bytes).unwrap();
            if count == 0 {
                break;
            }
            destination
                .read_exact(&mut destination_bytes[..count])
                .unwrap();
            assert_eq!(&source_bytes[..count], &destination_bytes[..count]);
        }
    }

    #[cfg(unix)]
    fn assert_sparse_when_supported(directory: &Path, copied: &Path) {
        use std::os::unix::fs::MetadataExt;
        let probe = directory.join("sparse-probe");
        let probe_file = File::create(&probe).unwrap();
        probe_file.set_len(16 * 1024 * 1024).unwrap();
        probe_file.sync_all().unwrap();
        File::open(copied).unwrap().sync_all().unwrap();
        let probe_metadata = fs::metadata(probe).unwrap();
        if probe_metadata.blocks() * 512 < probe_metadata.len() / 2 {
            let metadata = fs::metadata(copied).unwrap();
            assert!(
                metadata.blocks() * 512 < metadata.len() / 2,
                "copy materialized zero ranges"
            );
        }
    }

    #[test]
    fn sparse_copy_preserves_holes_footer_and_nonzero_unused_bytes() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fixture(&source, 64 * 1024 * 1024 + 64, false);
        copy(&source, &destination).unwrap();
        assert_equal_files(&source, &destination);
        #[cfg(unix)]
        assert_sparse_when_supported(temp.path(), &destination);
    }

    #[test]
    fn materialized_zeros_become_sparse_with_extents_and_buffered_fallback() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        // APFS may materialize gaps when syncing small files. Use a larger PMA
        // shape and measure after sync so delayed allocation cannot mask it.
        fixture(&source, 64 * 1024 * 1024 + 64, true);
        for use_extents in [true, false] {
            let destination = temp.path().join(format!("copy-{use_extents}"));
            copy_with_extents(&source, &destination, use_extents).unwrap();
            assert_equal_files(&source, &destination);
            #[cfg(unix)]
            assert_sparse_when_supported(temp.path(), &destination);
        }
    }

    #[test]
    fn copy_replaces_longer_destination_and_preserves_permissions() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::write(&source, b"short").unwrap();
        fs::write(&destination, b"old data beyond the new EOF").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
        }
        copy(&source, &destination).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"short");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(destination).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
    }

    #[test]
    fn empty_and_all_zero_files_keep_exact_length() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        for len in [0, 3 * 1024 * 1024 + 7] {
            File::create(&source).unwrap().set_len(len).unwrap();
            copy(&source, &destination).unwrap();
            assert_equal_files(&source, &destination);
        }
    }

    #[test]
    fn same_source_path_and_hard_link_are_rejected_without_damage() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::write(&source, b"keep source").unwrap();
        assert_eq!(
            copy(&source, &source).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        #[cfg(unix)]
        {
            let alias = temp.path().join("alias");
            fs::hard_link(&source, &alias).unwrap();
            assert_eq!(
                copy(&source, &alias).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
        assert_eq!(fs::read(source).unwrap(), b"keep source");
    }

    #[test]
    fn short_source_is_an_error_for_extent_and_buffered_paths() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::write(&source, b"short").unwrap();
        for use_extents in [true, false] {
            let mut input = File::open(&source).unwrap();
            let mut output = File::create(temp.path().join("destination")).unwrap();
            assert!(copy_contents(&mut input, &mut output, 1024 * 1024, use_extents).is_err());
        }
    }

    #[test]
    fn failed_publication_cleans_up_temporary_file() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::write(&source, b"source").unwrap();
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).unwrap();
        assert!(copy(&source, &destination).is_err());
        assert!(destination.is_dir());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }

    #[test]
    fn fallback_rejects_truncation_even_when_source_was_only_holes() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let mut input = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(source)
            .unwrap();
        let mut output = File::create(temp.path().join("destination")).unwrap();
        input.set_len(1024).unwrap();
        let error = copy_contents(&mut input, &mut output, 1024 * 1024, true).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }
}
