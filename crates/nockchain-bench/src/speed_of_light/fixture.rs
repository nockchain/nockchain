//! Unified speed-of-light fixture format and builder.
//!
//! A fixture bundles:
//! - an embedded checkpoint (derived during fixture creation)
//! - a `.solarch` archive replay window
//! - the kernel jam used to build and run the fixture

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::types::SolHeight;

const FIXTURE_MAGIC: &[u8; 8] = b"SOLTEST\0";
const FIXTURE_LAYOUT_VERSION: u16 = 4;
const MAX_FIXTURE_FILE_BYTES: u64 = 16 * 1024 * 1024 * 1024; // 16 GiB
const MAX_FIXTURE_MANIFEST_BYTES: u64 = 1 * 1024 * 1024; // 1 MiB
const MAX_FIXTURE_SECTION_BYTES: u64 = 8 * 1024 * 1024 * 1024; // 8 GiB per section

#[derive(Debug, Clone, Copy)]
struct FixtureSectionLengths {
    manifest_len: u64,
    checkpoint_len: u64,
    archive_len: u64,
    kernel_len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SolFixtureCheckpointKind {
    Derived,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolFixtureManifest {
    pub source_archive_path: String,
    pub source_archive_event_num: Option<u64>,
    pub checkpoint_kind: SolFixtureCheckpointKind,
    pub checkpoint_height: SolHeight,
    pub checkpoint_event_num: u64,
    pub archive_start_height: SolHeight,
    pub archive_end_height: SolHeight,
    pub include_mempool: bool,
    pub chunk_size: u64,
    pub kernel_hash_hex: String,
    pub checkpoint_hash_hex: String,
    pub archive_hash_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolFixtureFile {
    pub manifest: SolFixtureManifest,
    pub checkpoint_bytes: Vec<u8>,
    pub archive_bytes: Vec<u8>,
    pub kernel_bytes: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum FixtureError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] Box<bincode::ErrorKind>),

    #[error("Invalid fixture magic")]
    InvalidMagic,

    #[error("Unsupported fixture layout")]
    UnsupportedLayout,

    #[error("Truncated fixture payload")]
    TruncatedPayload,

    #[error("Fixture section lengths overflow")]
    LengthOverflow,

    #[error("Limit exceeded for {field}: {value} > {max}")]
    LimitExceeded {
        field: &'static str,
        value: u64,
        max: u64,
    },
}

pub fn write_fixture_file<P: AsRef<Path>>(
    path: P,
    fixture: &SolFixtureFile,
) -> Result<(), FixtureError> {
    let manifest_bytes = bincode::serialize(&fixture.manifest)?;
    let lengths = FixtureSectionLengths {
        manifest_len: manifest_bytes.len() as u64,
        checkpoint_len: fixture.checkpoint_bytes.len() as u64,
        archive_len: fixture.archive_bytes.len() as u64,
        kernel_len: fixture.kernel_bytes.len() as u64,
    };
    let mut writer = open_fixture_writer(path.as_ref(), lengths, &manifest_bytes)?;
    writer.write_all(&fixture.checkpoint_bytes)?;
    writer.write_all(&fixture.archive_bytes)?;
    writer.write_all(&fixture.kernel_bytes)?;
    writer.flush()?;
    Ok(())
}

pub fn write_fixture_file_from_paths<P: AsRef<Path>>(
    path: P,
    manifest: &SolFixtureManifest,
    checkpoint_path: &Path,
    archive_path: &Path,
    kernel_path: &Path,
) -> Result<(), FixtureError> {
    let manifest_bytes = bincode::serialize(manifest)?;
    let lengths = FixtureSectionLengths {
        manifest_len: manifest_bytes.len() as u64,
        checkpoint_len: std::fs::metadata(checkpoint_path)?.len(),
        archive_len: std::fs::metadata(archive_path)?.len(),
        kernel_len: std::fs::metadata(kernel_path)?.len(),
    };
    let mut writer = open_fixture_writer(path.as_ref(), lengths, &manifest_bytes)?;
    copy_path_to_writer(checkpoint_path, &mut writer)?;
    copy_path_to_writer(archive_path, &mut writer)?;
    copy_path_to_writer(kernel_path, &mut writer)?;
    writer.flush()?;
    Ok(())
}

pub fn read_fixture_file<P: AsRef<Path>>(path: P) -> Result<SolFixtureFile, FixtureError> {
    let mut reader = open_fixture_reader(path.as_ref())?;
    read_fixture_payload(&mut reader)
}

pub fn extract_fixture_to_paths<P: AsRef<Path>>(
    fixture_path: P,
    checkpoint_path: &Path,
    archive_path: &Path,
    kernel_path: &Path,
) -> Result<SolFixtureManifest, FixtureError> {
    let mut reader = open_fixture_reader(fixture_path.as_ref())?;
    let (lengths, manifest) = read_fixture_manifest_and_lengths(&mut reader)?;
    copy_reader_to_path_exact(&mut reader, checkpoint_path, lengths.checkpoint_len)?;
    copy_reader_to_path_exact(&mut reader, archive_path, lengths.archive_len)?;
    copy_reader_to_path_exact(&mut reader, kernel_path, lengths.kernel_len)?;
    Ok(manifest)
}

fn open_fixture_writer(
    path: &Path,
    lengths: FixtureSectionLengths,
    manifest_bytes: &[u8],
) -> Result<BufWriter<File>, FixtureError> {
    validate_layout_limits(lengths)?;
    let mut writer = BufWriter::new(File::create(path)?);
    write_fixture_header(&mut writer)?;
    write_fixture_layout(&mut writer, lengths)?;
    writer.write_all(manifest_bytes)?;
    Ok(writer)
}

fn open_fixture_reader(path: &Path) -> Result<BufReader<File>, FixtureError> {
    ensure_fixture_file_size(path)?;
    let mut reader = BufReader::new(File::open(path)?);
    let version = read_fixture_header(&mut reader)?;
    if version != FIXTURE_LAYOUT_VERSION {
        return Err(FixtureError::UnsupportedLayout);
    }
    Ok(reader)
}

fn read_fixture_manifest_and_lengths<R: Read>(
    reader: &mut R,
) -> Result<(FixtureSectionLengths, SolFixtureManifest), FixtureError> {
    let lengths = read_fixture_layout(reader)?;
    let manifest = bincode::deserialize(&read_exact_vec(reader, lengths.manifest_len)?)?;
    Ok((lengths, manifest))
}

fn write_fixture_header<W: Write>(writer: &mut W) -> Result<(), FixtureError> {
    writer.write_all(FIXTURE_MAGIC)?;
    writer.write_all(&FIXTURE_LAYOUT_VERSION.to_le_bytes())?;
    Ok(())
}

fn read_fixture_header<R: Read>(reader: &mut R) -> Result<u16, FixtureError> {
    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic)?;
    if &magic != FIXTURE_MAGIC {
        return Err(FixtureError::InvalidMagic);
    }
    let mut version_bytes = [0u8; 2];
    reader.read_exact(&mut version_bytes)?;
    Ok(u16::from_le_bytes(version_bytes))
}

