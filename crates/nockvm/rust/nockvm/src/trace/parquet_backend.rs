use std::fs::{File, OpenOptions};
use std::io::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use arrow_array::builder::{ListBuilder, StringBuilder, UInt64Builder};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;

use super::*;

const DEFAULT_FLUSH_ROWS: usize = 2048;

#[derive(Debug)]
struct TraceRow {
    event_id: u64,
    ts_ns: u64,
    kind: &'static str,
    path_parts: Option<Vec<String>>,
    chum: Option<String>,
    name: Option<String>,
    elapsed_us: Option<u64>,
    output_path: Option<String>,
    engine: Option<String>,
    phase: Option<String>,
    location: Option<String>,
    trace_path: Option<String>,
    detail: Option<String>,
}

pub struct ParquetBackend {
    output_path: PathBuf,
    schema: Arc<Schema>,
    writer: Option<ArrowWriter<File>>,
    pending_rows: Vec<TraceRow>,
    next_event_id: u64,
    write_failed: bool,
}

impl ParquetBackend {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, Error> {
        let output_path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&output_path)?;

        let schema = Arc::new(Schema::new(vec![
            Field::new("event_id", DataType::UInt64, false),
            Field::new("ts_ns", DataType::UInt64, false),
            Field::new("kind", DataType::Utf8, false),
            Field::new(
                "path_parts",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                true,
            ),
            Field::new("chum", DataType::Utf8, true),
            Field::new("name", DataType::Utf8, true),
            Field::new("elapsed_us", DataType::UInt64, true),
            Field::new("output_path", DataType::Utf8, true),
            Field::new("engine", DataType::Utf8, true),
            Field::new("phase", DataType::Utf8, true),
            Field::new("location", DataType::Utf8, true),
            Field::new("trace_path", DataType::Utf8, true),
            Field::new("detail", DataType::Utf8, true),
        ]));

