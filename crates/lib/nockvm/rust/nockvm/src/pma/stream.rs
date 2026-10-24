use std::alloc::{alloc, dealloc, Layout};
use std::fs::{File, OpenOptions};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::FileExt;
#[cfg(target_os = "linux")]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(target_os = "macos")]
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::ptr::NonNull;

#[cfg(unix)]
use libc;
use thiserror::Error;

use super::Pma;
use crate::noun::{
    CELL_MASK, CELL_TAG, DIRECT_MASK, DIRECT_TAG, INDIRECT_MASK, INDIRECT_TAG, LOCATION_BIT,
};
use crate::offset::PmaOffsetWords;

const DEFAULT_CACHE_PAGES: usize = 64;

#[derive(Clone, Copy, Debug)]
pub struct PmaDirectJamConfig {
    pub cache_pages: usize,
    pub require_direct_io: bool,
}

impl Default for PmaDirectJamConfig {
    fn default() -> Self {
        Self {
            cache_pages: DEFAULT_CACHE_PAGES,
            require_direct_io: true,
        }
    }
}

#[derive(Debug, Error)]
pub enum PmaDirectJamError {
    #[error("direct IO open failed: {0}")]
    DirectIoOpen(#[source] io::Error),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid noun raw {raw:#x}: {reason}")]
    InvalidNoun { raw: u64, reason: &'static str },
    #[error("offset {offset} out of bounds (limit {limit})")]
    OffsetOutOfBounds { offset: u64, limit: u64 },
    #[error("indirect atom size {size} at offset {offset} exceeds limit {limit}")]
    IndirectSizeOutOfBounds { offset: u64, size: u64, limit: u64 },
    #[error("aligned allocation failed")]
    AllocationFailed,
    #[error("invalid alignment: {0}")]
    BadAlignment(String),
    #[error("direct IO unsupported on this platform")]
    UnsupportedPlatform,
}

struct AlignedBuffer {
    ptr: NonNull<u8>,
    len: usize,
    layout: Layout,
}

impl AlignedBuffer {
    fn new(len: usize, align: usize) -> Result<Self, PmaDirectJamError> {
        let layout = Layout::from_size_align(len, align)
            .map_err(|err| PmaDirectJamError::BadAlignment(err.to_string()))?;
        let ptr = unsafe { alloc(layout) };
        let ptr = NonNull::new(ptr).ok_or(PmaDirectJamError::AllocationFailed)?;
        Ok(Self { ptr, len, layout })
    }

    fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        unsafe {
            dealloc(self.ptr.as_ptr(), self.layout);
        }
    }
}

struct CachedPage {
    index: u64,
    last_used: u64,
    data: AlignedBuffer,
}

pub struct PmaDirectReader {
    file: File,
    page_size: usize,
    data_words: u64,
    alloc_words: u64,
    cache_capacity: usize,
    cache: Vec<CachedPage>,
    tick: u64,
}

impl PmaDirectReader {
    pub fn new(pma: &Pma, config: PmaDirectJamConfig) -> Result<Self, PmaDirectJamError> {
        Self::from_path(
            pma.path(),
            u64::try_from(pma.size_words()).expect("PMA data words exceed u64 addressable range"),
            pma.alloc_offset_words().into(),
            config,
        )
    }

    pub fn from_path(
        path: &Path,
        data_words: u64,
        alloc_words: u64,
        config: PmaDirectJamConfig,
    ) -> Result<Self, PmaDirectJamError> {
        let file = open_direct_file(path, config.require_direct_io)?;
        let page_size = page_size()?;
        let cache_capacity = config.cache_pages.max(1);
        Ok(Self {
            file,
            page_size,
            data_words,
            alloc_words,
            cache_capacity,
            cache: Vec::with_capacity(cache_capacity),
            tick: 0,
        })
    }

    pub fn alloc_words(&self) -> u64 {
        self.alloc_words
    }

