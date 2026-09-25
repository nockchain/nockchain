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

/// Parses a whole dependency-tree file, as hoonc's `+parse-dir` does for
/// every Hoon file before building anything, imported or not.
pub fn parse_dependency_tree_file(path: &Path) -> Result<()> {
    let source = std::fs::read_to_string(path)?;
    parse_native_hoon_source_without_docs(path, source.as_str(), Vec::new(), true)?;
    Ok(())
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
            let dep_expr = match import_kind_for(import.kind, &dep_path) {
                NativeImportKind::Data => data_import_expr(dep_path.as_path())?,
                NativeImportKind::Hoon => self.parse(dep_path.as_path())?,
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
                let path = self.resolve_import(path, import.kind, import.suffix.as_str())?;
                Ok(ResolvedNativeImport {
                    kind: import_kind_for(import.kind, &path),
                    face: import.face,
                    path,
                })
            })
            .collect()
    }

    /// Resolves an import the way hoonc's `+resolve-pile` looks it up in its
    /// directory map: `/=` and `/*` name one file, `/-` `/+` `/#` try the
    /// `+get-fit` hyphen variants, and only files hoonc's directory walk keeps
    /// can be found.
    fn resolve_import(&self, from: &Path, kind: ImportKind, suffix: &str) -> Result<PathBuf> {
        let found = match kind {
            ImportKind::Raw => path_knots(suffix).and_then(|mut knots| {
                let last = knots.pop()?;
                knots.push(format!("{last}.hoon"));
                self.hoonc_tree_file(&knots.iter().collect::<PathBuf>())
            }),
            ImportKind::Bar => path_knots(suffix).and_then(|mut knots| {
                if knots.len() < 2 {
                    return None;
                }
                let extension = knots.pop()?;
                let stem = knots.pop()?;
                knots.push(format!("{stem}.{extension}"));
                self.hoonc_tree_file(&knots.iter().collect::<PathBuf>())
            }),
            _ => {
                let prefix = match kind {
                    ImportKind::Lib => "lib",
                    ImportKind::Sur => "sur",
                    ImportKind::Sys => "sys",
                    ImportKind::Dat => "dat",
                    ImportKind::Raw => unreachable!("raw import handled above"),
                    ImportKind::Bar => unreachable!("bar import handled above"),
                };
                suffix_path_candidates(prefix, suffix)
                    .into_iter()
                    .find_map(|candidate| self.hoonc_tree_file(&candidate))
            }
        };
        if let Some(path) = found {
            return Ok(path);
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

    /// The file at `rel` under the dependency root, if hoonc's directory walk
    /// would load it.
    fn hoonc_tree_file(&self, rel: &Path) -> Option<PathBuf> {
        // The walk's keys hold only plain names (no `.` or `..` knots).
        if !rel
            .components()
            .all(|knot| matches!(knot, Component::Normal(_)))
        {
            return None;
        }
        let file = rel.file_name()?.to_string_lossy();
        let skipped_dir = rel.parent().is_some_and(|dirs| {
            dirs.components()
                .any(|dir| HOONC_SKIPPED_DIRS.contains(&dir.as_os_str().to_string_lossy().as_ref()))
        });
        if skipped_dir || !hoonc_loads_file_name(&file) {
            return None;
        }
        let path = self.wer_base.join(rel);
        path.is_file().then_some(path)
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
            // Urbit-mode prelude: non-sys files get zuse in scope, zuse
            // depends on lull, and lull expects `..part` from /sys/hoon.
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

/// Directories hoonc's dependency walk skips (`BLACKLISTED_DIRS` in
/// crates/hoonc/src/lib.rs).
const HOONC_SKIPPED_DIRS: &[&str] = &["packages", "node_modules", ".git", "target"];

/// File suffixes hoonc's dependency walk loads (`is_valid_file_or_dir` in
/// crates/hoonc/src/lib.rs).
const HOONC_LOADED_SUFFIXES: &[&str] =
    &[".jock", ".hoon", ".txt", ".jam", ".html", ".css", ".js", ".jpg", ".png", ".gif"];

fn hoonc_loads_file_name(name: &str) -> bool {
    HOONC_LOADED_SUFFIXES
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

/// hoonc compiles a dependency as Hoon when its file name contains `.hoon`
/// (`+is-hoon` in hoonc.hoon), whichever rune imported it; every other file
/// is an `$octs` data leaf.
fn import_kind_for(kind: ImportKind, path: &Path) -> NativeImportKind {
    let is_hoon = path
        .file_name()
        .map(|name| name.to_string_lossy().contains(".hoon"))
        .unwrap_or(false);
    if kind == ImportKind::Bar && !is_hoon {
        NativeImportKind::Data
    } else {
        NativeImportKind::Hoon
    }
}

/// The knots of a `stap` path rendered as text (`/a/b`); `None` for the
/// root path or a path with an empty knot, which name no file.
fn path_knots(path: &str) -> Option<Vec<String>> {
    let rest = path.strip_prefix('/')?;
    if rest.is_empty() {
        return None;
    }
    let knots: Vec<String> = rest.split('/').map(ToString::to_string).collect();
    (!knots.iter().any(String::is_empty)).then_some(knots)
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

/// A file's import header as hoonc reads it (`+pile-rule` in hoonc.hoon):
/// optional whitespace, an optional `/?  <kelvin>` pin, then the `/-`, `/+`,
/// `/=`, `/*` and `/#` clauses in that order, each clause followed by a gap.
/// `body_start` is where the Hoon body begins: the byte after the header's
/// last gap, or 0 when there is no header.
struct ImportHeader {
    imports: Vec<ScopedImport>,
    body_start: usize,
}

fn parse_leading_imports(source: &str) -> Result<Vec<ScopedImport>> {
    Ok(parse_import_header(source)?.imports)
}

fn parse_import_header(source: &str) -> Result<ImportHeader> {
    let header = HeaderParser {
        src: source.as_bytes(),
    };
    let mut pos = header.gay(0);
    let mut has_header = false;
    if let Some(next) = header.kelvin_pin(pos) {
        pos = next;
        has_header = true;
    }
    let mut imports = Vec::new();
    for rune in [b'-', b'+', b'=', b'*', b'#'] {
        if let Some((group, next)) = header.rune_group(pos, rune) {
            imports.extend(group);
            pos = next;
            has_header = true;
        }
    }
    header.reject_leftover_import(pos)?;
    Ok(ImportHeader {
        imports,
        body_start: if has_header { pos } else { 0 },
    })
}

/// A hand port of the hoon-138 parsers hoonc's header rule is built from.
/// Each method takes a byte offset and returns the offset after its match
/// (`None` on failure); alternatives are ordered and do not backtrack once
/// one matches, as in Hoon's `;~(pose ...)`.
struct HeaderParser<'a> {
    src: &'a [u8],
}

impl HeaderParser<'_> {
    fn byte(&self, at: usize) -> Option<u8> {
        self.src.get(at).copied()
    }

    fn just(&self, at: usize, byte: u8) -> Option<usize> {
        (self.byte(at) == Some(byte)).then_some(at + 1)
    }

    /// `gah`: a newline or a space. Tabs are not Hoon whitespace.
    fn gah(&self, at: usize) -> Option<usize> {
        matches!(self.byte(at), Some(b'\n' | b' ')).then_some(at + 1)
    }

    /// `vul`: a `::` comment of printable bytes ended by a newline.
    fn vul(&self, at: usize) -> Option<usize> {
        let mut at = self.just(self.just(at, b':')?, b':')?;
        while matches!(self.byte(at), Some(byte) if byte >= 32 && byte != 127) {
            at += 1;
        }
        self.just(at, b'\n')
    }

    /// `(star ;~(pose vul gah))`, which is also `gaw`.
    fn white(&self, mut at: usize) -> usize {
        while let Some(next) = self.vul(at).or_else(|| self.gah(at)) {
            at = next;
        }
        at
    }

    /// `gaq`: a newline, two whitespace characters, or a comment.
    fn gaq(&self, at: usize) -> Option<usize> {
        if let Some(next) = self.just(at, b'\n') {
            return Some(next);
        }
        if let Some(next) = self.gah(at) {
            if let Some(next) = self.gah(next).or_else(|| self.vul(next)) {
                return Some(next);
            }
        }
        self.vul(at)
    }

    /// `gap`: `gaq` then any whitespace and comments.
    fn gap(&self, at: usize) -> Option<usize> {
        self.gaq(at).map(|next| self.white(next))
    }

    /// `gay`: an optional gap.
    fn gay(&self, at: usize) -> usize {
        self.gap(at).unwrap_or(at)
    }

    /// `sym`: a lowercase letter, then lowercase letters, digits and `-`.
    fn sym(&self, at: usize) -> Option<(String, usize)> {
        if !matches!(self.byte(at), Some(b'a'..=b'z')) {
            return None;
        }
        let mut end = at + 1;
        while matches!(self.byte(end), Some(b'a'..=b'z' | b'0'..=b'9' | b'-')) {
            end += 1;
        }
        Some((self.text(at, end), end))
    }

    /// `stap`: `/` then `/`-separated knots of `[0-9a-z-.~_]`; the last knot
    /// may be empty only in the root path `/`. Rendered back as text.
    fn stap(&self, at: usize) -> Option<(String, usize)> {
        let mut end = self.just(at, b'/')?;
        let mut last_start = end;
        loop {
            while matches!(
                self.byte(end),
                Some(b'0'..=b'9' | b'a'..=b'z' | b'-' | b'.' | b'~' | b'_')
            ) {
                end += 1;
            }
            match self.just(end, b'/') {
                Some(next) => {
                    end = next;
                    last_start = next;
                }
                None => break,
            }
        }
        let root_only = last_start == at + 1;
        if last_start == end && !root_only {
            return None;
        }
        Some((self.text(at, end), end))
    }

    /// `/?  <decimal>` then a gap. `dem` allows `\` gay `/` between digits.
    fn kelvin_pin(&self, at: usize) -> Option<usize> {
        let at = self.just(self.just(at, b'/')?, b'?')?;
        let mut at = self.gap(at)?;
        at = self.digit(at)?;
        loop {
            let resume = self
                .just(at, b'\\')
                .map(|next| self.gay(next))
                .and_then(|next| self.just(next, b'/'))
                .unwrap_or(at);
            match self.digit(resume) {
                Some(next) => at = next,
                None => break,
            }
        }
        self.gap(at)
    }

    fn digit(&self, at: usize) -> Option<usize> {
        matches!(self.byte(at), Some(b'0'..=b'9')).then_some(at + 1)
    }

    /// `rune`: `pant (mast gap ;~(pfix fas bus gap fel))`, one or more
    /// gap-separated clauses followed by a gap. A group that is not followed
    /// by a gap is dropped whole, as `pant` backtracks.
    fn rune_group(&self, at: usize, rune: u8) -> Option<(Vec<ScopedImport>, usize)> {
        let (mut imports, mut end) = self.rune_clause(at, rune)?;
        while let Some((more, next)) = self.gap(end).and_then(|next| self.rune_clause(next, rune)) {
            imports.extend(more);
            end = next;
        }
        self.gap(end).map(|next| (imports, next))
    }

    fn rune_clause(&self, at: usize, rune: u8) -> Option<(Vec<ScopedImport>, usize)> {
        let at = self.just(self.just(at, b'/')?, rune)?;
        let at = self.gap(at)?;
        match rune {
            b'-' => self.taut_list(at, ImportKind::Sur),
            b'+' => self.taut_list(at, ImportKind::Lib),
            b'#' => self.taut_list(at, ImportKind::Dat),
            b'=' => {
                let (face, at) = match self.just(at, b'*') {
                    Some(next) => (None, next),
                    None => {
                        let (face, next) = self.sym(at)?;
                        (Some(face), next)
                    }
                };
                let (suffix, at) = self.stap(self.gap(at)?)?;
                let import = ScopedImport {
                    kind: ImportKind::Raw,
                    face,
                    mark: None,
                    suffix,
                };
                Some((vec![import], at))
            }
            b'*' => {
                let (face, at) = self.sym(at)?;
                let (mark, at) = self.sym(self.just(self.gap(at)?, b'%')?)?;
                let (suffix, at) = self.stap(self.gap(at)?)?;
                let import = ScopedImport {
                    kind: ImportKind::Bar,
                    face: Some(face),
                    mark: Some(mark),
                    suffix,
                };
                Some((vec![import], at))
            }
            _ => None,
        }
    }

    /// `(most ;~(plug com gaw) taut-rule)`: comma-separated library names,
    /// with any whitespace (blank lines and comments included) after a comma
    /// but none before it.
    fn taut_list(&self, at: usize, kind: ImportKind) -> Option<(Vec<ScopedImport>, usize)> {
        let (first, mut end) = self.taut(at, kind)?;
        let mut imports = vec![first];
        while let Some((next_import, next)) = self
            .just(end, b',')
            .map(|next| self.white(next))
            .and_then(|next| self.taut(next, kind))
        {
            imports.push(next_import);
            end = next;
        }
        Some((imports, end))
    }

    /// `taut-rule`: `*name` (no face), `face=name`, or `name` (face `name`).
    fn taut(&self, at: usize, kind: ImportKind) -> Option<(ScopedImport, usize)> {
        let import = |face: Option<String>, suffix: String| ScopedImport {
            kind,
            face,
            mark: None,
            suffix,
        };
        if let Some(next) = self.just(at, b'*') {
            let (suffix, end) = self.sym(next)?;
            return Some((import(None, suffix), end));
        }
        let (face, end) = self.sym(at)?;
        if let Some((suffix, after)) = self.just(end, b'=').and_then(|next| self.sym(next)) {
            return Some((import(Some(face), suffix), after));
        }
        Some((import(Some(face.clone()), face), end))
    }

    /// hoonc parses the body right after the header, and no Hoon starts with
    /// an import rune (`/=`, `/-` and `/%` only begin paths when a knot
    /// follows), so a clause the header rule could not take fails the file.
    fn reject_leftover_import(&self, at: usize) -> Result<()> {
        if self.byte(at) != Some(b'/') {
            return Ok(());
        }
        let Some(rune) = self.byte(at + 1) else {
            return Ok(());
        };
        let clause_like = matches!(rune, b'+' | b'*' | b'#' | b'?')
            || (matches!(rune, b'-' | b'=' | b'%')
                && matches!(self.byte(at + 2), None | Some(b' ' | b'\n' | b'\t' | b'\r')));
        if !clause_like {
            return Ok(());
        }
        let line_end = self.src[at..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(self.src.len(), |len| at + len);
        let line = self.src[..at].iter().filter(|byte| **byte == b'\n').count() + 1;
        let clause = self.text(at, line_end);
        if rune == b'%' {
            return Err(CompilerError::UnsupportedExpr(format!(
                "/% imports are not supported by honk: `{clause}`"
            )));
        }
        Err(CompilerError::Parse(format!(
            "malformed /{} import clause at line {line}: `{clause}` (hoonc's import \
             header needs `/-` `/+` `/=` `/*` `/#` in that order, each clause after a \
             two-space gap and followed by a gap)",
            rune as char
        )))
    }

    fn text(&self, start: usize, end: usize) -> String {
        String::from_utf8_lossy(&self.src[start..end]).into_owned()
    }
}

/// Replaces the import header with spaces (keeping newlines) so the Hoon
/// parser sees only the body at its original line and column, as hoonc's
/// header rule hands the body to `tall:vang`.
fn blank_import_header(source: &str, body_start: usize) -> std::borrow::Cow<'_, str> {
    if body_start == 0 {
        return std::borrow::Cow::Borrowed(source);
    }
    let mut blanked: Vec<u8> = source.as_bytes()[..body_start]
        .iter()
        .map(|byte| if *byte == b'\n' { b'\n' } else { b' ' })
        .collect();
    blanked.extend_from_slice(&source.as_bytes()[body_start..]);
    // The header rule only consumes ASCII, and the body is untouched.
    std::borrow::Cow::Owned(String::from_utf8(blanked).expect("blanked header stays UTF-8"))
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
    let parts: Vec<String> = suffix.split('-').map(ToString::to_string).collect();
    // hoonc's `+segments` splits only a name whose every hyphen-separated
    // part is non-empty; otherwise (`dbl--dash`, `trail-`) the literal name
    // is the one candidate.
    if parts.iter().any(String::is_empty) {
        return vec![vec![suffix.to_string()]];
    }
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
    // hoonc parses the import header separately and the body from where the
    // header ends, so the body's first `%spot` starts there.
    let header = parse_import_header(source)?;
    let source = blank_import_header(source, header.body_start);
    let source = source.as_ref();
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

    // An entry file outside the deps root gets a cwd-relative spot, so the
    // artifact does not embed the build machine's absolute paths. dbug_path
    // shortens error-display paths the same way.
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
        // hoonc's `+segments` keeps a name with an empty part whole.
        for name in ["dbl--dash", "trail-"] {
            assert_eq!(hyphen_segment_variants(name), vec![vec![name.to_string()]]);
        }
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
