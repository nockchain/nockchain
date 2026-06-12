use std::collections::HashMap;
use std::sync::Mutex;

use either::Either;
use tracing::callsite::DefaultCallsite;
use tracing::dispatcher::{self, Dispatch};
use tracing::span::Attributes;
use tracing::{Id, Level, Metadata};
use tracing_core::field::FieldSet;
use tracing_core::identify_callsite;
use tracing_core::metadata::Kind;

use super::*;

#[derive(Clone, Copy)]
struct TraceData {
    pub span_id: u64,
}

static NOCK_METADATA: Metadata<'static> = Metadata::new(
    "nockroot",
    module_path!(),
    Level::DEBUG,
    Some(file!()),
    Some(line!()),
    Some(module_path!()),
    FieldSet::new(&[], identify_callsite!(&NOCK_CALLSITE)),
    Kind::SPAN,
);

static NOCK_CALLSITE: DefaultCallsite = DefaultCallsite::new(&NOCK_METADATA);

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct TraceKey {
    path: String,
    chum: String,
}

#[derive(Clone)]
struct TraceEntry {
    id: Id,
}

impl TraceEntry {
    fn new(chum: &str, path: &str, dispatch: &Dispatch, level: Level) -> Self {
        let path: &'static str = Box::leak(path.to_string().into_boxed_str());
        let chum: &'static str = Box::leak(chum.to_string().into_boxed_str());
        let span_name: &'static str = Box::leak(format!("{chum}::{path}").into_boxed_str());

        let metadata = Box::leak(Box::new(Metadata::new(
            span_name,
            "nockcode",
            level,
            None,
            None,
            Some(path),
            FieldSet::new(&[], identify_callsite!(&NOCK_CALLSITE)),
            Kind::SPAN,
        )));
        let values = metadata.fields().value_set(&[]);
        let attrs = Attributes::new(metadata, &values);
        let id = dispatch.new_span(&attrs);
        Self { id }
    }
}

// In case we reinitialize Serf (we probably won't), cache old entries.
static GLOBAL_ENTRIES: Mutex<Option<HashMap<TraceKey, TraceEntry>>> = Mutex::new(None);

pub struct TracingBackend {
    entries: HashMap<TraceKey, TraceEntry>,
    subscriber: Option<Dispatch>,
}

impl Default for TracingBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl TracingBackend {
    pub fn new() -> Self {
        Self {
            entries: GLOBAL_ENTRIES
                .lock()
                .ok()
                .and_then(|mut v| v.take())
                .unwrap_or_default(),
            subscriber: None,
        }
    }
}

impl Drop for TracingBackend {
    fn drop(&mut self) {
        let mut entries = GLOBAL_ENTRIES.lock().expect("GLOBAL_ENTRIES lock poisoned");
        if entries.as_ref().map(|v| v.len()).unwrap_or(0) <= self.entries.len() {
            *entries = Some(core::mem::take(&mut self.entries));
        }
    }
}

impl TraceBackend for TracingBackend {
    fn append_trace(&mut self, stack: &mut NockStack, path: Noun) {
        let mut tmp = path;
        let space = stack.noun_space();

        let chum = loop {
            match tmp.as_either_atom_cell() {
                Either::Left(atom) => break atom,
                Either::Right(cell) => tmp = cell.in_space(&space).head().noun(),
            }
        };

        let chum_handle = chum.in_space(&space);
        let Ok(chum) = std::str::from_utf8(chum_handle.as_ne_bytes()) else {
            return;
        };
        let chum = chum.trim_end_matches('\0');

        let path = path_to_cord(stack, path);
        let path_handle = path.in_space(&space);
        let path = std::str::from_utf8(path_handle.as_ne_bytes()).unwrap_or("");

        if self.subscriber.is_none() {
            self.subscriber = Some(dispatcher::get_default(Clone::clone));
        }
        let subscriber = self.subscriber.as_ref().expect("subscriber should be set");

        let key = TraceKey {
            path: path.to_string(),
            chum: chum.to_string(),
        };
        let id = if let Some(entry) = self.entries.get(&key) {
            entry.id.clone()
        } else {
            let entry = TraceEntry::new(chum, path, subscriber, Level::DEBUG);
            let id = entry.id.clone();
            self.entries.insert(key, entry);
            id
        };

        subscriber.enter(&id);
        TraceStack::push_on_stack(
            stack,
            TraceData {
                span_id: id.into_u64(),
            },
        );
    }

    unsafe fn write_nock_trace(
        &mut self,
        _: &mut NockStack,
        trace_stack: *const TraceStack,
    ) -> Result<(), Error> {
        let mut trace_stack = trace_stack as *const TraceStack<TraceData>;
        if trace_stack.is_null() {
            return Ok(());
        }

        let subscriber = self
            .subscriber
            .as_ref()
            .expect("No subscriber with a trace stack");

        loop {
            let id = Id::from_u64((&*trace_stack).span_id);
            subscriber.exit(&id);
            trace_stack = (&*trace_stack).next;
            if trace_stack.is_null() {
                break Ok(());
            }
        }
    }
}