fn write_fixture_layout<W: Write>(
    writer: &mut W,
    layout: FixtureSectionLengths,
) -> Result<(), FixtureError> {
    writer.write_all(&layout.manifest_len.to_le_bytes())?;
    writer.write_all(&layout.checkpoint_len.to_le_bytes())?;
    writer.write_all(&layout.archive_len.to_le_bytes())?;
    writer.write_all(&layout.kernel_len.to_le_bytes())?;
    Ok(())
}

fn read_fixture_layout<R: Read>(reader: &mut R) -> Result<FixtureSectionLengths, FixtureError> {
    let manifest_len = read_u64(reader)?;
    let checkpoint_len = read_u64(reader)?;
    let archive_len = read_u64(reader)?;
    let kernel_len = read_u64(reader)?;
    let layout = FixtureSectionLengths {
        manifest_len,
        checkpoint_len,
        archive_len,
        kernel_len,
    };
    validate_layout_limits(layout)?;
    Ok(layout)
}

fn read_u64<R: Read>(reader: &mut R) -> Result<u64, FixtureError> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_exact_vec<R: Read>(reader: &mut R, len: u64) -> Result<Vec<u8>, FixtureError> {
    let len = usize::try_from(len).map_err(|_| FixtureError::TruncatedPayload)?;
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    Ok(payload)
}

