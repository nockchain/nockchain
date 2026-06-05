use std::path::Path;

use bytes::Bytes;
use nockvm::noun::{Atom, IndirectAtom, Noun, NounAllocator, NounSpace, D, SIG, T};
use nockvm_macros::tas;
use noun_serde::{NounDecode, NounEncode};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::AsyncWriteExt;
use tracing::{debug, error};

use crate::nockapp::driver::{make_driver, IODriverFn};
use crate::nockapp::error::NockAppError;
use crate::nockapp::wire::{Wire, WireRepr};
use crate::noun::slab::NounSlab;
use crate::utils::durability;
use crate::{AtomExt, IndirectAtomExt};

#[derive(Clone, Debug, NounDecode)]
struct BatchWriteRequestEntry {
    path: String,
    contents: Bytes,
}

#[derive(Clone, Debug, NounEncode)]
struct BatchWriteResultEntry {
    path: String,
    contents: Bytes,
    success: bool,
}

fn decode_from_noun<T: NounDecode>(noun: Noun, space: &NounSpace) -> Result<T, NockAppError> {
    T::from_noun(&noun, space).map_err(|err| NockAppError::NounDecodeError(Box::new(err)))
}

async fn ensure_parent_dirs(path: &str) -> std::io::Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent).await?;
    }
    Ok(())
}

async fn prepare_file_write(path: &str, contents: &[u8]) -> std::io::Result<File> {
    ensure_parent_dirs(path).await?;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .await?;
    file.write_all(contents).await?;
    Ok(file)
}

async fn write_then_flush(path: &str, contents: &[u8]) -> std::io::Result<()> {
    debug!("file driver: writing {} bytes to: {}", contents.len(), path);
    let file = prepare_file_write(path, contents).await?;
    durability::sync_all_async(&file, "file_driver_write_fsync", Some(Path::new(path))).await
}

pub enum FileWire {
    Read,
    Write,
    BatchWrite,
}

impl Wire for FileWire {
    const VERSION: u64 = 1;
    const SOURCE: &'static str = "file";

    fn to_wire(&self) -> crate::nockapp::wire::WireRepr {
        let tags = match self {
            FileWire::Read => vec!["read".into()],
            FileWire::Write => vec!["write".into()],
            FileWire::BatchWrite => vec!["batch-write".into()],
        };
        WireRepr::new(FileWire::SOURCE, FileWire::VERSION, tags)
    }
}

