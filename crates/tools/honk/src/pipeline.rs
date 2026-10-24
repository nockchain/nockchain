use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use chumsky::Parser;
use hatch::ast::hoon as ast;
use hatch::native_parser;
use hatch::utils::LineMap;
use num_bigint::BigUint;

use crate::errors::{CompilerError, CompilerErrorLocation, CompilerErrorMetadata, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeMode {
    Standard,
    Urbit,
}

#[derive(Clone)]
pub struct CompileRequest {
    pub entry: PathBuf,
    pub deps_dir: PathBuf,
    pub out_dir: Option<PathBuf>,
    pub arbitrary: bool,
    pub dynock: bool,
    pub dynock_typed: bool,
    pub new: bool,
}

impl CompileRequest {
    pub fn new(entry: PathBuf, deps_dir: PathBuf) -> Self {
        Self {
            entry,
            deps_dir,
            out_dir: None,
            arbitrary: false,
            dynock: false,
            dynock_typed: false,
            new: false,
        }
    }
}

pub async fn build_jam(req: CompileRequest) -> Result<Vec<u8>> {
    let jam = hoonc::build_jam(
        &req.entry, req.deps_dir, req.out_dir, req.arbitrary, req.dynock, req.dynock_typed, req.new,
    )
    .await
    .map_err(|err| CompilerError::Backend(err.to_string()))?;
    Ok(jam)
}

pub fn parse_native_hoon(path: &Path, deps_dir: &Path, dbug: bool) -> Result<ast::Hoon> {
    parse_native_hoon_with_mode(path, deps_dir, dbug, ScopeMode::Standard)
}

pub fn parse_native_hoon_with_mode(
    path: &Path,
    deps_dir: &Path,
    dbug: bool,
    scope_mode: ScopeMode,
) -> Result<ast::Hoon> {
    let wer_base = wer_base_dir_for_mode(path, deps_dir, scope_mode);
    let mut resolver = NativeImportResolver::new(wer_base, dbug, scope_mode);
    resolver.parse(path)
}

pub fn parse_native_hoon_leaf(path: &Path, deps_dir: &Path, dbug: bool) -> Result<ast::Hoon> {
    parse_native_hoon_leaf_with_mode(path, deps_dir, dbug, ScopeMode::Standard)
}

pub fn parse_native_hoon_leaf_with_mode(
    path: &Path,
    deps_dir: &Path,
    dbug: bool,
    scope_mode: ScopeMode,
) -> Result<ast::Hoon> {
    let source = std::fs::read_to_string(path)?;
    let source = sanitize_urbit_sys_header(scope_mode, path, source.as_str());
    let wer_base = wer_base_dir_for_mode(path, deps_dir, scope_mode);
    let wer = hoon_path_for_any(path, &wer_base);
    let expr = parse_native_hoon_source_with_wer_and_dbug(path, source.as_str(), wer, dbug)?;
    Ok(NativeImportResolver::new(wer_base, dbug, scope_mode)
        .synthetic_urbit_scope_faces(path, expr))
}

pub fn parse_native_hoon_source(
    path: &Path,
    source: &str,
    wer: Vec<String>,
    dbug: bool,
) -> Result<ast::Hoon> {
    parse_native_hoon_source_with_wer_dbug_and_docs(path, source, wer, dbug, true)
}

pub fn parse_native_hoon_source_without_docs(
    path: &Path,
    source: &str,
    wer: Vec<String>,
    dbug: bool,
) -> Result<ast::Hoon> {
    parse_native_hoon_source_with_wer_dbug_and_docs(path, source, wer, dbug, false)
}

pub fn resolve_native_imports(
    path: &Path,
    deps_dir: &Path,
    scope_mode: ScopeMode,
) -> Result<Vec<ResolvedNativeImport>> {
    let wer_base = wer_base_dir_for_mode(path, deps_dir, scope_mode);
    let resolver = NativeImportResolver::new(wer_base, true, scope_mode);
    resolver.resolve_imports_for(path, true)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImportKind {
    Lib,
    Raw,
    Sur,
    Sys,
    Dat,
    Bar,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScopedImport {
    kind: ImportKind,
    face: Option<String>,
    mark: Option<String>,
    suffix: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeImportKind {
    Hoon,
    Data,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedNativeImport {
    pub kind: NativeImportKind,
    pub face: Option<String>,
    pub path: PathBuf,
}

struct NativeImportResolver {
    wer_base: PathBuf,
    dbug: bool,
    scope_mode: ScopeMode,
    cache: HashMap<PathBuf, ast::Hoon>,
    visiting: HashSet<PathBuf>,
}

impl NativeImportResolver {
    fn new(wer_base: PathBuf, dbug: bool, scope_mode: ScopeMode) -> Self {
        Self {
            wer_base,
            dbug,
            scope_mode,
            cache: HashMap::new(),
            visiting: HashSet::new(),
        }
    }

    fn parse(&mut self, path: &Path) -> Result<ast::Hoon> {
        let canonical = path.canonicalize()?;
        if let Some(cached) = self.cache.get(&canonical) {
            return Ok(cached.clone());
        }
        if self.visiting.contains(&canonical) {
            return Err(CompilerError::Parse(format!(
                "cyclic native import detected at {}",
                canonical.display()
            )));
        }
        let is_root = self.visiting.is_empty();
        self.visiting.insert(canonical.clone());

        let parsed = self.parse_uncached(path, is_root);
        let _ = self.visiting.remove(&canonical);
        if let Ok(expr) = &parsed {
            self.cache.insert(canonical, expr.clone());
        }
        parsed
    }

    fn parse_uncached(&mut self, path: &Path, is_root: bool) -> Result<ast::Hoon> {
        let source = std::fs::read_to_string(path)?;
        let source = sanitize_urbit_sys_header(self.scope_mode, path, source.as_str());
        let wer = hoon_path_for_any(path, &self.wer_base);
        let mut expr =
            parse_native_hoon_source_with_wer_and_dbug(path, source.as_str(), wer, self.dbug)?;
        expr = self.synthetic_urbit_scope_faces(path, expr);

        let mut imports = parse_leading_imports(source.as_str())?;
        let synthetic = self.synthetic_urbit_scope_imports(path, is_root);
        if !synthetic.is_empty() {
            imports.splice(0..0, synthetic);
        }

        for import in imports.iter().rev() {
            let dep_path = self.resolve_import(path, import.kind, import.suffix.as_str())?;
            let dep_expr = if import.kind == ImportKind::Bar {
                data_import_expr(dep_path.as_path())?
            } else {
                self.parse(dep_path.as_path())?
            };
            expr = match &import.face {
                Some(face) => ast::Hoon::TisLus(
                    Box::new(ast::Hoon::KetTis(
                        ast::Skin::Term(face.clone()),
                        Box::new(dep_expr),
                    )),
                    Box::new(expr),
                ),
                None => ast::Hoon::TisLus(Box::new(dep_expr), Box::new(expr)),
            };
        }
        Ok(expr)
    }

    fn resolve_imports_for(&self, path: &Path, is_root: bool) -> Result<Vec<ResolvedNativeImport>> {
        let source = std::fs::read_to_string(path)?;
        let source = sanitize_urbit_sys_header(self.scope_mode, path, source.as_str());
        let mut imports = parse_leading_imports(source.as_str())?;
        let synthetic = self.synthetic_urbit_scope_imports(path, is_root);
        if !synthetic.is_empty() {
            imports.splice(0..0, synthetic);
        }

        imports
            .into_iter()
            .map(|import| {
                let kind = match import.kind {
                    ImportKind::Bar => {
                        // `/*` data imports must declare a mark honk supports.
                        // Only `%jam` (raw jammed-noun inclusion) is
                        // implemented; reject others rather than silently
                        // importing them as generic data.
                        match import.mark.as_deref() {
                            Some("jam") => NativeImportKind::Data,
                            other => {
                                return Err(CompilerError::UnsupportedExpr(format!(
                                    "/* import `{}` uses mark {} which honk does not support (only %jam)",
                                    import.suffix,
                                    other
                                        .map(|m| format!("%{m}"))
                                        .unwrap_or_else(|| "<none>".to_string()),
                                )));
                            }
                        }
                    }
                    _ => NativeImportKind::Hoon,
                };
                let path = self.resolve_import(path, import.kind, import.suffix.as_str())?;
                Ok(ResolvedNativeImport {
                    kind,
                    face: import.face,
                    path,
                })
            })
            .collect()
    }

    fn resolve_import(&self, from: &Path, kind: ImportKind, suffix: &str) -> Result<PathBuf> {
        match kind {
            ImportKind::Raw => {
                let mut candidate = PathBuf::new();
                for segment in suffix.trim_start_matches('/').split('/') {
                    if !segment.is_empty() {
                        candidate.push(segment);
                    }
                }
                if let Some(file_name) = candidate.file_name().and_then(|name| name.to_str()) {
                    candidate.set_file_name(format!("{file_name}.hoon"));
                }
                let path = self.wer_base.join(candidate);
                if path.is_file() {
                    return Ok(path);
                }
            }
            ImportKind::Bar => {
                let mut segments: Vec<&str> = suffix
                    .trim_start_matches('/')
                    .split('/')
                    .filter(|segment| !segment.is_empty())
                    .collect();
                if segments.len() >= 2 {
                    let extension = segments.pop().unwrap_or_default();
                    let stem = segments.pop().unwrap_or_default();
                    let mut candidate = PathBuf::new();
                    for segment in segments {
                        candidate.push(segment);
                    }
                    candidate.push(format!("{stem}.{extension}"));
                    let path = self.wer_base.join(candidate);
                    if path.is_file() {
                        return Ok(path);
                    }
                }
            }
            _ => {
                let prefix = match kind {
                    ImportKind::Lib => "lib",
                    ImportKind::Sur => "sur",
                    ImportKind::Sys => "sys",
                    ImportKind::Dat => "dat",
                    ImportKind::Raw => unreachable!("raw import handled above"),
                    ImportKind::Bar => unreachable!("bar import handled above"),
                };
                for candidate in suffix_path_candidates(prefix, suffix) {
                    let path = self.wer_base.join(candidate);
                    if path.is_file() {
                        return Ok(path);
                    }
                }
            }
        }
        if kind == ImportKind::Sys && suffix == "hoon" {
            if let Some(path) = vendored_hoon_sys_path() {
                return Ok(path.canonicalize().unwrap_or(path));
            }
        }
        let rune = match kind {
            ImportKind::Lib => "/+",
            ImportKind::Raw => "/=",
            ImportKind::Sur => "/-",
            ImportKind::Sys => "/sys",
            ImportKind::Dat => "/#",
            ImportKind::Bar => "/*",
        };
        Err(CompilerError::Parse(format!(
            "native import not found: `{rune} {suffix}` (from {})",
            from.display()
        )))
    }

    fn synthetic_urbit_scope_faces(&self, path: &Path, expr: ast::Hoon) -> ast::Hoon {
        if self.scope_mode != ScopeMode::Urbit {
            return expr;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            return expr;
        };
        let faces: &[&str] = match stem {
            // /sys/hoon and /sys/arvo expect +ride in ambient context.
            "hoon" => &["ride"],
            "arvo" => &["ride", "zuse"],
            _ => &[],
        };
        faces.iter().rev().fold(expr, |inner, face| {
            ast::Hoon::TisLus(
                Box::new(ast::Hoon::KetTis(
                    ast::Skin::Term((*face).to_string()),
                    Box::new(ast::Hoon::Wing(vec![])),
                )),
                Box::new(inner),
            )
        })
    }

    fn synthetic_urbit_scope_imports(&self, path: &Path, is_root: bool) -> Vec<ScopedImport> {
        if self.scope_mode != ScopeMode::Urbit {
            return Vec::new();
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            return Vec::new();
        };
        let required_sys: Vec<(&str, Option<&str>)> = match stem {
            // Urbit-mode prelude:
            // non-sys files get zuse in scope.
            // zuse itself depends on lull. lull expects `..part` from /sys/hoon.
            "zuse" => vec![("lull", Some("lull"))],
            "lull" => vec![("hoon", Some("part"))],
            "hoon" => Vec::new(),
            _ => {
                if is_root {
                    vec![("zuse", None)]
                } else {
                    Vec::new()
                }
            }
        };

        required_sys
            .into_iter()
            .filter(|(name, _)| urbit_sys_file_exists(&self.wer_base, name))
            .map(|(name, face)| ScopedImport {
                kind: ImportKind::Sys,
                face: face.map(std::string::ToString::to_string),
                mark: None,
                suffix: name.to_string(),
            })
            .collect()
    }
}

fn urbit_sys_file_exists(wer_base: &Path, name: &str) -> bool {
    if name == "hoon" {
        return vendored_hoon_sys_path().is_some();
    }
    wer_base.join("sys").join(format!("{name}.hoon")).is_file()
}

fn vendored_hoon_sys_path() -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../hoonc/hoon/hoon-138.hoon")
        .canonicalize()
        .ok()?;
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

fn parse_leading_imports(source: &str) -> Result<Vec<ScopedImport>> {
    let mut imports = Vec::new();
    let lines: Vec<&str> = source.lines().collect();
    let mut index = 0usize;

    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim_start();

        if trimmed.is_empty() || trimmed.starts_with("::") {
            index += 1;
            continue;
        }
        if !trimmed.starts_with('/') {
            break;
        }

        let mut chars = trimmed.chars();
        let _slash = chars.next();
        let Some(rune) = chars.next() else {
            break;
        };
        // A leading `/` whose rune is not an import rune ends the import
        // block (e.g. the body starts). `/?` (Ford kelvin pin) and `/%`
        // (propagating build) are import-block runes honk handles
        // explicitly below.
        if !matches!(rune, '-' | '+' | '=' | '*' | '#' | '?' | '%') {
            break;
        }

        let mut clause = chars.as_str().trim().to_string();
        index += 1;

        while index < lines.len() {
            let continuation = lines[index];
            if continuation.starts_with(' ') || continuation.starts_with('\t') {
                let continuation = continuation.trim();
                if !continuation.is_empty() && !continuation.starts_with("::") {
                    if !clause.is_empty() {
                        clause.push(' ');
                    }
                    clause.push_str(continuation);
                }
                index += 1;
                continue;
            }
            break;
        }

        match rune {
            '=' => imports.push(parse_raw_import_clause(clause.as_str())?),
            '*' => imports.push(parse_bar_import_clause(clause.as_str())?),
            '+' => imports.extend(parse_import_clause(ImportKind::Lib, clause.as_str())?),
            '-' => imports.extend(parse_import_clause(ImportKind::Sur, clause.as_str())?),
            '#' => imports.extend(parse_import_clause(ImportKind::Dat, clause.as_str())?),
            // `/?` pins the Ford kelvin version; it is not an import. Accept
            // and ignore it rather than silently treating it as data.
            '?' => {
                tracing::debug!(clause = %clause, "ignoring /? Ford version pin");
            }
            // `/%` (propagating build) is a real Ford rune honk does not
            // implement; reject rather than silently dropping it.
            '%' => {
                return Err(CompilerError::UnsupportedExpr(format!(
                    "/% imports are not supported by honk: `/%{clause}`"
                )));
            }
            _ => unreachable!("rune gated by matches! above"),
        }
    }

    Ok(imports)
}

fn parse_raw_import_clause(clause: &str) -> Result<ScopedImport> {
    let token = strip_inline_comment(clause).trim();
    let malformed = || {
        CompilerError::Parse(format!(
            "malformed /= import clause: `/={clause}` (expected `face suffix`)"
        ))
    };
    if token.is_empty() {
        return Err(malformed());
    }

    let mut parts = token.split_whitespace();
    let face = parts.next().ok_or_else(malformed)?;
    let suffix = parts.next().ok_or_else(malformed)?;
    if parts.next().is_some() {
        return Err(malformed());
    }
    let face = if face == "*" {
        None
    } else {
        Some(face.to_string())
    };
    Ok(ScopedImport {
        kind: ImportKind::Raw,
        face,
        mark: None,
        suffix: suffix.to_string(),
    })
}

fn parse_bar_import_clause(clause: &str) -> Result<ScopedImport> {
    let token = strip_inline_comment(clause).trim();
    let malformed = || {
        CompilerError::Parse(format!(
            "malformed /* import clause: `/*{clause}` (expected `face mark suffix`)"
        ))
    };
    if token.is_empty() {
        return Err(malformed());
    }

    let mut parts = token.split_whitespace();
    let face = parts.next().ok_or_else(malformed)?;
    let mark = parts.next().ok_or_else(malformed)?;
    let suffix = parts.next().ok_or_else(malformed)?;
    if parts.next().is_some() {
        return Err(malformed());
    }
    let face = if face == "*" {
        None
    } else {
        Some(face.to_string())
    };
    Ok(ScopedImport {
        kind: ImportKind::Bar,
        face,
        mark: Some(mark.trim_start_matches('%').to_string()),
        suffix: suffix.to_string(),
    })
}

fn parse_import_clause(kind: ImportKind, clause: &str) -> Result<Vec<ScopedImport>> {
    let mut imports = Vec::new();
    for item in clause.split(',') {
        let token = strip_inline_comment(item.trim());
        if token.is_empty() {
            continue;
        }

        if let Some(rest) = token.strip_prefix('*') {
            let suffix = rest.trim();
            if suffix.is_empty() {
                return Err(CompilerError::Parse(format!(
                    "malformed import clause item `{token}` (empty `*` suffix)"
                )));
            }
            imports.push(ScopedImport {
                kind,
                face: None,
                mark: None,
                suffix: suffix.to_string(),
            });
        } else if let Some((face, suffix)) = token.split_once('=') {
            let face = face.trim();
            let suffix = suffix.trim();
            if face.is_empty() || suffix.is_empty() {
                return Err(CompilerError::Parse(format!(
                    "malformed import clause item `{token}` (expected `face=suffix`)"
                )));
            }
            imports.push(ScopedImport {
                kind,
                face: Some(face.to_string()),
                mark: None,
                suffix: suffix.to_string(),
            });
        } else {
            imports.push(ScopedImport {
                kind,
                face: Some(token.to_string()),
                mark: None,
                suffix: token.to_string(),
            });
        }
    }
    Ok(imports)
}

fn strip_inline_comment(token: &str) -> &str {
    match token.find("::") {
        Some(idx) => token[..idx].trim(),
        None => token,
    }
}

fn data_import_expr(path: &Path) -> Result<ast::Hoon> {
    let bytes = std::fs::read(path)?;
    let len = ast::ParsedAtom::from(bytes.len() as u64);
    let atom = ast::ParsedAtom::from_biguint(BigUint::from_bytes_le(&bytes));
    let value = ast::NounExpr::Cell(
        Box::new(ast::NounExpr::ParsedAtom(len)),
        Box::new(ast::NounExpr::ParsedAtom(atom)),
    );
    let value = ast::Hoon::Rock("$".to_string(), value);
    let skin = ast::Skin::Cell(
        Box::new(ast::Skin::Name(
            "p".to_string(),
            Box::new(ast::Skin::Base(ast::BaseType::Atom("ud".to_string()))),
        )),
        Box::new(ast::Skin::Name(
            "q".to_string(),
            Box::new(ast::Skin::Base(ast::BaseType::Atom("$".to_string()))),
        )),
    );
    Ok(ast::Hoon::KetTis(skin, Box::new(value)))
}

fn suffix_path_candidates(prefix: &str, suffix: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for segments in hyphen_segment_variants(suffix) {
        if segments.is_empty() {
            continue;
        }
        let mut rel = PathBuf::from(prefix);
        for segment in segments.iter().take(segments.len().saturating_sub(1)) {
            rel.push(segment);
        }
        if let Some(last) = segments.last() {
            rel.push(format!("{last}.hoon"));
            out.push(rel);
        }
    }
    out
}

fn hyphen_segment_variants(suffix: &str) -> Vec<Vec<String>> {
    let parts: Vec<String> = suffix
        .split('-')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect();
    let mut variants = hyphen_segment_variants_inner(parts.as_slice());
    // Clay tries fewer slashes first.
    variants.reverse();
    variants
}

fn hyphen_segment_variants_inner(parts: &[String]) -> Vec<Vec<String>> {
    if parts.is_empty() {
        return Vec::new();
    }
    if parts.len() == 1 {
        return vec![vec![parts[0].clone()]];
    }

    let head = parts[0].clone();
    let tail = hyphen_segment_variants_inner(&parts[1..]);
    let mut out = Vec::new();

    for segments in tail {
        if segments.is_empty() {
            continue;
        }
        let mut split = Vec::with_capacity(segments.len() + 1);
        split.push(head.clone());
        split.extend(segments.clone());
        out.push(split);

        let mut merged = segments;
        merged[0] = format!("{head}-{}", merged[0]);
        out.push(merged);
    }

    out
}

#[cfg(test)]
fn parse_native_hoon_with_wer_and_dbug(
    path: &Path,
    wer: Vec<String>,
    dbug: bool,
) -> Result<ast::Hoon> {
    let source = std::fs::read_to_string(path)?;
    parse_native_hoon_source_with_wer_and_dbug(path, source.as_str(), wer, dbug)
}

fn parse_native_hoon_source_with_wer_and_dbug(
    path: &Path,
    source: &str,
    wer: Vec<String>,
    dbug: bool,
) -> Result<ast::Hoon> {
    parse_native_hoon_source_with_wer_dbug_and_docs(path, source, wer, dbug, true)
}

fn parse_native_hoon_source_with_wer_dbug_and_docs(
    path: &Path,
    source: &str,
    wer: Vec<String>,
    dbug: bool,
    docs_enabled: bool,
) -> Result<ast::Hoon> {
    let linemap = std::sync::Arc::new(LineMap::new_with_docs(source, docs_enabled));
    let linemap_for_error = std::sync::Arc::clone(&linemap);
    let parsed = native_parser(wer, dbug, linemap)
        .parse(source)
        .into_result()
        .map_err(|errs| {
            let message = errs
                .first()
                .map(|err| format!("native parser failed: {}", err.reason()))
                .unwrap_or_else(|| format!("native parser failed: {errs:?}"));
            let mut parse_error = CompilerError::Parse(message);
            if let Some(err) = errs.first() {
                let span = err.span().into_range();
                let pint = linemap_for_error.pint(span.clone());
                parse_error = parse_error.with_metadata(
                    CompilerErrorMetadata::default().with_location(CompilerErrorLocation {
                        file: Some(path.display().to_string()),
                        start_byte: Some(span.start),
                        end_byte: Some(span.end),
                        start_line: Some(pint.p.0),
                        start_col: Some(pint.p.1),
                        end_line: Some(pint.q.0),
                        end_col: Some(pint.q.1),
                    }),
                );
            }
            parse_error
        })?;
    Ok(parsed)
}

fn sanitize_urbit_sys_header(scope_mode: ScopeMode, path: &Path, source: &str) -> String {
    if scope_mode != ScopeMode::Urbit {
        return source.to_string();
    }
    let parent_is_sys = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|seg| seg.to_str())
        == Some("sys");
    if !parent_is_sys {
        return source.to_string();
    }

    let mut out = String::with_capacity(source.len());
    let mut in_header = true;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if in_header {
            if trimmed.starts_with("=>") && trimmed.contains("..") {
                continue;
            }
            if trimmed.starts_with("~%") {
                continue;
            }
            if trimmed.starts_with("|%")
                || trimmed.starts_with("|_")
                || trimmed.starts_with("|=")
                || trimmed.starts_with("|*")
            {
                in_header = false;
            }
        }
        out.push_str(line);
    }

    if out.is_empty() {
        source.to_string()
    } else {
        out
    }
}

fn hoon_path_for_any(path: &Path, deps_dir: &Path) -> Vec<String> {
    if let Ok(rel) = path.strip_prefix(deps_dir) {
        return hoon_path_for_absolute(rel);
    }

    if let Ok(cwd) = std::env::current_dir() {
        let absolute_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        let absolute_deps = if deps_dir.is_absolute() {
            deps_dir.to_path_buf()
        } else {
            cwd.join(deps_dir)
        };
        if let Ok(rel) = absolute_path.strip_prefix(&absolute_deps) {
            return hoon_path_for_absolute(rel);
        }
    }

    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let canonical_deps = deps_dir
        .canonicalize()
        .unwrap_or_else(|_| deps_dir.to_path_buf());

    if let Ok(rel) = canonical_path.strip_prefix(&canonical_deps) {
        return hoon_path_for_absolute(rel);
    }

    // Entry files outside the deps root: prefer a cwd-relative spot so the
    // artifact does not bake in the build machine's absolute paths (same
    // canonicalization dbug_path applies to error display).
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(canonical_cwd) = cwd.canonicalize() {
            if let Ok(rel) = canonical_path.strip_prefix(&canonical_cwd) {
                return hoon_path_for_absolute(rel);
            }
        }
    }

    hoon_path_for_absolute(canonical_path.as_path())
}

fn hoon_path_for_absolute(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(seg) => Some(seg.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn wer_base_dir_for_mode(path: &Path, deps_dir: &Path, scope_mode: ScopeMode) -> PathBuf {
    match scope_mode {
        ScopeMode::Standard => deps_dir.to_path_buf(),
        ScopeMode::Urbit => {
            resolve_urbit_arvo_root(path, deps_dir).unwrap_or_else(|| deps_dir.to_path_buf())
        }
    }
}

#[cfg(test)]
fn parse_roots_for_mode(entry: &Path, deps_dir: &Path, scope_mode: ScopeMode) -> Vec<PathBuf> {
    let mut roots = vec![deps_dir.to_path_buf()];
    if scope_mode == ScopeMode::Urbit {
        if let Some(arvo_root) = resolve_urbit_arvo_root(entry, deps_dir) {
            let sys_root = arvo_root.join("sys");
            if sys_root.is_dir() && !roots.iter().any(|root| root == &sys_root) {
                roots.push(sys_root);
            }
        }
    }
    roots
}

fn resolve_urbit_arvo_root(path: &Path, deps_dir: &Path) -> Option<PathBuf> {
    if let Some(root) = find_urbit_arvo_root(deps_dir) {
        return Some(root);
    }
    find_urbit_arvo_root(path)
}

fn find_urbit_arvo_root(path: &Path) -> Option<PathBuf> {
    for ancestor in path.ancestors() {
        if ancestor.file_name().and_then(|seg| seg.to_str()) != Some("arvo") {
            continue;
        }
        if ancestor
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|seg| seg.to_str())
            != Some("pkg")
        {
            continue;
        }
        if ancestor.join("sys").is_dir() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        hyphen_segment_variants, parse_leading_imports, parse_native_hoon_with_mode,
        parse_native_hoon_with_wer_and_dbug, parse_roots_for_mode, sanitize_urbit_sys_header,
        suffix_path_candidates, vendored_hoon_sys_path, ImportKind, NativeImportResolver,
        ScopeMode, ScopedImport,
    };
    use crate::errors::CompilerError;

    fn temp_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("honk-{name}-{nanos}.hoon"))
    }

    #[test]
    fn native_parse_error_attaches_location_metadata() {
        let path = temp_path("bad-parse");
        fs::write(&path, "this is not hoon").expect("write test source");

        let err = parse_native_hoon_with_wer_and_dbug(
            &path,
            vec!["tmp".to_string(), "bad-parse.hoon".to_string()],
            false,
        )
        .expect_err("expected parse failure");

        let (metadata, message) = match err {
            CompilerError::Detailed {
                message, metadata, ..
            } => (metadata, message),
            other => panic!("expected detailed error, got {other:?}"),
        };

        assert!(
            message.starts_with("native parser failed:"),
            "unexpected message: {message}"
        );
        let location = metadata.location.expect("location metadata should exist");
        assert_eq!(location.file, Some(path.display().to_string()));
        assert!(location.start_byte.is_some(), "start byte should exist");
        assert!(location.end_byte.is_some(), "end byte should exist");
        assert!(location.start_line.is_some(), "start line should exist");
        assert!(location.start_col.is_some(), "start col should exist");
        assert!(location.end_line.is_some(), "end line should exist");
        assert!(location.end_col.is_some(), "end col should exist");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn urbit_scope_includes_sys_parse_root() {
        let tmp = std::env::temp_dir().join("honk-urbit-scope-test");
        let arvo = tmp.join("pkg").join("arvo");
        let sys = arvo.join("sys");
        let app = arvo.join("app");
        let _ = fs::create_dir_all(&sys);
        let _ = fs::create_dir_all(&app);
        let entry = app.join("ping.hoon");
        let _ = fs::write(&entry, "|=([a=@ b=@] a)\n");

        let roots = parse_roots_for_mode(&entry, &arvo, ScopeMode::Urbit);
        assert!(roots.iter().any(|root| root == &arvo));
        assert!(roots.iter().any(|root| root == &sys));

        let _ = fs::remove_file(&entry);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn parses_plus_import_clause_with_alias_and_star() {
        let source = "/+  default-agent, util=verb, *dbug\n|=([a=@ b=@] a)\n";
        let imports = parse_leading_imports(source).expect("imports parse");

        assert_eq!(imports.len(), 3);
        assert_eq!(
            imports[0],
            ScopedImport {
                kind: ImportKind::Lib,
                face: Some("default-agent".to_string()),
                mark: None,
                suffix: "default-agent".to_string(),
            }
        );
        assert_eq!(
            imports[1],
            ScopedImport {
                kind: ImportKind::Lib,
                face: Some("util".to_string()),
                mark: None,
                suffix: "verb".to_string(),
            }
        );
        assert_eq!(
            imports[2],
            ScopedImport {
                kind: ImportKind::Lib,
                face: None,
                mark: None,
                suffix: "dbug".to_string(),
            }
        );
    }

    #[test]
    fn parses_minus_import_clause() {
        let source = "/-  bowl, *lull\n|=([a=@ b=@] a)\n";
        let imports = parse_leading_imports(source).expect("imports parse");

        assert_eq!(imports.len(), 2);
        assert_eq!(
            imports[0],
            ScopedImport {
                kind: ImportKind::Sur,
                face: Some("bowl".to_string()),
                mark: None,
                suffix: "bowl".to_string(),
            }
        );
        assert_eq!(
            imports[1],
            ScopedImport {
                kind: ImportKind::Sur,
                face: None,
                mark: None,
                suffix: "lull".to_string(),
            }
        );
    }

    #[test]
    fn parses_raw_and_dat_import_clauses() {
        let source =
            "/=  t  /common/tx-engine\n/=  *  /common/wrapper\n/#  softed-constraints\n|%\n--\n";
        let imports = parse_leading_imports(source).expect("imports parse");

        assert_eq!(imports.len(), 3);
        assert_eq!(
            imports[0],
            ScopedImport {
                kind: ImportKind::Raw,
                face: Some("t".to_string()),
                mark: None,
                suffix: "/common/tx-engine".to_string(),
            }
        );
        assert_eq!(
            imports[1],
            ScopedImport {
                kind: ImportKind::Raw,
                face: None,
                mark: None,
                suffix: "/common/wrapper".to_string(),
            }
        );
        assert_eq!(
            imports[2],
            ScopedImport {
                kind: ImportKind::Dat,
                face: Some("softed-constraints".to_string()),
                mark: None,
                suffix: "softed-constraints".to_string(),
            }
        );
    }

    #[test]
    fn parses_imports_after_comment_lines_in_import_block() {
        let source = concat!(
            "/=  a  /common/a\n", "\n", ":: /=  skipped  /common/skipped\n", "/=  b  /common/b\n",
            "::  file docs\n", "|%\n", "--\n",
        );
        let imports = parse_leading_imports(source).expect("imports parse");

        assert_eq!(imports.len(), 2);
        assert_eq!(imports[0].suffix, "/common/a");
        assert_eq!(imports[1].suffix, "/common/b");
    }

    #[test]
    fn parses_bar_data_import_with_mark() {
        let source = "/*  blocks  %jam  /jams/small-blocks/jam\n|%\n--\n";
        let imports = parse_leading_imports(source).expect("imports parse");
        assert_eq!(imports.len(), 1);
        assert_eq!(
            imports[0],
            ScopedImport {
                kind: ImportKind::Bar,
                face: Some("blocks".to_string()),
                mark: Some("jam".to_string()),
                suffix: "/jams/small-blocks/jam".to_string(),
            }
        );
    }

    #[test]
    fn ignores_ford_version_pin() {
        // `/?` pins the Ford kelvin version; it is not an import.
        let source = "/?  310\n/=  *  /common/wrapper\n|%\n--\n";
        let imports = parse_leading_imports(source).expect("imports parse");
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].kind, ImportKind::Raw);
        assert_eq!(imports[0].suffix, "/common/wrapper");
    }

    #[test]
    fn rejects_propagating_build_rune() {
        let source = "/%  thing  %hoon  /lib/thing\n|%\n--\n";
        let err = parse_leading_imports(source).expect_err("/% must be rejected");
        assert!(
            matches!(err, CompilerError::UnsupportedExpr(_)),
            "expected UnsupportedExpr, got {err:?}"
        );
    }

    #[test]
    fn rejects_malformed_raw_import() {
        // `/=` requires `face suffix`; a lone token is malformed.
        let source = "/=  onlyface\n|%\n--\n";
        let err = parse_leading_imports(source).expect_err("malformed /= must error");
        assert!(matches!(err, CompilerError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn rejects_malformed_bar_import() {
        // `/*` requires `face mark suffix`.
        let source = "/*  blocks  /jams/x/jam\n|%\n--\n";
        let err = parse_leading_imports(source).expect_err("malformed /* must error");
        assert!(matches!(err, CompilerError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn hyphen_variants_match_clay_order() {
        let variants = hyphen_segment_variants("a-b-c");
        let expected = vec![
            vec!["a-b-c".to_string()],
            vec!["a".to_string(), "b-c".to_string()],
            vec!["a-b".to_string(), "c".to_string()],
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        ];
        assert_eq!(variants, expected);
    }

    #[test]
    fn lib_suffix_candidate_paths_include_hoon_extension() {
        let candidates = suffix_path_candidates("lib", "default-agent");
        let rendered: Vec<String> = candidates
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();
        assert!(rendered
            .iter()
            .any(|path| path.ends_with("lib/default-agent.hoon")));
    }

    #[test]
    fn native_parse_wraps_plus_import_as_binding() {
        let tmp = std::env::temp_dir().join("honk-lib-import-ast-test");
        let lib = tmp.join("lib");
        let _ = fs::create_dir_all(&lib);
        let entry = tmp.join("demo.hoon");
        let dep = lib.join("helper.hoon");
        let _ = fs::write(&dep, "|=([a=@ b=@] (add a b))\n");
        let _ = fs::write(&entry, "/+  helper\n|=([a=@ b=@] (helper a b))\n");

        let expr = parse_native_hoon_with_mode(&entry, &tmp, false, ScopeMode::Standard)
            .expect("native parse should resolve /+ imports");
        match expr {
            super::ast::Hoon::TisLus(left, _) => match *left {
                super::ast::Hoon::KetTis(super::ast::Skin::Term(face), _) => {
                    assert_eq!(face, "helper")
                }
                other => panic!("expected KetTis(helper) binding wrapper, got {other:?}"),
            },
            other => panic!("expected top-level TisLus import wrapper, got {other:?}"),
        }

        let _ = fs::remove_file(&entry);
        let _ = fs::remove_file(&dep);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn native_parse_wraps_raw_star_import_as_subject_extension() {
        let tmp = std::env::temp_dir().join("honk-raw-import-ast-test");
        let common = tmp.join("common");
        let _ = fs::create_dir_all(&common);
        let entry = tmp.join("demo.hoon");
        let dep = common.join("wrapper.hoon");
        let _ = fs::write(&dep, "|%\n++  keep  1\n--\n");
        let _ = fs::write(&entry, "/=  *  /common/wrapper\nkeep\n");

        let expr = parse_native_hoon_with_mode(&entry, &tmp, false, ScopeMode::Standard)
            .expect("native parse should resolve /= imports");
        match expr {
            super::ast::Hoon::TisLus(_, _) => {}
            other => panic!("expected top-level TisLus raw import wrapper, got {other:?}"),
        }

        let _ = fs::remove_file(&entry);
        let _ = fs::remove_file(&dep);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn urbit_synthetic_import_chain_matches_bootstrap_order() {
        let tmp = std::env::temp_dir().join("honk-urbit-chain-test");
        let arvo = tmp.join("pkg").join("arvo");
        let sys = arvo.join("sys");
        let app = arvo.join("app");
        let _ = fs::create_dir_all(&sys);
        let _ = fs::create_dir_all(&app);
        let _ = fs::write(sys.join("lull.hoon"), "|=(a=@ a)\n");
        let _ = fs::write(sys.join("zuse.hoon"), "|=(a=@ a)\n");
        let entry = app.join("ping.hoon");
        let _ = fs::write(&entry, "|=(a=@ a)\n");

        let resolver = NativeImportResolver::new(arvo.clone(), false, ScopeMode::Urbit);

        let root_imports = resolver.synthetic_urbit_scope_imports(&entry, true);
        assert_eq!(root_imports.len(), 1);
        assert_eq!(root_imports[0].kind, ImportKind::Sys);
        assert_eq!(root_imports[0].suffix, "zuse");

        let zuse_imports = resolver.synthetic_urbit_scope_imports(&sys.join("zuse.hoon"), false);
        assert_eq!(zuse_imports.len(), 1);
        assert_eq!(zuse_imports[0].kind, ImportKind::Sys);
        assert_eq!(zuse_imports[0].suffix, "lull");
        assert_eq!(zuse_imports[0].face.as_deref(), Some("lull"));

        let lull_imports = resolver.synthetic_urbit_scope_imports(&sys.join("lull.hoon"), false);
        assert_eq!(lull_imports.len(), 1);
        assert_eq!(lull_imports[0].kind, ImportKind::Sys);
        assert_eq!(lull_imports[0].suffix, "hoon");
        assert_eq!(lull_imports[0].face.as_deref(), Some("part"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn urbit_synthetic_faces_match_bootstrap_expectations() {
        let tmp = std::env::temp_dir().join("honk-urbit-faces-test");
        let arvo = tmp.join("pkg").join("arvo");
        let sys = arvo.join("sys");
        let _ = fs::create_dir_all(&sys);
        let resolver = NativeImportResolver::new(arvo, false, ScopeMode::Urbit);

        let wrapped = resolver
            .synthetic_urbit_scope_faces(&sys.join("lull.hoon"), super::ast::Hoon::Wing(vec![]));
        assert!(
            matches!(wrapped, super::ast::Hoon::Wing(_)),
            "lull should not need direct face wrapper"
        );

        let wrapped = resolver
            .synthetic_urbit_scope_faces(&sys.join("zuse.hoon"), super::ast::Hoon::Wing(vec![]));
        assert!(
            matches!(wrapped, super::ast::Hoon::Wing(_)),
            "zuse should not need direct face wrapper"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn urbit_parse_wraps_zuse_with_lull_face() {
        let tmp = std::env::temp_dir().join("honk-urbit-zuse-wrap-test");
        let arvo = tmp.join("pkg").join("arvo");
        let sys = arvo.join("sys");
        let _ = fs::create_dir_all(&sys);
        let _ = fs::write(sys.join("lull.hoon"), "|=(a=@ a)\n");
        let _ = fs::write(sys.join("zuse.hoon"), "=>  ..lull\n|=(a=@ a)\n");

        let expr =
            parse_native_hoon_with_mode(&sys.join("zuse.hoon"), &arvo, false, ScopeMode::Urbit)
                .expect("urbit zuse parse should succeed");

        match expr {
            super::ast::Hoon::TisLus(left, _) => match *left {
                super::ast::Hoon::KetTis(super::ast::Skin::Term(face), _) => {
                    assert_eq!(face, "lull");
                }
                other => panic!("expected KetTis(lull) binding, got {other:?}"),
            },
            other => panic!("expected top-level TisLus(lull), got {other:?}"),
        }

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn urbit_sys_hoon_resolves_to_vendored_hoon_file() {
        let Some(vendored) = vendored_hoon_sys_path() else {
            panic!("expected vendored hoon-138.hoon to exist");
        };
        let tmp = std::env::temp_dir().join("honk-urbit-hoon-resolve-test");
        let arvo = tmp.join("pkg").join("arvo");
        let app = arvo.join("app");
        let _ = fs::create_dir_all(&app);
        let from = app.join("ping.hoon");
        let _ = fs::write(&from, "|=(a=@ a)\n");

        let resolver = NativeImportResolver::new(arvo.clone(), false, ScopeMode::Urbit);
        let resolved = resolver
            .resolve_import(&from, ImportKind::Sys, "hoon")
            .expect("sys hoon should resolve");
        assert_eq!(
            resolved.canonicalize().ok(),
            vendored.canonicalize().ok(),
            "resolved sys/hoon should point at vendored hoon-138 file"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn urbit_sys_header_sanitizer_drops_part_registration_lines() {
        let tmp = std::env::temp_dir().join("honk-urbit-sanitize-header-test");
        let sys = tmp.join("sys");
        let _ = fs::create_dir_all(&sys);
        let path = sys.join("lull.hoon");
        let source = "!:\n=>  ..part\n~%  %lull  ..part  ~\n|%\n++  x  1\n--\n";
        let sanitized = sanitize_urbit_sys_header(ScopeMode::Urbit, &path, source);
        assert!(!sanitized.contains("..part"));
        assert!(sanitized.starts_with("!:\n|%\n"));
        assert!(sanitized.contains("++  x  1\n"));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn urbit_header_sanitizer_keeps_non_sys_files() {
        let tmp = std::env::temp_dir().join("honk-urbit-sanitize-nonsys-test");
        let app = tmp.join("app");
        let _ = fs::create_dir_all(&app);
        let path = app.join("ping.hoon");
        let source = "=>  ..part\n~%  %demo  ..part  ~\n|=([a=@] a)\n";
        let sanitized = sanitize_urbit_sys_header(ScopeMode::Urbit, &path, source);
        assert_eq!(sanitized, source);
        let _ = fs::remove_dir_all(&tmp);
    }
}