fn read_fixture_payload<R: Read>(reader: &mut R) -> Result<SolFixtureFile, FixtureError> {
    let (lengths, manifest) = read_fixture_manifest_and_lengths(reader)?;
    let checkpoint_bytes = read_exact_vec(reader, lengths.checkpoint_len)?;
    let archive_bytes = read_exact_vec(reader, lengths.archive_len)?;
    let kernel_bytes = read_exact_vec(reader, lengths.kernel_len)?;
    Ok(SolFixtureFile {
        manifest,
        checkpoint_bytes,
        archive_bytes,
        kernel_bytes,
    })
}

fn copy_path_to_writer<W: Write>(source_path: &Path, writer: &mut W) -> Result<(), FixtureError> {
    let mut source = File::open(source_path)?;
    std::io::copy(&mut source, writer)?;
    Ok(())
}

fn copy_reader_to_path_exact<R: Read>(
    reader: &mut R,
    destination_path: &Path,
    len: u64,
) -> Result<(), FixtureError> {
    let mut destination = BufWriter::new(File::create(destination_path)?);
    let mut limited = reader.take(len);
    let copied = std::io::copy(&mut limited, &mut destination)?;
    if copied != len {
        return Err(FixtureError::TruncatedPayload);
    }
    destination.flush()?;
    Ok(())
}

fn ensure_fixture_file_size(path: &Path) -> Result<(), FixtureError> {
    let file_size = std::fs::metadata(path)?.len();
    enforce_limit("fixture.file_size", file_size, MAX_FIXTURE_FILE_BYTES)
}

fn validate_layout_limits(layout: FixtureSectionLengths) -> Result<(), FixtureError> {
    enforce_limit(
        "fixture.manifest_bytes", layout.manifest_len, MAX_FIXTURE_MANIFEST_BYTES,
    )?;
    enforce_limit(
        "fixture.checkpoint_bytes", layout.checkpoint_len, MAX_FIXTURE_SECTION_BYTES,
    )?;
    enforce_limit(
        "fixture.archive_bytes", layout.archive_len, MAX_FIXTURE_SECTION_BYTES,
    )?;
    enforce_limit(
        "fixture.kernel_bytes", layout.kernel_len, MAX_FIXTURE_SECTION_BYTES,
    )?;

    let total_size = encoded_fixture_file_size(layout)?;
    enforce_limit("fixture.file_size", total_size, MAX_FIXTURE_FILE_BYTES)?;
    Ok(())
}

fn encoded_fixture_file_size(layout: FixtureSectionLengths) -> Result<u64, FixtureError> {
    let header_bytes = 8u64 + 2 + 8 + 8 + 8 + 8;
    header_bytes
        .checked_add(layout.manifest_len)
        .and_then(|sum| sum.checked_add(layout.checkpoint_len))
        .and_then(|sum| sum.checked_add(layout.archive_len))
        .and_then(|sum| sum.checked_add(layout.kernel_len))
        .ok_or(FixtureError::LengthOverflow)
}