        let writer_props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::default()))
            .set_dictionary_enabled(true)
            .build();

        let writer = ArrowWriter::try_new(file, schema.clone(), Some(writer_props))
            .map_err(|err| Error::other(format!("failed to initialize parquet writer: {err}")))?;

        Ok(Self {
            output_path,
            schema,
            writer: Some(writer),
            pending_rows: Vec::new(),
            next_event_id: 0,
            write_failed: false,
        })
    }

    fn now_ns() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_event_id;
        self.next_event_id = self.next_event_id.saturating_add(1);
        id
    }

    fn atom_text(atom: crate::noun::AtomHandle<'_>) -> Option<String> {
        let text = std::str::from_utf8(atom.as_ne_bytes()).ok()?;
        Some(text.trim_end_matches('\0').to_string())
    }

    fn atom_decimal(atom: crate::noun::AtomHandle<'_>) -> Option<String> {
        atom.as_u64().ok().map(|n| n.to_string())
    }

    fn path_parts_and_chum(
        path: Noun,
        space: &crate::noun::NounSpace,
    ) -> Option<(Vec<String>, String)> {
        let mut chum_cursor = path;
        let chum_atom = loop {
            match chum_cursor.in_space(space).as_either_atom_cell() {
                either::Either::Left(atom) => break atom,
                either::Either::Right(cell) => chum_cursor = cell.head().noun(),
            }
        };
        let chum = Self::atom_text(chum_atom)?;

        let mut parts = Vec::new();
        let mut path_cursor = path;
        loop {
            if path_cursor
                .in_space(space)
                .as_atom()
                .ok()
                .and_then(|atom| atom.as_u64().ok())
                .map(|n| n == 0)
                .unwrap_or(false)
            {
                break;
            }

            let cell = path_cursor.in_space(space).as_cell().ok()?;
            match cell.head().as_either_atom_cell() {
                either::Either::Left(atom) => {
                    parts.push(Self::atom_text(atom)?);
                }
                either::Either::Right(pair) => {
                    let name = Self::atom_text(pair.head().as_atom().ok()?)?;
                    let value = Self::atom_decimal(pair.tail().as_atom().ok()?)?;
                    parts.push(format!("{name}{value}"));
                }
            }
            path_cursor = cell.tail().noun();
        }

        Some((parts, chum))
    }

    fn push_row(&mut self, row: TraceRow) -> Result<(), Error> {
        self.pending_rows.push(row);
        if self.pending_rows.len() >= DEFAULT_FLUSH_ROWS {
            self.flush_pending_rows()?;
        }
        Ok(())
    }

    fn flush_pending_rows(&mut self) -> Result<(), Error> {
        if self.pending_rows.is_empty() {
            return Ok(());
        }

        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| Error::other("parquet writer is already closed"))?;

        let row_count = self.pending_rows.len();

        let mut event_id_builder = UInt64Builder::with_capacity(row_count);
        let mut ts_ns_builder = UInt64Builder::with_capacity(row_count);
        let mut kind_builder = StringBuilder::with_capacity(row_count, row_count * 8);
        let mut path_parts_builder = ListBuilder::new(StringBuilder::new());
        let mut chum_builder = StringBuilder::new();
        let mut name_builder = StringBuilder::new();
        let mut elapsed_us_builder = UInt64Builder::with_capacity(row_count);
        let mut output_path_builder = StringBuilder::new();
        let mut engine_builder = StringBuilder::new();
        let mut phase_builder = StringBuilder::new();
        let mut location_builder = StringBuilder::new();
        let mut trace_path_builder = StringBuilder::new();
        let mut detail_builder = StringBuilder::new();

        for row in self.pending_rows.drain(..) {
            event_id_builder.append_value(row.event_id);
            ts_ns_builder.append_value(row.ts_ns);
            kind_builder.append_value(row.kind);

            match row.path_parts {
                Some(parts) => {
                    for part in parts {
                        path_parts_builder.values().append_value(part);
                    }
                    path_parts_builder.append(true);
                }
                None => path_parts_builder.append(false),
            }

            match row.chum {
                Some(chum) => chum_builder.append_value(chum),
                None => chum_builder.append_null(),
            }

            match row.name {
                Some(name) => name_builder.append_value(name),
                None => name_builder.append_null(),
            }

            match row.elapsed_us {
                Some(elapsed_us) => elapsed_us_builder.append_value(elapsed_us),
                None => elapsed_us_builder.append_null(),
            }

            match row.output_path {
                Some(output_path) => output_path_builder.append_value(output_path),
                None => output_path_builder.append_null(),
            }

            match row.engine {
                Some(engine) => engine_builder.append_value(engine),
                None => engine_builder.append_null(),
            }

            match row.phase {
                Some(phase) => phase_builder.append_value(phase),
                None => phase_builder.append_null(),
            }

            match row.location {
                Some(location) => location_builder.append_value(location),
                None => location_builder.append_null(),
            }

            match row.trace_path {
                Some(trace_path) => trace_path_builder.append_value(trace_path),
                None => trace_path_builder.append_null(),
            }

            match row.detail {
                Some(detail) => detail_builder.append_value(detail),
                None => detail_builder.append_null(),
            }
        }

        let arrays: Vec<ArrayRef> = vec![
            Arc::new(event_id_builder.finish()),
            Arc::new(ts_ns_builder.finish()),
            Arc::new(kind_builder.finish()),
            Arc::new(path_parts_builder.finish()),
            Arc::new(chum_builder.finish()),
            Arc::new(name_builder.finish()),
            Arc::new(elapsed_us_builder.finish()),
            Arc::new(output_path_builder.finish()),
            Arc::new(engine_builder.finish()),
            Arc::new(phase_builder.finish()),
            Arc::new(location_builder.finish()),
            Arc::new(trace_path_builder.finish()),
            Arc::new(detail_builder.finish()),
        ];

        let batch = RecordBatch::try_new(self.schema.clone(), arrays)
            .map_err(|err| Error::other(format!("failed to build record batch: {err}")))?;

        writer
            .write(&batch)
            .map_err(|err| Error::other(format!("failed to write record batch: {err}")))?;

        Ok(())
    }

    fn finish(&mut self) -> Result<(), Error> {
        self.flush_pending_rows()?;
        if let Some(writer) = self.writer.take() {
            writer
                .close()
                .map_err(|err| Error::other(format!("failed to close parquet writer: {err}")))?;
        }
        Ok(())
    }

    pub fn write_behavior_event(
        &mut self,
        engine: &str,
        phase: &str,
        location: Option<&str>,
        trace_path: Option<&str>,
        detail: Option<&str>,
    ) -> Result<(), Error> {
        if self.write_failed {
            return Ok(());
        }

        let row = TraceRow {
            event_id: self.next_id(),
            ts_ns: Self::now_ns(),
            kind: "behavior",
            path_parts: None,
            chum: None,
            name: None,
            elapsed_us: None,
            output_path: None,
            engine: Some(engine.to_string()),
            phase: Some(phase.to_string()),
            location: location.map(ToOwned::to_owned),
            trace_path: trace_path.map(ToOwned::to_owned),
            detail: detail.map(ToOwned::to_owned),
        };

        match self.push_row(row) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.write_failed = true;
                Err(err)
            }
        }
    }
}

