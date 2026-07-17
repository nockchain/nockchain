use std::cmp::Reverse;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};

use honk::pipeline;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticNodeId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticTextRange {
    pub start: u32,
    pub end: u32,
}

impl SemanticTextRange {
    pub fn contains(self, byte_offset: u32) -> bool {
        self.start <= byte_offset && byte_offset < self.end
    }

    fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }
}

/// Convert Hatch's one-based debug-spot coordinates back to source bytes,
/// including its tall-tape column normalization.
pub fn range_from_one_based_spot(
    source: &str,
    start_line: u64,
    start_column: u64,
    end_line: u64,
    end_column: u64,
) -> Option<SemanticTextRange> {
    if source.len() > u32::MAX as usize {
        return None;
    }
    let lines = LineIndex::new(source);
    let start = u32::try_from(lines.byte_offset(start_line, start_column)?).ok()?;
    let end = u32::try_from(lines.byte_offset(end_line, end_column)?).ok()?;
    Some(SemanticTextRange {
        start,
        end: end
            .max(start.saturating_add(1))
            .min(lines.source_len as u32),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticNode {
    pub id: SemanticNodeId,
    pub range: SemanticTextRange,
    pub syntax_kind: String,
    pub rune: Option<String>,
    signature: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticSymbolKind {
    Arm,
    Mold,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticSymbol {
    pub id: SemanticNodeId,
    pub name: String,
    pub kind: SemanticSymbolKind,
    pub detail: String,
    pub range: SemanticTextRange,
    pub selection_range: SemanticTextRange,
    pub parent: Option<SemanticNodeId>,
    signature: String,
}

/// An editor-only lexical face and the exact source region in which it is
/// visible. These records are derived from the traced parser AST without
/// changing the AST or any compiler-owned type-checking path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticBinding {
    pub id: SemanticNodeId,
    pub name: String,
    pub declaration_range: SemanticTextRange,
    pub scope_range: SemanticTextRange,
    signature: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticHover {
    pub id: SemanticNodeId,
    pub range: SemanticTextRange,
    pub markdown: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticSnapshot {
    pub path: PathBuf,
    pub version: i64,
    pub nodes: Vec<SemanticNode>,
    pub symbols: Vec<SemanticSymbol>,
    pub bindings: Vec<SemanticBinding>,
}

impl SemanticSnapshot {
    pub fn hover(&self, byte_offset: u32) -> Option<SemanticHover> {
        if let Some(symbol) = self
            .symbols
            .iter()
            .filter(|symbol| {
                SemanticTextRange {
                    start: symbol.range.start,
                    end: symbol.selection_range.end,
                }
                .contains(byte_offset)
            })
            .min_by_key(|symbol| {
                symbol
                    .selection_range
                    .start
                    .saturating_sub(symbol.range.start)
            })
        {
            let noun = match symbol.kind {
                SemanticSymbolKind::Arm => "arm",
                SemanticSymbolKind::Mold => "mold arm",
            };
            return Some(SemanticHover {
                id: symbol.id,
                range: if symbol.selection_range.contains(byte_offset) {
                    symbol.selection_range
                } else {
                    SemanticTextRange {
                        start: symbol.range.start,
                        end: symbol.selection_range.end,
                    }
                },
                markdown: format!("**`{}`** — Hoon {noun} (`{}`)", symbol.name, symbol.detail),
            });
        }

        self.nodes
            .iter()
            .filter(|node| node.range.contains(byte_offset))
            .min_by_key(|node| node.range.len())
            .map(|node| {
                let label = node.rune.as_deref().unwrap_or(node.syntax_kind.as_str());
                SemanticHover {
                    id: node.id,
                    range: node.range,
                    markdown: format!(
                        "Hoon syntax: **`{label}`**\n\nInternal form: `{}`",
                        node.syntax_kind
                    ),
                }
            })
    }

    /// Resolve the innermost lexical face or an unambiguous same-document arm
    /// or mold name.
    ///
    /// Compiler-owned provenance remains authoritative for scoped and
    /// cross-file definitions. This structural fallback deliberately declines
    /// duplicate names rather than guessing across nested cores.
    pub fn definition(&self, source: &str, byte_offset: u32) -> Option<SemanticTextRange> {
        let name = hoon_term_at(source, byte_offset)?;
        if let Some(binding) = self
            .bindings
            .iter()
            .filter(|binding| binding.name == name && binding.scope_range.contains(byte_offset))
            .min_by_key(|binding| {
                (
                    binding.scope_range.len(),
                    Reverse(binding.declaration_range.start),
                )
            })
        {
            return Some(binding.declaration_range);
        }
        let mut matches = self.symbols.iter().filter(|symbol| symbol.name == name);
        let definition = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(definition.selection_range)
    }
}

/// Return the Hoon term under a byte offset using the editor's term boundary
/// rules. This deliberately excludes uppercase and punctuation-heavy tokens.
pub fn hoon_term_at(source: &str, byte_offset: u32) -> Option<&str> {
    let range = hoon_term_range_at(source, byte_offset)?;
    source.get(range.start as usize..range.end as usize)
}

/// Return the two-glyph Hoon rune under a byte offset.
pub fn hoon_rune_at(source: &str, byte_offset: u32) -> Option<&str> {
    let offset = usize::try_from(byte_offset).ok()?;
    if offset >= source.len() || !source.is_char_boundary(offset) {
        return None;
    }
    [offset.checked_sub(1), Some(offset)]
        .into_iter()
        .flatten()
        .find_map(|start| {
            let rune = source.get(start..start.checked_add(2)?)?;
            rune_tag(rune).is_some().then_some(rune)
        })
}

/// Locate a unique arm or mold declaration without parsing the whole file.
///
/// This lightweight structural index is used for imported editor sources and
/// the large standard-library prelude. It declines duplicate declarations so
/// callers never guess between nested cores with the same arm name.
pub fn structural_definition(source: &str, name: &str) -> Option<SemanticTextRange> {
    if source.len() > u32::MAX as usize {
        return None;
    }
    let mut matches = scan_arm_headers(source)
        .into_iter()
        .filter(|symbol| symbol.name == name);
    let definition = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(definition.selection_range)
}

/// Locate a rune's canonical tagged alternative in the standard-library
/// `hoon` mold, such as `^-` at `[%kthp p=spec q=hoon]`.
pub fn structural_rune_definition(source: &str, rune: &str) -> Option<SemanticTextRange> {
    if source.len() > u32::MAX as usize {
        return None;
    }
    if let Some(parser_arm) = match rune {
        "++" => Some("bola"),
        "+$" => Some("boba"),
        "+|" => Some("whip"),
        _ => None,
    } {
        return structural_definition(source, parser_arm);
    }
    let tag = rune_tag(rune)?;
    let tagged_alternative = format!("[%{tag}");
    let mut offset = 0usize;
    let mut definition = None;
    for line in source.split_inclusive('\n') {
        let line_without_newline = line.strip_suffix('\n').unwrap_or(line);
        let Some(tag_start) = line_without_newline.find(&tagged_alternative) else {
            offset += line.len();
            continue;
        };
        let Some(comment_start) = line_without_newline[tag_start + tagged_alternative.len()..]
            .find("::")
            .map(|relative| tag_start + tagged_alternative.len() + relative)
        else {
            offset += line.len();
            continue;
        };
        let comment = line_without_newline[comment_start + 2..].trim_start();
        let rune_is_documented = comment.strip_prefix(rune).is_some_and(|remainder| {
            remainder.is_empty()
                || remainder
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_whitespace)
        });
        if !rune_is_documented {
            offset += line.len();
            continue;
        }
        let start = offset + tag_start + 2;
        let range = SemanticTextRange {
            start: u32::try_from(start).ok()?,
            end: u32::try_from(start + tag.len()).ok()?,
        };
        if definition.replace(range).is_some() {
            return None;
        }
        offset += line.len();
    }
    definition
}

fn rune_tag(rune: &str) -> Option<String> {
    let bytes: [u8; 2] = rune.as_bytes().try_into().ok()?;
    Some(format!(
        "{}{}",
        rune_syllable_tag(bytes[0])?,
        rune_syllable_tag(bytes[1])?
    ))
}

fn rune_syllable_tag(glyph: u8) -> Option<&'static str> {
    match glyph {
        b'|' => Some("br"),
        b'$' => Some("bc"),
        b'_' => Some("cb"),
        b'%' => Some("cn"),
        b':' => Some("cl"),
        b',' => Some("cm"),
        b'.' => Some("dt"),
        b'/' => Some("fs"),
        b'<' => Some("gl"),
        b'>' => Some("gr"),
        b'#' => Some("hx"),
        b'-' => Some("hp"),
        b'^' => Some("kt"),
        b'+' => Some("ls"),
        b';' => Some("mc"),
        b'&' => Some("pm"),
        b'@' => Some("pt"),
        b'~' => Some("sg"),
        b'*' => Some("tr"),
        b'`' => Some("tc"),
        b'=' => Some("ts"),
        b'?' => Some("wt"),
        b'!' => Some("zp"),
        _ => None,
    }
}

fn hoon_term_range_at(source: &str, byte_offset: u32) -> Option<SemanticTextRange> {
    let offset = usize::try_from(byte_offset).ok()?;
    if offset >= source.len() || !source.is_char_boundary(offset) {
        return None;
    }
    if !is_hoon_term_byte(source.as_bytes()[offset]) {
        return None;
    }
    let mut start = offset;
    while start > 0 && is_hoon_term_byte(source.as_bytes()[start - 1]) {
        start -= 1;
    }
    let mut end = offset + 1;
    while end < source.len() && is_hoon_term_byte(source.as_bytes()[end]) {
        end += 1;
    }
    if !source.as_bytes()[start].is_ascii_lowercase() {
        return None;
    }
    Some(SemanticTextRange {
        start: u32::try_from(start).ok()?,
        end: u32::try_from(end).ok()?,
    })
}

fn is_hoon_term_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticError {
    message: String,
}

impl SemanticError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SemanticError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SemanticError {}

#[derive(Clone)]
struct CachedDocument {
    version: i64,
    source_hash: blake3::Hash,
    result: Result<SemanticSnapshot, SemanticError>,
}

/// Lightweight, Send-safe semantic state for editor protocol threads.
///
/// This intentionally does not share noun-bearing compiler state. It reparses
/// with the existing traced parser and builds side tables whose lifetimes are
/// independent of the compiler arena. IDs are stable across revisions when a
/// symbol or traced syntax fragment can be matched to the previous snapshot.
#[derive(Default)]
pub struct SemanticSession {
    documents: HashMap<PathBuf, CachedDocument>,
    next_id: u64,
}

impl SemanticSession {
    pub fn snapshot(
        &mut self,
        path: &Path,
        version: i64,
        source: &str,
    ) -> Result<&SemanticSnapshot, SemanticError> {
        let source_hash = blake3::hash(source.as_bytes());
        let is_current = self
            .documents
            .get(path)
            .is_some_and(|cached| cached.version == version && cached.source_hash == source_hash);
        if !is_current {
            let previous = self
                .documents
                .get(path)
                .and_then(|cached| cached.result.as_ref().ok())
                .cloned();
            if self.next_id == 0 {
                self.next_id = 1;
            }
            let result =
                build_snapshot(path, version, source, previous.as_ref(), &mut self.next_id);
            self.documents.insert(
                path.to_path_buf(),
                CachedDocument {
                    version,
                    source_hash,
                    result,
                },
            );
        }

        match &self
            .documents
            .get(path)
            .expect("semantic document was inserted")
            .result
        {
            Ok(snapshot) => Ok(snapshot),
            Err(error) => Err(error.clone()),
        }
    }

    pub fn close(&mut self, path: &Path) {
        self.documents.remove(path);
    }
}

#[derive(Clone, Debug)]
struct RawNode {
    range: SemanticTextRange,
    syntax_kind: String,
    rune: Option<String>,
    signature: String,
}

#[derive(Clone, Debug)]
struct RawSymbol {
    name: String,
    kind: SemanticSymbolKind,
    detail: String,
    range: SemanticTextRange,
    selection_range: SemanticTextRange,
    indent: usize,
    signature: String,
}

#[derive(Clone, Debug)]
struct RawBinding {
    name: String,
    declaration_range: SemanticTextRange,
    scope_range: SemanticTextRange,
    signature: String,
}

fn build_snapshot(
    path: &Path,
    version: i64,
    source: &str,
    previous: Option<&SemanticSnapshot>,
    next_id: &mut u64,
) -> Result<SemanticSnapshot, SemanticError> {
    if source.len() > u32::MAX as usize {
        return Err(SemanticError::new(
            "editor semantic snapshots are limited to 4 GiB documents",
        ));
    }
    let wer = vec![path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("document.hoon")
        .to_string()];
    let ast = pipeline::parse_native_hoon_source_without_docs(path, source, wer, true)
        .map_err(|error| SemanticError::new(error.to_string()))?;
    let value = serde_json::to_value(&ast)
        .map_err(|error| SemanticError::new(format!("failed to index Hoon AST: {error}")))?;
    let lines = LineIndex::new(source);

    let mut raw_nodes = Vec::new();
    collect_traced_nodes(&value, source, &lines, &mut raw_nodes);
    raw_nodes.sort_by_key(|node| (node.range.start, node.range.end, node.syntax_kind.clone()));
    raw_nodes
        .dedup_by(|right, left| right.range == left.range && right.syntax_kind == left.syntax_kind);

    let mut old_node_ids = previous
        .map(|snapshot| signature_ids(snapshot.nodes.iter().map(|node| (&node.signature, node.id))))
        .unwrap_or_default();
    let nodes = raw_nodes
        .into_iter()
        .map(|node| SemanticNode {
            id: reused_or_new_id(&node.signature, &mut old_node_ids, next_id),
            range: node.range,
            syntax_kind: node.syntax_kind,
            rune: node.rune,
            signature: node.signature,
        })
        .collect::<Vec<_>>();

    let raw_symbols = scan_arm_symbols(source);
    let indents = raw_symbols
        .iter()
        .map(|symbol| symbol.indent)
        .collect::<Vec<_>>();
    let mut old_symbol_ids = previous
        .map(|snapshot| {
            signature_ids(
                snapshot
                    .symbols
                    .iter()
                    .map(|symbol| (&symbol.signature, symbol.id)),
            )
        })
        .unwrap_or_default();
    let mut symbols = raw_symbols
        .into_iter()
        .map(|symbol| SemanticSymbol {
            id: reused_or_new_id(&symbol.signature, &mut old_symbol_ids, next_id),
            name: symbol.name,
            kind: symbol.kind,
            detail: symbol.detail,
            range: symbol.range,
            selection_range: symbol.selection_range,
            parent: None,
            signature: symbol.signature,
        })
        .collect::<Vec<_>>();

    let mut hierarchy = Vec::<(usize, SemanticNodeId)>::new();
    for (symbol, indent) in symbols.iter_mut().zip(indents) {
        while hierarchy
            .last()
            .is_some_and(|(parent_indent, _)| *parent_indent >= indent)
        {
            hierarchy.pop();
        }
        symbol.parent = hierarchy.last().map(|(_, id)| *id);
        hierarchy.push((indent, symbol.id));
    }

    let mut raw_bindings = Vec::new();
    collect_bindings(&value, source, &lines, None, &mut raw_bindings);
    raw_bindings.sort_by_key(|binding| {
        (
            binding.declaration_range.start,
            binding.declaration_range.end,
            binding.scope_range.end,
            binding.name.clone(),
        )
    });
    raw_bindings.dedup_by(|right, left| {
        right.name == left.name
            && right.declaration_range == left.declaration_range
            && right.scope_range == left.scope_range
    });
    let mut occurrences = HashMap::<String, usize>::new();
    for binding in &mut raw_bindings {
        let occurrence = occurrences.entry(binding.name.clone()).or_default();
        binding.signature = format!("binding:{}:{}", binding.name, *occurrence);
        *occurrence += 1;
    }
    let mut old_binding_ids = previous
        .map(|snapshot| {
            signature_ids(
                snapshot
                    .bindings
                    .iter()
                    .map(|binding| (&binding.signature, binding.id)),
            )
        })
        .unwrap_or_default();
    let bindings = raw_bindings
        .into_iter()
        .map(|binding| SemanticBinding {
            id: reused_or_new_id(&binding.signature, &mut old_binding_ids, next_id),
            name: binding.name,
            declaration_range: binding.declaration_range,
            scope_range: binding.scope_range,
            signature: binding.signature,
        })
        .collect();

    Ok(SemanticSnapshot {
        path: path.to_path_buf(),
        version,
        nodes,
        symbols,
        bindings,
    })
}

fn signature_ids<'a>(
    values: impl IntoIterator<Item = (&'a String, SemanticNodeId)>,
) -> HashMap<String, VecDeque<SemanticNodeId>> {
    let mut out = HashMap::<String, VecDeque<SemanticNodeId>>::new();
    for (signature, id) in values {
        out.entry(signature.clone()).or_default().push_back(id);
    }
    out
}

fn reused_or_new_id(
    signature: &str,
    old_ids: &mut HashMap<String, VecDeque<SemanticNodeId>>,
    next_id: &mut u64,
) -> SemanticNodeId {
    if let Some(id) = old_ids.get_mut(signature).and_then(VecDeque::pop_front) {
        return id;
    }
    let id = SemanticNodeId(*next_id);
    *next_id = next_id.saturating_add(1);
    id
}

fn collect_traced_nodes(value: &Value, source: &str, lines: &LineIndex, nodes: &mut Vec<RawNode>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_traced_nodes(value, source, lines, nodes);
            }
        }
        Value::Object(values) => {
            if let Some(Value::Array(parts)) = values.get("Dbug") {
                if let [spot, inner] = parts.as_slice() {
                    if let (Some(range), Some(kind)) = (spot_range(spot, lines), syntax_kind(inner))
                    {
                        let start = range.start as usize;
                        let end = range.end as usize;
                        if start <= end && end <= source.len() {
                            let fingerprint = blake3::hash(&source.as_bytes()[start..end]);
                            nodes.push(RawNode {
                                range,
                                rune: rune_for_kind(kind).map(str::to_string),
                                syntax_kind: kind.to_string(),
                                signature: format!("node:{kind}:{fingerprint}"),
                            });
                        }
                    }
                }
            }
            for value in values.values() {
                collect_traced_nodes(value, source, lines, nodes);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Walk the serialized traced AST as an editor side table. Keeping this walk
/// separate from Hatch's AST types is intentional: the compiler consumes the
/// exact same tree as before, while editor analyses can grow their own
/// annotations and lifetimes around its stable serialized skeleton.
fn collect_bindings(
    value: &Value,
    source: &str,
    lines: &LineIndex,
    enclosing_range: Option<SemanticTextRange>,
    bindings: &mut Vec<RawBinding>,
) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_bindings(value, source, lines, enclosing_range, bindings);
            }
        }
        Value::Object(values) => {
            if let Some(Value::Array(parts)) = values.get("Dbug") {
                if let [spot, inner] = parts.as_slice() {
                    collect_bindings(
                        inner,
                        source,
                        lines,
                        spot_range(spot, lines).or(enclosing_range),
                        bindings,
                    );
                }
                return;
            }
            if values.len() == 1 {
                let (kind, payload) = values.iter().next().expect("single AST variant");
                if matches!(kind.as_str(), "Note" | "Gist" | "Help") {
                    if let Some(inner) = payload.as_array().and_then(|parts| parts.last()) {
                        collect_bindings(inner, source, lines, enclosing_range, bindings);
                    }
                    return;
                }

                match kind.as_str() {
                    "TisFas" => record_skin_body_binding(
                        payload, 0, 2, enclosing_range, source, lines, bindings,
                    ),
                    "TisMic" => record_skin_body_binding(
                        payload, 0, 1, enclosing_range, source, lines, bindings,
                    ),
                    "TisKet" => record_skin_body_binding(
                        payload, 0, 3, enclosing_range, source, lines, bindings,
                    ),
                    "TisTar" => record_named_body_binding(
                        payload, 0, 2, enclosing_range, source, lines, bindings,
                    ),
                    "BarSig" | "BarTar" | "BarTis" | "TisBar" => record_spec_body_binding(
                        payload, 0, 1, enclosing_range, source, lines, bindings,
                    ),
                    "BarCab" => record_core_sample_bindings(
                        payload, enclosing_range, source, lines, bindings,
                    ),
                    "WutHep" => record_clause_bindings(payload, 1, source, lines, bindings),
                    "WutLus" => record_clause_bindings(payload, 2, source, lines, bindings),
                    _ => {}
                }
            }

            for value in values.values() {
                collect_bindings(value, source, lines, enclosing_range, bindings);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn record_skin_body_binding(
    payload: &Value,
    skin_index: usize,
    body_index: usize,
    enclosing_range: Option<SemanticTextRange>,
    source: &str,
    lines: &LineIndex,
    bindings: &mut Vec<RawBinding>,
) {
    let Some(parts) = payload.as_array() else {
        return;
    };
    let (Some(skin), Some(body), Some(node_range)) = (
        parts.get(skin_index),
        parts.get(body_index),
        enclosing_range,
    ) else {
        return;
    };
    let Some(scope_range) = traced_range(body, lines) else {
        return;
    };
    let mut names = Vec::new();
    collect_skin_names(skin, &mut names);
    record_names(names, node_range, scope_range, source, bindings);
}

fn record_spec_body_binding(
    payload: &Value,
    spec_index: usize,
    body_index: usize,
    enclosing_range: Option<SemanticTextRange>,
    source: &str,
    lines: &LineIndex,
    bindings: &mut Vec<RawBinding>,
) {
    let Some(parts) = payload.as_array() else {
        return;
    };
    let (Some(spec), Some(body), Some(node_range)) = (
        parts.get(spec_index),
        parts.get(body_index),
        enclosing_range,
    ) else {
        return;
    };
    let Some(scope_range) = traced_range(body, lines) else {
        return;
    };
    let mut names = Vec::new();
    collect_spec_names(spec, &mut names);
    record_names(names, node_range, scope_range, source, bindings);
}

fn record_named_body_binding(
    payload: &Value,
    name_index: usize,
    body_index: usize,
    enclosing_range: Option<SemanticTextRange>,
    source: &str,
    lines: &LineIndex,
    bindings: &mut Vec<RawBinding>,
) {
    let Some(parts) = payload.as_array() else {
        return;
    };
    let (Some(name_payload), Some(body), Some(node_range)) = (
        parts.get(name_index),
        parts.get(body_index),
        enclosing_range,
    ) else {
        return;
    };
    let Some(scope_range) = traced_range(body, lines) else {
        return;
    };
    let Some(name) = name_payload
        .as_array()
        .and_then(|parts| parts.first())
        .and_then(Value::as_str)
    else {
        return;
    };
    record_names(
        vec![name.to_string()],
        node_range,
        scope_range,
        source,
        bindings,
    );
}

fn record_core_sample_bindings(
    payload: &Value,
    enclosing_range: Option<SemanticTextRange>,
    source: &str,
    lines: &LineIndex,
    bindings: &mut Vec<RawBinding>,
) {
    let (Some(parts), Some(node_range)) = (payload.as_array(), enclosing_range) else {
        return;
    };
    let Some(spec) = parts.first() else {
        return;
    };
    let Some(scope_start) = parts
        .iter()
        .skip(1)
        .filter_map(|value| earliest_traced_range(value, lines))
        .map(|range| range.start)
        .min()
    else {
        return;
    };
    let scope_range = SemanticTextRange {
        start: scope_start,
        end: node_range.end,
    };
    let mut names = Vec::new();
    collect_spec_names(spec, &mut names);
    record_names(names, node_range, scope_range, source, bindings);
}

fn record_clause_bindings(
    payload: &Value,
    clauses_index: usize,
    source: &str,
    lines: &LineIndex,
    bindings: &mut Vec<RawBinding>,
) {
    let Some(clauses) = payload
        .as_array()
        .and_then(|parts| parts.get(clauses_index))
        .and_then(Value::as_array)
    else {
        return;
    };
    for clause in clauses {
        let Some(pair) = clause.as_array() else {
            continue;
        };
        let (Some(spec), Some(body)) = (pair.first(), pair.get(1)) else {
            continue;
        };
        let (Some(declaration_area), Some(scope_range)) =
            (traced_range(spec, lines), traced_range(body, lines))
        else {
            continue;
        };
        let mut names = Vec::new();
        collect_spec_names(spec, &mut names);
        record_names(names, declaration_area, scope_range, source, bindings);
    }
}

fn record_names(
    names: Vec<String>,
    node_range: SemanticTextRange,
    scope_range: SemanticTextRange,
    source: &str,
    bindings: &mut Vec<RawBinding>,
) {
    let declaration_area = SemanticTextRange {
        start: node_range.start,
        end: scope_range.start.min(node_range.end),
    };
    for name in names {
        let Some(declaration_range) = find_term_in_range(source, &name, declaration_area) else {
            continue;
        };
        bindings.push(RawBinding {
            name,
            declaration_range,
            scope_range,
            signature: String::new(),
        });
    }
}

fn traced_range(value: &Value, lines: &LineIndex) -> Option<SemanticTextRange> {
    let values = value.as_object()?;
    if let Some(parts) = values.get("Dbug").and_then(Value::as_array) {
        return parts.first().and_then(|spot| spot_range(spot, lines));
    }
    if values.len() == 1 {
        let (kind, payload) = values.iter().next()?;
        if matches!(kind.as_str(), "Note" | "Gist" | "Help") {
            return payload
                .as_array()
                .and_then(|parts| parts.last())
                .and_then(|inner| traced_range(inner, lines));
        }
    }
    None
}

fn earliest_traced_range(value: &Value, lines: &LineIndex) -> Option<SemanticTextRange> {
    if let Some(range) = traced_range(value, lines) {
        return Some(range);
    }
    match value {
        Value::Array(values) => values
            .iter()
            .filter_map(|value| earliest_traced_range(value, lines))
            .min_by_key(|range| range.start),
        Value::Object(values) => values
            .values()
            .filter_map(|value| earliest_traced_range(value, lines))
            .min_by_key(|range| range.start),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    }
}

fn collect_skin_names(value: &Value, names: &mut Vec<String>) {
    let Some(values) = value.as_object() else {
        return;
    };
    let Some((kind, payload)) = values.iter().next() else {
        return;
    };
    match kind.as_str() {
        "Term" => push_binding_name(payload, names),
        "Cell" => for_each_array(payload, |value| collect_skin_names(value, names)),
        "Dbug" | "Help" | "Over" => {
            if let Some(inner) = payload.as_array().and_then(|parts| parts.last()) {
                collect_skin_names(inner, names);
            }
        }
        "Name" => {
            if let Some(parts) = payload.as_array() {
                if let Some(name) = parts.first() {
                    push_binding_name(name, names);
                }
                if let Some(inner) = parts.get(1) {
                    collect_skin_names(inner, names);
                }
            }
        }
        "Spec" => {
            if let Some(parts) = payload.as_array() {
                if let Some(spec) = parts.first() {
                    collect_spec_names(spec, names);
                }
                if let Some(inner) = parts.get(1) {
                    collect_skin_names(inner, names);
                }
            }
        }
        _ => {}
    }
}

fn collect_spec_names(value: &Value, names: &mut Vec<String>) {
    let Some(values) = value.as_object() else {
        return;
    };
    let Some((kind, payload)) = values.iter().next() else {
        return;
    };
    match kind.as_str() {
        "Dbug" | "Gist" | "Made" | "Over" => {
            if let Some(inner) = payload.as_array().and_then(|parts| parts.last()) {
                collect_spec_names(inner, names);
            }
        }
        "Make" => {
            if let Some(specs) = payload.as_array().and_then(|parts| parts.get(1)) {
                for_each_array(specs, |spec| collect_spec_names(spec, names));
            }
        }
        "Name" => {
            if let Some(parts) = payload.as_array() {
                if let Some(name) = parts.first() {
                    push_binding_name(name, names);
                }
                if let Some(inner) = parts.get(1) {
                    collect_spec_names(inner, names);
                }
            }
        }
        "BucGar" | "BucGal" | "BucHep" | "BucKet" | "BucPat" => {
            for_each_array(payload, |spec| collect_spec_names(spec, names));
        }
        "BucBuc" | "BucDot" | "BucFas" | "BucTic" | "BucZap" => {
            if let Some(parts) = payload.as_array() {
                if let Some(spec) = parts.first() {
                    collect_spec_names(spec, names);
                }
                if let Some(specs) = parts.get(1).and_then(Value::as_object) {
                    for spec in specs.values() {
                        collect_spec_names(spec, names);
                    }
                }
            }
        }
        "BucBar" | "BucPam" => {
            if let Some(spec) = payload.as_array().and_then(|parts| parts.first()) {
                collect_spec_names(spec, names);
            }
        }
        "BucCol" | "BucCen" | "BucWut" => {
            if let Some(parts) = payload.as_array() {
                if let Some(spec) = parts.first() {
                    collect_spec_names(spec, names);
                }
                if let Some(specs) = parts.get(1) {
                    for_each_array(specs, |spec| collect_spec_names(spec, names));
                }
            }
        }
        "BucLus" | "BucSig" => {
            if let Some(spec) = payload.as_array().and_then(|parts| parts.get(1)) {
                collect_spec_names(spec, names);
            }
        }
        "BucTis" => {
            if let Some(parts) = payload.as_array() {
                if let Some(skin) = parts.first() {
                    collect_skin_names(skin, names);
                }
                if let Some(spec) = parts.get(1) {
                    collect_spec_names(spec, names);
                }
            }
        }
        _ => {}
    }
}

fn for_each_array(value: &Value, mut visit: impl FnMut(&Value)) {
    if let Some(values) = value.as_array() {
        for value in values {
            visit(value);
        }
    }
}

fn push_binding_name(value: &Value, names: &mut Vec<String>) {
    let Some(name) = value.as_str() else {
        return;
    };
    if name.as_bytes().first().is_some_and(u8::is_ascii_lowercase) {
        names.push(name.to_string());
    }
}

fn find_term_in_range(
    source: &str,
    name: &str,
    range: SemanticTextRange,
) -> Option<SemanticTextRange> {
    let start = usize::try_from(range.start).ok()?;
    let end = usize::try_from(range.end).ok()?;
    let haystack = source.get(start..end)?;
    for (relative, _) in haystack.match_indices(name) {
        let match_start = start + relative;
        let match_end = match_start + name.len();
        let left_boundary =
            match_start == 0 || !is_hoon_term_byte(*source.as_bytes().get(match_start - 1)?);
        let right_boundary =
            match_end == source.len() || !is_hoon_term_byte(*source.as_bytes().get(match_end)?);
        if left_boundary && right_boundary {
            return Some(SemanticTextRange {
                start: u32::try_from(match_start).ok()?,
                end: u32::try_from(match_end).ok()?,
            });
        }
    }
    None
}

fn syntax_kind(value: &Value) -> Option<&str> {
    match value {
        Value::String(kind) => Some(kind.as_str()),
        Value::Object(values) if values.len() == 1 => {
            let (kind, payload) = values.iter().next()?;
            if matches!(kind.as_str(), "Dbug" | "Note" | "Gist" | "Help") {
                if let Value::Array(parts) = payload {
                    return parts.last().and_then(syntax_kind);
                }
            }
            Some(kind.as_str())
        }
        _ => None,
    }
}

fn spot_range(value: &Value, lines: &LineIndex) -> Option<SemanticTextRange> {
    let pint = value.get("q")?;
    let start = json_line_col(pint.get("p")?)?;
    let end = json_line_col(pint.get("q")?)?;
    let start = u32::try_from(lines.byte_offset(start.0, start.1)?).ok()?;
    let end = u32::try_from(lines.byte_offset(end.0, end.1)?).ok()?;
    Some(SemanticTextRange {
        start,
        end: end
            .max(start.saturating_add(1))
            .min(lines.source_len as u32),
    })
}

fn json_line_col(value: &Value) -> Option<(u64, u64)> {
    let values = value.as_array()?;
    Some((values.first()?.as_u64()?, values.get(1)?.as_u64()?))
}

fn rune_for_kind(kind: &str) -> Option<&'static str> {
    const SYLLABLES: &[(&str, &str)] = &[
        ("Bar", "|"),
        ("Buc", "$"),
        ("Cab", "_"),
        ("Cen", "%"),
        ("Col", ":"),
        ("Com", ","),
        ("Dot", "."),
        ("Fas", "/"),
        ("Gal", "<"),
        ("Gar", ">"),
        ("Hax", "#"),
        ("Hep", "-"),
        ("Ket", "^"),
        ("Lus", "+"),
        ("Mic", ";"),
        ("Pam", "&"),
        ("Pat", "@"),
        ("Sig", "~"),
        ("Tar", "*"),
        ("Tic", "`"),
        ("Tis", "="),
        ("Wut", "?"),
        ("Zap", "!"),
    ];
    for (first_name, first_rune) in SYLLABLES {
        let Some(rest) = kind.strip_prefix(first_name) else {
            continue;
        };
        for (second_name, second_rune) in SYLLABLES {
            if rest == *second_name {
                return match (*first_rune, *second_rune) {
                    ("|", "=") => Some("|="),
                    ("|", "%") => Some("|%"),
                    ("=", "+") => Some("=+"),
                    ("=", "-") => Some("=-"),
                    ("=", "<") => Some("=<"),
                    ("=", ">") => Some("=>"),
                    ("^", "-") => Some("^-"),
                    ("^", "=") => Some("^="),
                    ("?", ":") => Some("?:"),
                    ("?", "=") => Some("?="),
                    ("?", "^") => Some("?^"),
                    ("?", "@") => Some("?@"),
                    ("~", "=") => Some("~="),
                    ("~", "+") => Some("~+"),
                    ("!", "=") => Some("!="),
                    ("!", "?") => Some("!?"),
                    (":", "*") => Some(":*"),
                    ("%", "-") => Some("%-"),
                    ("%", "+") => Some("%+"),
                    (".", "^") => Some(".^"),
                    (".", "+") => Some(".+"),
                    (";", ":") => Some(";:"),
                    (";", "~") => Some(";~"),
                    _ => None,
                };
            }
        }
    }
    None
}

fn scan_arm_symbols(source: &str) -> Vec<RawSymbol> {
    let mut headers = scan_arm_headers(source);
    for index in 0..headers.len() {
        let end = headers[index + 1..]
            .iter()
            .find(|next| next.indent <= headers[index].indent)
            .map_or(source.len(), |next| next.range.start as usize);
        headers[index].range.end = trim_end(source, end) as u32;
    }
    headers
}

fn scan_arm_headers(source: &str) -> Vec<RawSymbol> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    let mut triple: Option<&str> = None;
    let mut occurrences = HashMap::<String, usize>::new();
    let mut hierarchy = Vec::<(usize, String)>::new();

    for line in source.split_inclusive('\n') {
        let line_without_newline = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = line_without_newline.trim_start_matches([' ', '\t']);
        let indent = line_without_newline.len().saturating_sub(trimmed.len());

        if let Some(delimiter) = triple {
            if trimmed.contains(delimiter) {
                triple = None;
            }
            offset += line.len();
            continue;
        }
        if let Some(remainder) = trimmed.strip_prefix("\"\"\"") {
            if !remainder.contains("\"\"\"") {
                triple = Some("\"\"\"");
            }
            offset += line.len();
            continue;
        }
        if let Some(remainder) = trimmed.strip_prefix("'''") {
            if !remainder.contains("'''") {
                triple = Some("'''");
            }
            offset += line.len();
            continue;
        }

        let bytes = trimmed.as_bytes();
        let is_arm = bytes.len() >= 3
            && bytes[0] == b'+'
            && matches!(bytes[1], b'+' | b'$' | b'*' | b'|')
            && matches!(bytes[2], b' ' | b'\t');
        if is_arm {
            let mut cursor = 2;
            while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            let name_start = cursor;
            while bytes
                .get(cursor)
                .is_some_and(|byte| !byte.is_ascii_whitespace())
            {
                cursor += 1;
            }
            if cursor > name_start {
                let name = &trimmed[name_start..cursor];
                let detail = &trimmed[..2];
                while hierarchy
                    .last()
                    .is_some_and(|(parent_indent, _)| *parent_indent >= indent)
                {
                    hierarchy.pop();
                }
                let ancestry = hierarchy
                    .iter()
                    .map(|(_, identity)| identity.as_str())
                    .collect::<Vec<_>>()
                    .join("/");
                let identity = format!("{detail}:{name}");
                let base_signature = format!("symbol:{ancestry}/{identity}");
                let occurrence = occurrences.entry(base_signature.clone()).or_default();
                let signature = format!("{base_signature}:{}", *occurrence);
                *occurrence += 1;
                hierarchy.push((indent, identity));
                let header_start = offset + indent;
                let selection_start = header_start + name_start;
                out.push(RawSymbol {
                    name: name.to_string(),
                    kind: if detail == "+$" {
                        SemanticSymbolKind::Mold
                    } else {
                        SemanticSymbolKind::Arm
                    },
                    detail: detail.to_string(),
                    range: SemanticTextRange {
                        start: header_start as u32,
                        end: (offset + line_without_newline.len()) as u32,
                    },
                    selection_range: SemanticTextRange {
                        start: selection_start as u32,
                        end: (selection_start + name.len()) as u32,
                    },
                    indent,
                    signature,
                });
            }
        }
        offset += line.len();
    }
    out
}

fn trim_end(source: &str, mut end: usize) -> usize {
    while end > 0 && source.as_bytes()[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end
}

struct LineIndex {
    starts: Vec<usize>,
    col_offsets: Vec<usize>,
    source_len: usize,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            source
                .bytes()
                .enumerate()
                .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
        );

        // Hatch normalizes columns inside tall tapes by removing their outer
        // indentation. Reconstruct the same offsets here so debug spots map
        // back onto the original source bytes.
        let bytes = source.as_bytes();
        let mut col_offsets = vec![0; starts.len()];
        let mut in_tall_tape = false;
        let mut tall_indent = 0usize;
        for line_index in 0..starts.len() {
            let start = starts[line_index];
            let mut end = starts.get(line_index + 1).copied().unwrap_or(bytes.len());
            if end > start && bytes[end - 1] == b'\n' {
                end -= 1;
            }
            let line = &bytes[start..end];
            let indent = line
                .iter()
                .take_while(|byte| matches!(byte, b' ' | b'\t'))
                .count();
            let trimmed = &line[indent..];
            if !in_tall_tape {
                if trimmed.starts_with(b"\"\"\"") {
                    in_tall_tape = true;
                    tall_indent = indent;
                }
            } else if indent == tall_indent && trimmed.starts_with(b"\"\"\"") {
                in_tall_tape = false;
            } else {
                col_offsets[line_index] = tall_indent;
            }
        }
        Self {
            starts,
            col_offsets,
            source_len: source.len(),
        }
    }

    fn byte_offset(&self, one_based_line: u64, one_based_column: u64) -> Option<usize> {
        let line = usize::try_from(one_based_line.checked_sub(1)?).ok()?;
        let column = usize::try_from(one_based_column.checked_sub(1)?).ok()?;
        let start = *self.starts.get(line)?;
        let limit = self
            .starts
            .get(line + 1)
            .copied()
            .map(|next| next.saturating_sub(1))
            .unwrap_or(self.source_len);
        Some(
            start
                .saturating_add(column)
                .saturating_add(self.col_offsets.get(line).copied().unwrap_or(0))
                .min(limit),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        hoon_rune_at, range_from_one_based_spot, scan_arm_headers, structural_definition,
        structural_rune_definition, LineIndex, SemanticSession, SemanticSymbolKind,
        SemanticTextRange,
    };

    const SOURCE: &str = "|%\n++  answer\n  42\n+$  pair\n  $:  left=@  right=@  ==\n--\n";

    #[test]
    fn compiler_spots_map_to_source_bytes() {
        assert_eq!(
            range_from_one_based_spot(SOURCE, 3, 3, 3, 5),
            Some(SemanticTextRange {
                start: u32::try_from(SOURCE.find("42").expect("constant offset"))
                    .expect("small source"),
                end: u32::try_from(SOURCE.find("42").expect("constant offset") + 2)
                    .expect("small source"),
            })
        );
        assert_eq!(range_from_one_based_spot(SOURCE, 0, 1, 1, 1), None);
    }

    #[test]
    fn semantic_snapshot_indexes_arms_and_hover() {
        let mut session = SemanticSession::default();
        let snapshot = session
            .snapshot(Path::new("/tmp/semantic.hoon"), 1, SOURCE)
            .expect("semantic snapshot");

        assert_eq!(snapshot.symbols.len(), 2);
        assert_eq!(snapshot.symbols[0].name, "answer");
        assert_eq!(snapshot.symbols[0].kind, SemanticSymbolKind::Arm);
        assert_eq!(snapshot.symbols[1].name, "pair");
        assert_eq!(snapshot.symbols[1].kind, SemanticSymbolKind::Mold);
        assert!(!snapshot.nodes.is_empty());

        let answer_offset =
            u32::try_from(SOURCE.find("answer").expect("answer offset")).expect("small source");
        let hover = snapshot.hover(answer_offset).expect("answer hover");
        assert!(hover.markdown.contains("answer"));
        assert!(hover.markdown.contains("++"));

        let body_offset =
            u32::try_from(SOURCE.find("42").expect("body offset")).expect("small source");
        let body_hover = snapshot.hover(body_offset).expect("body hover");
        assert!(body_hover.markdown.contains("Hoon syntax"));
        assert!(!body_hover.markdown.contains("answer"));
    }

    #[test]
    fn structural_definition_resolves_unique_hyphenated_symbols_only() {
        let source =
            "|%\n+$  kernel-state  [%state version=%1]\n++  moat  (keep kernel-state)\n--\n";
        let mut session = SemanticSession::default();
        let mut snapshot = session
            .snapshot(Path::new("/tmp/definition.hoon"), 1, source)
            .expect("semantic snapshot")
            .clone();
        let offset =
            u32::try_from(source.rfind("kernel-state").expect("use offset")).expect("small source");
        let definition = snapshot
            .definition(source, offset)
            .expect("unique structural definition");
        let start = usize::try_from(definition.start).expect("small source");
        let end = usize::try_from(definition.end).expect("small source");
        assert_eq!(&source[start..end], "kernel-state");

        let duplicate = snapshot
            .symbols
            .iter()
            .find(|symbol| symbol.name == "kernel-state")
            .expect("kernel-state symbol")
            .clone();
        snapshot.symbols.push(duplicate);
        assert!(snapshot.definition(source, offset).is_none());

        let lightweight =
            structural_definition(source, "kernel-state").expect("lightweight definition");
        assert_eq!(
            &source[lightweight.start as usize..lightweight.end as usize],
            "kernel-state"
        );
        let duplicated_source = source.replace("++  moat", "+$  kernel-state  @\n++  moat");
        assert!(structural_definition(&duplicated_source, "kernel-state").is_none());
    }

    #[test]
    fn structural_rune_definition_resolves_both_glyph_positions() {
        let use_source = "  ^-  [(list effect) state]\n";
        let rune_start = use_source.find("^-").expect("rune start");
        assert_eq!(
            hoon_rune_at(use_source, u32::try_from(rune_start).expect("small source")),
            Some("^-")
        );
        assert_eq!(
            hoon_rune_at(
                use_source,
                u32::try_from(rune_start + 1).expect("small source")
            ),
            Some("^-")
        );

        let prelude = concat!(
            "++  bola  ::  ++ arms\n  parser\n", "++  boba  ::  +$ arms\n  parser\n",
            "++  whip  ::  +| chapter declare\n  parser\n",
            "  [%ktls p=hoon q=hoon] :: ^+ expression cast\n",
            "  [%kthp p=spec q=hoon] :: ^- structure cast\n",
        );
        let definition = structural_rune_definition(prelude, "^-").expect("ket-hep definition");
        assert_eq!(
            &prelude[definition.start as usize..definition.end as usize],
            "kthp"
        );
        for (rune, parser_arm) in [("++", "bola"), ("+$", "boba"), ("+|", "whip")] {
            let definition = structural_rune_definition(prelude, rune)
                .unwrap_or_else(|| panic!("{rune} parser definition"));
            assert_eq!(
                &prelude[definition.start as usize..definition.end as usize],
                parser_arm
            );
        }
        assert!(structural_rune_definition(prelude, "::").is_none());
    }

    #[test]
    fn symbol_ids_survive_position_and_body_changes() {
        let path = Path::new("/tmp/stable.hoon");
        let mut session = SemanticSession::default();
        let first = session
            .snapshot(path, 1, SOURCE)
            .expect("first snapshot")
            .symbols[0]
            .clone();
        let edited = SOURCE.replace("|%\n", "|%\n\n").replace("  42", "  43");
        let second = session
            .snapshot(path, 2, edited.as_str())
            .expect("second snapshot")
            .symbols[0]
            .clone();

        assert_eq!(first.id, second.id);
        assert_ne!(first.selection_range, second.selection_range);
    }

    #[test]
    fn malformed_snapshot_recovers_at_the_next_revision() {
        let path = Path::new("/tmp/recovery.hoon");
        let mut session = SemanticSession::default();
        assert!(session.snapshot(path, 1, "|=  [a=@\n").is_err());
        assert!(session.snapshot(path, 1, "|=  [a=@\n").is_err());
        assert!(session.snapshot(path, 2, "|=  a=@\n  a\n").is_ok());
    }

    #[test]
    fn lexical_bindings_resolve_innermost_scope_and_keep_stable_ids() {
        let source = concat!(
            "=/  value  1\n", "=/  before  value\n", "=/  result\n", "  =/  value  2\n",
            "  value\n", "[before result value]\n",
        );
        let path = Path::new("/tmp/bindings.hoon");
        let mut session = SemanticSession::default();
        let first = session
            .snapshot(path, 1, source)
            .expect("binding snapshot")
            .clone();
        assert_eq!(first.bindings.len(), 4);

        let outer_declaration = source.find("value").expect("outer declaration");
        let inner_declaration =
            source.find("  =/  value").expect("inner declaration") + "  =/  ".len();
        let inner_use = source.rfind("  value\n").expect("inner use") + 2;
        let final_use = source.rfind("value]").expect("final use");
        assert_eq!(
            first
                .definition(source, u32::try_from(inner_use).expect("small source"))
                .expect("inner definition")
                .start,
            u32::try_from(inner_declaration).expect("small source")
        );
        assert_eq!(
            first
                .definition(source, u32::try_from(final_use).expect("small source"))
                .expect("outer definition")
                .start,
            u32::try_from(outer_declaration).expect("small source")
        );
        assert!(first
            .definition(
                source,
                u32::try_from(outer_declaration).expect("small source")
            )
            .is_none());

        let edited = format!("\n{}", source.replace("  1\n", "  42\n"));
        let second = session
            .snapshot(path, 2, &edited)
            .expect("edited binding snapshot");
        assert_eq!(first.bindings[0].id, second.bindings[0].id);
        assert_ne!(
            first.bindings[0].declaration_range,
            second.bindings[0].declaration_range
        );

        let core_source = "|_  value=value\n++  read\n  value\n--\n";
        let core = session
            .snapshot(Path::new("/tmp/core-bindings.hoon"), 1, core_source)
            .expect("core binding snapshot");
        let declaration = core_source.find("value").expect("sample declaration");
        let mold = core_source
            .find("=value")
            .expect("sample mold")
            .saturating_add(1);
        let body = core_source.rfind("value").expect("sample body");
        assert!(core
            .definition(core_source, u32::try_from(mold).expect("small source"))
            .is_none());
        assert_eq!(
            core.definition(core_source, u32::try_from(body).expect("small source"))
                .expect("core sample definition")
                .start,
            u32::try_from(declaration).expect("small source")
        );
    }

    #[test]
    fn nested_arms_form_a_symbol_hierarchy() {
        let source = "|%\n++  outer\n  |%\n  ++  inner\n    42\n  --\n--\n";
        let mut session = SemanticSession::default();
        let snapshot = session
            .snapshot(Path::new("/tmp/nested.hoon"), 1, source)
            .expect("nested semantic snapshot");

        assert_eq!(snapshot.symbols.len(), 2);
        assert_eq!(snapshot.symbols[0].name, "outer");
        assert_eq!(snapshot.symbols[1].name, "inner");
        assert_eq!(snapshot.symbols[1].parent, Some(snapshot.symbols[0].id));
    }

    #[test]
    fn tall_tape_columns_map_back_to_original_source_bytes() {
        let source = "  \"\"\"\n    content\n  \"\"\"\n";
        let lines = LineIndex::new(source);
        let content = source.find("content").expect("content offset");

        assert_eq!(lines.byte_offset(2, 3), Some(content));
    }

    #[test]
    fn single_line_triple_delimiters_do_not_hide_later_arms() {
        let source = "\"\"\"inline\"\"\"\n++  visible\n  42\n";
        let headers = scan_arm_headers(source);

        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].name, "visible");
    }
}