fn enforce_limit(field: &'static str, value: u64, max: u64) -> Result<(), FixtureError> {
    if value > max {
        return Err(FixtureError::LimitExceeded { field, value, max });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixture_file_roundtrip() {
        let fixture = SolFixtureFile {
            manifest: SolFixtureManifest {
                source_archive_path: "/tmp/source.solarch".to_string(),
                source_archive_event_num: Some(100_000),
                checkpoint_kind: SolFixtureCheckpointKind::Derived,
                checkpoint_height: SolHeight(49_999),
                checkpoint_event_num: 49_999,
                archive_start_height: SolHeight(50_000),
                archive_end_height: SolHeight(60_000),
                include_mempool: false,
                chunk_size: 8,
                kernel_hash_hex: "k".repeat(64),
                checkpoint_hash_hex: "c".repeat(64),
                archive_hash_hex: "a".repeat(64),
            },
            checkpoint_bytes: vec![1, 2, 3],
            archive_bytes: vec![4, 5, 6],
            kernel_bytes: vec![7, 8, 9],
        };

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = temp_dir.path().join("fixture.soltest");
        write_fixture_file(&path, &fixture).expect("write fixture");
        let loaded = read_fixture_file(&path).expect("read fixture");

        assert_eq!(loaded.manifest.archive_start_height, SolHeight(50_000));
        assert_eq!(loaded.checkpoint_bytes, vec![1, 2, 3]);
        assert_eq!(loaded.archive_bytes, vec![4, 5, 6]);
        assert_eq!(loaded.kernel_bytes, vec![7, 8, 9]);
    }

    #[test]
    fn test_extract_fixture_to_paths_roundtrip() {
        let fixture = SolFixtureFile {
            manifest: SolFixtureManifest {
                source_archive_path: "/tmp/source.solarch".to_string(),
                source_archive_event_num: Some(100_000),
                checkpoint_kind: SolFixtureCheckpointKind::Full,
                checkpoint_height: SolHeight(49_999),
                checkpoint_event_num: 49_999,
                archive_start_height: SolHeight(50_000),
                archive_end_height: SolHeight(60_000),
                include_mempool: false,
                chunk_size: 8,
                kernel_hash_hex: "k".repeat(64),
                checkpoint_hash_hex: "c".repeat(64),
                archive_hash_hex: "a".repeat(64),
            },
            checkpoint_bytes: vec![1, 2, 3],
            archive_bytes: vec![4, 5, 6],
            kernel_bytes: vec![7, 8, 9],
        };

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let fixture_path = temp_dir.path().join("fixture.soltest");
        let checkpoint_path = temp_dir.path().join("fixture.chkjam");
        let archive_path = temp_dir.path().join("fixture.solarch");
        let kernel_path = temp_dir.path().join("fixture.jam");

        write_fixture_file(&fixture_path, &fixture).expect("write fixture");
        let manifest =
            extract_fixture_to_paths(&fixture_path, &checkpoint_path, &archive_path, &kernel_path)
                .expect("extract fixture");

        assert_eq!(manifest.archive_start_height, SolHeight(50_000));
        assert_eq!(
            std::fs::read(checkpoint_path).expect("read checkpoint"),
            vec![1, 2, 3]
        );
        assert_eq!(
            std::fs::read(archive_path).expect("read archive"),
            vec![4, 5, 6]
        );
        assert_eq!(
            std::fs::read(kernel_path).expect("read kernel"),
            vec![7, 8, 9]
        );
    }

    #[test]
    fn fixture_round_trips_derived_checkpoint_kind() {
        let fixture = SolFixtureFile {
            manifest: SolFixtureManifest {
                source_archive_path: "/tmp/source.solarch".to_string(),
                source_archive_event_num: Some(7),
                checkpoint_kind: SolFixtureCheckpointKind::Derived,
                checkpoint_height: SolHeight(10),
                checkpoint_event_num: 11,
                archive_start_height: SolHeight(11),
                archive_end_height: SolHeight(12),
                include_mempool: false,
                chunk_size: 8,
                kernel_hash_hex: "k".repeat(64),
                checkpoint_hash_hex: "c".repeat(64),
                archive_hash_hex: "a".repeat(64),
            },
            checkpoint_bytes: vec![1],
            archive_bytes: vec![2],
            kernel_bytes: vec![3],
        };

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = temp_dir.path().join("fixture.soltest");
        write_fixture_file(&path, &fixture).expect("write fixture");
        let loaded = read_fixture_file(&path).expect("read fixture");

        assert_eq!(
            loaded.manifest.checkpoint_kind,
            SolFixtureCheckpointKind::Derived
        );
    }

    #[test]
    fn fixture_round_trips_full_checkpoint_kind() {
        let fixture = SolFixtureFile {
            manifest: SolFixtureManifest {
                source_archive_path: "/tmp/source.solarch".to_string(),
                source_archive_event_num: Some(7),
                checkpoint_kind: SolFixtureCheckpointKind::Full,
                checkpoint_height: SolHeight(10),
                checkpoint_event_num: 11,
                archive_start_height: SolHeight(11),
                archive_end_height: SolHeight(12),
                include_mempool: false,
                chunk_size: 8,
                kernel_hash_hex: "k".repeat(64),
                checkpoint_hash_hex: "c".repeat(64),
                archive_hash_hex: "a".repeat(64),
            },
            checkpoint_bytes: vec![1],
            archive_bytes: vec![2],
            kernel_bytes: vec![3],
        };

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = temp_dir.path().join("fixture.soltest");
        write_fixture_file(&path, &fixture).expect("write fixture");
        let loaded = read_fixture_file(&path).expect("read fixture");

        assert_eq!(
            loaded.manifest.checkpoint_kind,
            SolFixtureCheckpointKind::Full
        );
    }
}