    pub fn read_u64(&mut self, offset_words: u64) -> Result<u64, PmaDirectJamError> {
        if offset_words >= self.alloc_words {
            return Err(PmaDirectJamError::OffsetOutOfBounds {
                offset: offset_words,
                limit: self.alloc_words,
            });
        }
        let byte_offset =
            offset_words
                .checked_mul(8)
                .ok_or(PmaDirectJamError::OffsetOutOfBounds {
                    offset: offset_words,
                    limit: self.alloc_words,
                })?;
        let page_size = self.page_size as u64;
        let page_index = byte_offset / page_size;
        let page_offset = (byte_offset % page_size) as usize;
        let cache_index = self.ensure_page(page_index)?;
        let page = self.cache[cache_index].data.as_slice();
        if page_offset + 8 <= self.page_size {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&page[page_offset..page_offset + 8]);
            return Ok(u64::from_ne_bytes(bytes));
        }

        let mut bytes = [0u8; 8];
        let first_len = self.page_size - page_offset;
        bytes[..first_len].copy_from_slice(&page[page_offset..]);
        let next_index = self.ensure_page(page_index + 1)?;
        let next_page = self.cache[next_index].data.as_slice();
        bytes[first_len..].copy_from_slice(&next_page[..8 - first_len]);
        Ok(u64::from_ne_bytes(bytes))
    }

    fn ensure_page(&mut self, index: u64) -> Result<usize, PmaDirectJamError> {
        self.tick = self.tick.wrapping_add(1);
        if let Some((idx, entry)) = self
            .cache
            .iter_mut()
            .enumerate()
            .find(|(_, entry)| entry.index == index)
        {
            entry.last_used = self.tick;
            return Ok(idx);
        }

        let slot = if self.cache.len() < self.cache_capacity {
            let data = AlignedBuffer::new(self.page_size, self.page_size)?;
            self.cache.push(CachedPage {
                index,
                last_used: self.tick,
                data,
            });
            self.cache.len() - 1
        } else {
            let (idx, _) = self
                .cache
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.last_used)
                .ok_or(PmaDirectJamError::AllocationFailed)?;
            self.cache[idx].index = index;
            self.cache[idx].last_used = self.tick;
            idx
        };

