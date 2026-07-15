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

    Ok(SemanticSnapshot {
        path: path.to_path_buf(),
        version,
        nodes,
        symbols,
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
        range_from_one_based_spot, scan_arm_headers, LineIndex, SemanticSession,
        SemanticSymbolKind, SemanticTextRange,
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