impl Drop for ParquetBackend {
    fn drop(&mut self) {
        if self.write_failed {
            return;
        }

        if let Err(err) = self.finish() {
            self.write_failed = true;
            eprintln!(
                "failed to finalize parquet trace backend at {}: {err}",
                self.output_path.display()
            );
        }
    }
}

impl TraceBackend for ParquetBackend {
    fn append_trace(&mut self, stack: &mut NockStack, path: Noun) {
        if self.write_failed {
            return;
        }

        let space = stack.noun_space();
        let Some((path_parts, chum)) = Self::path_parts_and_chum(path, &space) else {
            return;
        };

        let row = TraceRow {
            event_id: self.next_id(),
            ts_ns: Self::now_ns(),
            kind: "nock_trace",
            path_parts: Some(path_parts),
            chum: Some(chum),
            name: None,
            elapsed_us: None,
            output_path: None,
            engine: Some("oracle".to_string()),
            phase: Some("nock.trace".to_string()),
            location: None,
            trace_path: None,
            detail: None,
        };

        if self.push_row(row).is_err() {
            self.write_failed = true;
        }
    }

    unsafe fn write_nock_trace(
        &mut self,
        _: &mut NockStack,
        _: *const TraceStack,
    ) -> Result<(), Error> {
        Ok(())
    }

    fn write_serf_trace(&mut self, name: &str, start: Instant) -> Result<(), Error> {
        if self.write_failed {
            return Ok(());
        }

        let row = TraceRow {
            event_id: self.next_id(),
            ts_ns: Self::now_ns(),
            kind: "serf_trace",
            path_parts: None,
            chum: None,
            name: Some(name.to_string()),
            elapsed_us: Some(start.elapsed().as_micros() as u64),
            output_path: None,
            engine: Some("oracle".to_string()),
            phase: Some("serf.trace".to_string()),
            location: None,
            trace_path: None,
            detail: None,
        };

        match self.push_row(row) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.write_failed = true;
                Err(err)
            }
        }
    }

    fn write_metadata(&mut self) -> Result<(), Error> {
        if self.write_failed {
            return Ok(());
        }

        let row = TraceRow {
            event_id: self.next_id(),
            ts_ns: Self::now_ns(),
            kind: "trace_metadata",
            path_parts: None,
            chum: None,
            name: None,
            elapsed_us: None,
            output_path: Some(self.output_path.display().to_string()),
            engine: Some("oracle".to_string()),
            phase: Some("trace.metadata".to_string()),
            location: None,
            trace_path: None,
            detail: None,
        };

        match self.push_row(row) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.write_failed = true;
                Err(err)
            }
        }
    }

    fn write_behavior_event(
        &mut self,
        engine: &str,
        phase: &str,
        location: Option<&str>,
        trace_path: Option<&str>,
        detail: Option<&str>,
    ) -> Result<(), Error> {
        ParquetBackend::write_behavior_event(self, engine, phase, location, trace_path, detail)
    }
}