        let data = self.cache[slot].data.as_mut_slice();
        Self::read_page(&self.file, self.page_size, self.data_words, index, data)?;
        Ok(slot)
    }

    fn read_page(
        file: &File,
        page_size: usize,
        data_words: u64,
        index: u64,
        buffer: &mut [u8],
    ) -> Result<(), PmaDirectJamError> {
        let offset_bytes =
            index
                .checked_mul(page_size as u64)
                .ok_or(PmaDirectJamError::OffsetOutOfBounds {
                    offset: index,
                    limit: data_words,
                })?;
        let mut total = 0usize;
        while total < buffer.len() {
            let read = file.read_at(&mut buffer[total..], offset_bytes + total as u64)?;
            if read == 0 {
                if total == 0 {
                    return Err(PmaDirectJamError::Io(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "short read while reading PMA page",
                    )));
                }
                break;
            }
            total += read;
        }
        if total < buffer.len() {
            buffer[total..].fill(0);
        }
        Ok(())
    }

    pub fn read_cell(&mut self, offset: u64) -> Result<(u64, u64), PmaDirectJamError> {
        let limit = self.alloc_words;
        if offset + 3 > limit {
            return Err(PmaDirectJamError::OffsetOutOfBounds { offset, limit });
        }
        let head = self.read_u64(offset + 1)?;
        let tail = self.read_u64(offset + 2)?;
        Ok((head, tail))
    }

    pub fn indirect_atom_words(&mut self, offset: u64) -> Result<usize, PmaDirectJamError> {
        let size_raw = self.read_u64(offset + 1)?;
        if size_raw & CELL_MASK == CELL_MASK {
            return Err(PmaDirectJamError::InvalidNoun {
                raw: size_raw,
                reason: "forwarding pointer in indirect atom header",
            });
        }
        let size_words =
            usize::try_from(size_raw).map_err(|_| PmaDirectJamError::IndirectSizeOutOfBounds {
                offset,
                size: size_raw,
                limit: self.alloc_words,
            })?;
        if size_words == 0 {
            return Err(PmaDirectJamError::InvalidNoun {
                raw: size_raw,
                reason: "zero-length indirect atom",
            });
        }
        let limit = self.alloc_words;
        let end = offset
            .checked_add(2)
            .and_then(|base| base.checked_add(size_words as u64))
            .ok_or(PmaDirectJamError::IndirectSizeOutOfBounds {
                offset,
                size: size_raw,
                limit,
            })?;
        if end > limit {
            return Err(PmaDirectJamError::IndirectSizeOutOfBounds {
                offset,
                size: size_raw,
                limit,
            });
        }
        Ok(size_words)
    }

    pub fn indirect_atom_bits(&mut self, offset: u64) -> Result<usize, PmaDirectJamError> {
        let size_words = self.indirect_atom_words(offset)?;
        let last_word = self.read_u64(offset + 2 + (size_words as u64 - 1))?;
        let last_bits = 64usize.saturating_sub(last_word.leading_zeros() as usize);
        Ok((size_words - 1).saturating_mul(64) + last_bits)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum PmaRawNounKind {
    Direct(u64),
    Indirect { offset: PmaOffsetWords },
    Cell { offset: PmaOffsetWords },
}

pub fn classify_pma_noun(raw: u64) -> Result<PmaRawNounKind, PmaDirectJamError> {
    if raw & DIRECT_MASK == DIRECT_TAG {
        return Ok(PmaRawNounKind::Direct(raw));
    }
    if raw & INDIRECT_MASK == INDIRECT_TAG {
        return offset_noun(raw, INDIRECT_MASK, "indirect");
    }
    if raw & CELL_MASK == CELL_TAG {
        return offset_noun(raw, CELL_MASK, "cell");
    }
    if raw & CELL_MASK == CELL_MASK {
        return Err(PmaDirectJamError::InvalidNoun {
            raw,
            reason: "forwarding pointer not supported",
        });
    }
    Err(PmaDirectJamError::InvalidNoun {
        raw,
        reason: "unknown noun tag",
    })
}

fn offset_noun(
    raw: u64,
    mask: u64,
    kind: &'static str,
) -> Result<PmaRawNounKind, PmaDirectJamError> {
    if raw & LOCATION_BIT == 0 {
        return Err(PmaDirectJamError::InvalidNoun {
            raw,
            reason: "expected offset-form noun",
        });
    }
    let offset = raw & !(mask | LOCATION_BIT);
    let offset = PmaOffsetWords::from_words(offset);
    match kind {
        "indirect" => Ok(PmaRawNounKind::Indirect { offset }),
        "cell" => Ok(PmaRawNounKind::Cell { offset }),
        _ => Err(PmaDirectJamError::InvalidNoun {
            raw,
            reason: "unknown offset noun type",
        }),
    }
}

fn page_size() -> Result<usize, PmaDirectJamError> {
    #[cfg(unix)]
    {
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if page <= 0 {
            return Err(PmaDirectJamError::Io(io::Error::last_os_error()));
        }
        Ok(page as usize)
    }
    #[cfg(not(unix))]
    {
        Ok(4096)
    }
}

fn open_direct_file(path: &Path, require_direct_io: bool) -> Result<File, PmaDirectJamError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        if require_direct_io {
            options.custom_flags(libc::O_DIRECT);
        }
    }
    let file = options.open(path).map_err(|err| {
        if require_direct_io {
            PmaDirectJamError::DirectIoOpen(err)
        } else {
            PmaDirectJamError::Io(err)
        }
    })?;

    #[cfg(target_os = "macos")]
    {
        if require_direct_io {
            let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_NOCACHE, 1) };
            if rc != 0 {
                return Err(PmaDirectJamError::DirectIoOpen(io::Error::last_os_error()));
            }
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        if require_direct_io {
            return Err(PmaDirectJamError::UnsupportedPlatform);
        }
    }

    Ok(file)
}
