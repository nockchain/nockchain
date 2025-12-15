use std::fs::{create_dir_all, File};
use std::io::{Error, Write};
use std::result::Result;
use std::time::Instant;

use ::json::object;

use super::*;
use crate::mem::NockStack;
use crate::noun::Noun;

#[derive(Clone, Copy)]
struct TraceData {
    pub start: Instant,
    pub path: Noun,
}

pub struct JsonBackend {
    pub file: File,
    pub pid: u32,
    pub process_start: Instant,
}

impl TraceBackend for JsonBackend {
    fn append_trace(&mut self, stack: &mut NockStack, path: Noun) {
        TraceStack::push_on_stack(
            stack,
            TraceData {
                start: Instant::now(),
                path,
            },
        );
    }

    unsafe fn write_nock_trace(
        &mut self,
        stack: &mut NockStack,
        trace_stack: *const TraceStack,
    ) -> Result<(), Error> {
        let mut trace_stack = trace_stack as *const TraceStack<TraceData>;
        let now = Instant::now();

        while !trace_stack.is_null() {
            let ts = (&(*trace_stack))
                .start
                .saturating_duration_since(self.process_start)
                .as_micros() as f64;
            let dur = now
                .saturating_duration_since((&(*trace_stack)).start)
                .as_micros() as f64;

            // Don't write out traces less than 33us
            // (same threshhold used in vere)
            if dur < 33.0 {
                trace_stack = (&(*trace_stack)).next;
                continue;
            }

            let pc = path_to_cord(stack, (&(*trace_stack)).path);
            let pc_len = met3_usize(pc);
            let pc_bytes = &pc.as_ne_bytes()[0..pc_len];
            let pc_str = match std::str::from_utf8(pc_bytes) {
                Ok(valid) => valid,
                Err(error) => {
                    let (valid, _) = pc_bytes.split_at(error.valid_up_to());
                    unsafe { std::str::from_utf8_unchecked(valid) }
                }
            };

            let obj = object! {
                "cat" => "nock",
                "name" => pc_str,
                "ph" => "X",
                "pid" => self.pid,
                "tid" => 1,
                "ts" => ts,
                "dur" => dur,
            };
            obj.write(&mut self.file)?;
            self.file.write_all(",\n".as_bytes())?;

            trace_stack = (&(*trace_stack)).next;
        }

        Ok(())
    }

    fn write_serf_trace(&mut self, name: &str, start: Instant) -> Result<(), Error> {
        let ts = start
            .saturating_duration_since(self.process_start)
            .as_micros() as f64;
        let dur = Instant::now().saturating_duration_since(start).as_micros() as f64;

        let obj = object! {
            "cat" => "event",
            "name" => name,
            "ph" => "X",
            "pid" => self.pid,
            "tid" => 1,
            "ts" => ts,
            "dur" => dur,
        };
        obj.write(&mut self.file)?;
        self.file.write_all(",\n".as_bytes())?;

        Ok(())
    }

    fn write_metadata(&mut self) -> Result<(), Error> {
        self.file.write_all("[ ".as_bytes())?;

        (object! {
            "name" => "process_name",
            "ph" => "M",
            "pid" => self.pid,
            "args" => object! { "name" => "urbit", },
        })
        .write(&mut self.file)?;
        self.file.write_all(",\n".as_bytes())?;

        (object! {
            "name" => "thread_name",
            "ph" => "M",
            "pid" => self.pid,
            "tid" => 1,
            "args" => object! { "name" => "Event Processing", },
        })
        .write(&mut self.file)?;
        self.file.write_all(",\n".as_bytes())?;

        (object! {
            "name" => "thread_sort_index",
            "ph" => "M",
            "pid" => self.pid,
            "tid" => 1,
            "args" => object! { "sort_index" => 1, },
        })
        .write(&mut self.file)?;
        self.file.write_all(",\n".as_bytes())?;

        Ok(())
    }
}

pub fn create_trace_file(pier_path: PathBuf) -> Result<TraceInfo, Error> {
    let mut trace_dir_path = pier_path.clone();
    trace_dir_path.push(".urb");
    trace_dir_path.push("put");
    trace_dir_path.push("trace");
    create_dir_all(&trace_dir_path)?;

    let trace_path: PathBuf;
    let mut trace_idx = 0u32;
    loop {
        let mut prospective_path = trace_dir_path.clone();
        prospective_path.push(format!("{trace_idx}.json"));

        if prospective_path.exists() {
            trace_idx += 1;
        } else {
            trace_path = prospective_path.clone();
            break;
        }
    }

    let file = File::create(trace_path)?;
    let process_start = Instant::now();
    let pid = std::process::id();

    let backend = Box::new(JsonBackend {
        file,
        pid,
        process_start,
    });

    Ok(TraceInfo {
        backend,
        filter: None,
    })
}