/// File IO Driver
///
/// ## Effects
/// `[%file %read path=@t]`
/// results in poke
/// `[%file %read ~]` on read error
/// or
/// `[%file %read ~ contents=@]` on read success
///
///  `[%file %write path=@t contents=@]`
///  results in file written to disk and poke
///  `[%file %write path=@t contents=@ success=?]`
///
///  `[%file %batch-write (list [path=@t contents=@])]`
///  results in each file written to disk and poke
///  `[%file %batch-write (list [path=@t contents=@ success=?])]`
pub fn file() -> IODriverFn {
    make_driver(|handle| async move {
        enum FileOp {
            Read(String),
            Write { path: String, contents: Bytes },
            BatchWrite(Vec<BatchWriteRequestEntry>),
        }

        loop {
            let effect_res = handle.next_effect().await;
            let slab = match effect_res {
                Ok(slab) => slab,
                Err(e) => {
                    error!("Error receiving effect: {:?}", e);
                    continue;
                }
            };

            let parsed = (|| {
                let space = slab.noun_space();
                let effect_cell = unsafe { slab.root() }.in_space(&space).as_cell().ok()?;

                if !unsafe { effect_cell.head().noun().raw_equals(&D(tas!(b"file"))) } {
                    return None;
                }

                let file_cell = effect_cell.tail().as_cell().ok()?;
                let operation = decode_from_noun::<String>(file_cell.head().noun(), &space).ok()?;

                match operation.as_str() {
                    "read" => {
                        let path =
                            decode_from_noun::<String>(file_cell.tail().noun(), &space).ok()?;
                        Some(FileOp::Read(path))
                    }
                    "write" => {
                        let (path, contents) =
                            decode_from_noun::<(String, Bytes)>(file_cell.tail().noun(), &space)
                                .ok()?;
                        Some(FileOp::Write { path, contents })
                    }
                    "batch-write" => {
                        let batch_entries = decode_from_noun::<Vec<BatchWriteRequestEntry>>(
                            file_cell.tail().noun(),
                            &space,
                        )
                        .ok()?;
                        Some(FileOp::BatchWrite(batch_entries))
                    }
                    _ => None,
                }
            })();

            let Some(operation) = parsed else {
                continue;
            };

            match operation {
                FileOp::Read(path) => match fs::read(&path).await {
                    Ok(contents) => {
                        let mut poke_slab = NounSlab::new();
                        let contents_atom = <IndirectAtom as IndirectAtomExt>::from_bytes(
                            &mut poke_slab,
                            contents.as_slice(),
                        );
                        let poke_noun: Noun = T(
                            &mut poke_slab,
                            &[D(tas!(b"file")), D(tas!(b"read")), SIG, contents_atom.as_noun()],
                        );
                        poke_slab.set_root(poke_noun);
                        let wire = FileWire::Read.to_wire();
                        handle.poke(wire, poke_slab).await?;
                    }
                    Err(_) => {
                        let mut poke_slab = NounSlab::new();
                        let poke_noun: Noun =
                            T(&mut poke_slab, &[D(tas!(b"file")), D(tas!(b"read")), D(0)]);
                        poke_slab.set_root(poke_noun);
                        let wire = FileWire::Read.to_wire();
                        handle.poke(wire, poke_slab).await?;
                    }
                },
                FileOp::Write { path, contents } => {
                    let success = match write_then_flush(&path, contents.as_ref()).await {
                        Ok(_) => true,
                        Err(e) => {
                            error!("file driver: error finalizing path {}: {}", path, e);
                            false
                        }
                    };

                    let mut poke_slab = NounSlab::new();
                    let path_atom = <Atom as AtomExt>::from_value(&mut poke_slab, path.as_str())?;
                    let contents_atom = <IndirectAtom as IndirectAtomExt>::from_bytes(
                        &mut poke_slab,
                        contents.as_ref(),
                    );
                    let success_noun = success.to_noun(&mut poke_slab);
                    let poke_noun: Noun = T(
                        &mut poke_slab,
                        &[
                            D(tas!(b"file")),
                            D(tas!(b"write")),
                            path_atom.as_noun(),
                            contents_atom.as_noun(),
                            success_noun,
                        ],
                    );
                    poke_slab.set_root(poke_noun);
                    let wire = FileWire::Write.to_wire();
                    handle.poke(wire, poke_slab).await?;
                }
                FileOp::BatchWrite(batch_entries) => {
                    let mut results: Vec<BatchWriteResultEntry> =
                        Vec::with_capacity(batch_entries.len());

                    let mut pending_flushes: Vec<(usize, String, File)> =
                        Vec::with_capacity(batch_entries.len());

                    for entry in batch_entries {
                        let BatchWriteRequestEntry { path, contents } = entry;
                        let idx = results.len();
                        results.push(BatchWriteResultEntry {
                            path: path.clone(),
                            contents: contents.clone(),
                            success: false,
                        });

                        match prepare_file_write(&path, contents.as_ref()).await {
                            Ok(file) => pending_flushes.push((idx, path, file)),
                            Err(e) => error!("file driver: error writing to path {}: {}", path, e),
                        }
                    }

                    for (idx, path, file) in pending_flushes {
                        match durability::sync_all_async(
                            &file,
                            "file_driver_batch_write_fsync",
                            Some(Path::new(&path)),
                        )
                        .await
                        {
                            Ok(_) => results[idx].success = true,
                            Err(e) => error!("file driver: error flushing path {}: {}", path, e),
                        }
                    }

                    let mut poke_slab = NounSlab::new();
                    let entries_noun = results.to_noun(&mut poke_slab);
                    let batch_atom = <Atom as AtomExt>::from_value(&mut poke_slab, "batch-write")?;
                    let poke_noun = T(
                        &mut poke_slab,
                        &[D(tas!(b"file")), batch_atom.as_noun(), entries_noun],
                    );
                    poke_slab.set_root(poke_noun);
                    let wire = FileWire::BatchWrite.to_wire();
                    handle.poke(wire, poke_slab).await?;
                }
            }
        }
    })
}
