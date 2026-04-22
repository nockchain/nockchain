//! Post-install patch engine.
//!
//! A package can ship a `[[patches]]` array in its `hoon.toml` asking
//! nockup to reshape specific consumer files after symlinking is done.
//! Four declarative ops — no shell hooks, no code execution:
//!
//! | Op                  | Use case                                            |
//! |---------------------|-----------------------------------------------------|
//! | `overwrite`         | replace a scaffold file wholesale                   |
//! | `replace_pattern`   | swap a scaffold idiom for the package's shape       |
//! | `ensure_dependency` | insert/preserve a `[<table>].<name>` entry          |
//! | `ensure_patch`      | merge into `[patch."<registry>"]` without clobber   |

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result};
use colored::Colorize;
use regex::Regex;
use toml_edit::{value as te_value, DocumentMut, Item, Table};

use crate::manifest::{AppliedPatch, PackagePatch, PatchOp};

#[derive(Debug, Clone, Copy, Default)]
pub struct PatchOptions {
    /// Plan without writing. Caller still gets a `PatchReport` so it can
    /// decide what to say on the command line.
    pub dry_run: bool,
}

#[derive(Debug, Default)]
pub struct PatchReport {
    /// Entries that were (or would be) written to the lockfile ledger.
    pub applied: Vec<AppliedPatch>,
    /// Count of `applied` entries that were no-ops (file already in shape).
    pub skipped_noop: usize,
    pub refused: Vec<PatchRefusal>,
}

impl PatchReport {
    /// True when every patch was a no-op and nothing was refused — i.e.
    /// the project is already in the shape the package wants.
    pub fn nothing_to_do(&self) -> bool {
        self.refused.is_empty() && self.applied.len() == self.skipped_noop
    }
}

#[derive(Debug)]
pub struct PatchRefusal {
    pub file: String,
    pub reason: String,
}

/// Render the summary that both the prompt and `--show-patches` print.
pub fn summarize(project_dir: &Path, package: &str, patches: &[PackagePatch]) -> String {
    let mut out = format!(
        "  {} {} patches from {}:\n",
        "»".cyan(),
        patches.len(),
        package.yellow()
    );
    for (idx, patch) in patches.iter().enumerate() {
        let exists = project_dir.join(&patch.file).exists();
        let op_label = match &patch.op {
            PatchOp::Overwrite { .. } if exists => "overwrite".red(),
            PatchOp::Overwrite { .. } => "create".green(),
            PatchOp::ReplacePattern { .. } => "replace-pattern".yellow(),
            PatchOp::EnsureDependency { .. } => "ensure-dependency".cyan(),
            PatchOp::EnsurePatch { .. } => "ensure-patch".cyan(),
        };
        out.push_str(&format!(
            "    [{}] {:<18} {}\n",
            idx + 1,
            op_label,
            patch.file
        ));
        if let Some(desc) = &patch.description {
            out.push_str(&format!("        {}\n", desc.dimmed()));
        }
    }
    out
}

pub fn prompt_user(package: &str, summary: &str) -> Result<bool> {
    eprintln!(
        "{} Package {} wants to apply {} to your project.",
        "!".yellow(),
        package.yellow(),
        "post-install patches".bold()
    );
    eprintln!("{}", summary);
    eprintln!("  Patches are declarative (no code execution). Apply now? [y/N] ");
    io::stderr().flush().ok();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)?;
    Ok(matches!(buf.trim(), "y" | "Y" | "yes" | "YES"))
}

/// Apply a package's patches against `project_dir`. Caller runs the
/// confirmation prompt and persists the returned ledger entries.
pub fn apply_patches(
    project_dir: &Path,
    patches: &[PackagePatch],
    prior_ledger: &[AppliedPatch],
    opts: PatchOptions,
) -> Result<PatchReport> {
    let mut report = PatchReport::default();
    for (idx, patch) in patches.iter().enumerate() {
        let prior = prior_ledger.iter().find(|e| e.index == idx);
        let file_path = project_dir.join(&patch.file);
        match apply_one(&file_path, idx, patch, prior, opts) {
            Ok(ApplyOutcome::Applied(entry)) => report.applied.push(entry),
            Ok(ApplyOutcome::Noop(entry)) => {
                report.applied.push(entry);
                report.skipped_noop += 1;
            }
            Ok(ApplyOutcome::Refused { reason }) => report.refused.push(PatchRefusal {
                file: patch.file.clone(),
                reason,
            }),
            Err(e) => report.refused.push(PatchRefusal {
                file: patch.file.clone(),
                reason: format!("{e:#}"),
            }),
        }
    }
    Ok(report)
}

// --- internals -------------------------------------------------------------

enum ApplyOutcome {
    Applied(AppliedPatch),
    Noop(AppliedPatch),
    Refused { reason: String },
}

fn apply_one(
    file_path: &Path,
    idx: usize,
    patch: &PackagePatch,
    prior: Option<&AppliedPatch>,
    opts: PatchOptions,
) -> Result<ApplyOutcome> {
    match &patch.op {
        PatchOp::Overwrite { content } => overwrite_op(file_path, idx, patch, content, prior, opts),
        PatchOp::ReplacePattern {
            pattern,
            replacement,
            once,
        } => replace_pattern_op(file_path, idx, patch, pattern, replacement, *once, opts),
        PatchOp::EnsureDependency { table, name, value } => {
            ensure_toml_entry(file_path, idx, patch, &[table.as_str()], name, value, opts)
        }
        PatchOp::EnsurePatch { registry, entries } => {
            // One declared patch, but multiple entries. Fold their outcomes
            // into a single report entry so the ledger has a 1:1 mapping
            // with the `[[patches]]` array.
            let mut any_written = false;
            let mut last: Option<AppliedPatch> = None;
            for (name, value) in entries {
                match ensure_toml_entry(
                    file_path,
                    idx,
                    patch,
                    &["patch", registry.as_str()],
                    name,
                    value,
                    opts,
                )? {
                    ApplyOutcome::Applied(entry) => {
                        any_written = true;
                        last = Some(entry);
                    }
                    ApplyOutcome::Noop(entry) => last = Some(entry),
                    refused @ ApplyOutcome::Refused { .. } => return Ok(refused),
                }
            }
            let entry = last.unwrap_or_else(|| AppliedPatch {
                index: idx,
                file: patch.file.clone(),
                content_hash: blake3_file(file_path).unwrap_or_default(),
            });
            Ok(if any_written {
                ApplyOutcome::Applied(entry)
            } else {
                ApplyOutcome::Noop(entry)
            })
        }
    }
}

fn overwrite_op(
    file_path: &Path,
    idx: usize,
    patch: &PackagePatch,
    content: &str,
    prior: Option<&AppliedPatch>,
    opts: PatchOptions,
) -> Result<ApplyOutcome> {
    if file_path.exists() {
        let current = fs::read_to_string(file_path)?;
        if current == content {
            return Ok(ApplyOutcome::Noop(ledger_entry(idx, patch, &current)));
        }
        // Refuse if the user has edited a previously-patched file.
        if let Some(entry) = prior {
            let cur_hash = blake3_bytes(current.as_bytes());
            if cur_hash != entry.content_hash {
                return Ok(ApplyOutcome::Refused {
                    reason: format!(
                        "file was edited since last install (hash {} != ledger {}); \
                         resolve manually or run `nockup patches eject`",
                        &cur_hash[..12],
                        &entry.content_hash[..12],
                    ),
                });
            }
        }
    }
    if !opts.dry_run {
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        fs::write(file_path, content)
            .with_context(|| format!("Failed to write {}", file_path.display()))?;
    }
    Ok(ApplyOutcome::Applied(ledger_entry(idx, patch, content)))
}

fn replace_pattern_op(
    file_path: &Path,
    idx: usize,
    patch: &PackagePatch,
    pattern: &str,
    replacement: &str,
    once: bool,
    opts: PatchOptions,
) -> Result<ApplyOutcome> {
    if !file_path.exists() {
        return Ok(ApplyOutcome::Refused {
            reason: format!("target {} does not exist", patch.file),
        });
    }
    let before = fs::read_to_string(file_path)?;
    let re = Regex::new(pattern).with_context(|| format!("invalid regex `{pattern}`"))?;
    if !re.is_match(&before) {
        // Pattern gone and replacement present → already applied.
        if before.contains(replacement) {
            return Ok(ApplyOutcome::Noop(ledger_entry(idx, patch, &before)));
        }
        return Ok(ApplyOutcome::Refused {
            reason: format!(
                "pattern `{pattern}` does not match in {} and replacement is absent; \
                 consumer file is in an unknown state",
                patch.file
            ),
        });
    }
    let after = if once {
        re.replacen(&before, 1, replacement).into_owned()
    } else {
        re.replace_all(&before, replacement).into_owned()
    };
    if !opts.dry_run {
        fs::write(file_path, &after)?;
    }
    Ok(ApplyOutcome::Applied(ledger_entry(idx, patch, &after)))
}

fn ensure_toml_entry(
    file_path: &Path,
    idx: usize,
    patch: &PackagePatch,
    table_path: &[&str],
    name: &str,
    value: &toml::Value,
    opts: PatchOptions,
) -> Result<ApplyOutcome> {
    debug_assert!(!table_path.is_empty(), "table_path must be non-empty");

    let mut doc = if file_path.exists() {
        read_toml_doc(file_path)?
    } else {
        DocumentMut::new()
    };

    // Walk / create the table path. Non-leaf segments stay implicit so
    // we don't render an empty `[parent]` above `[parent.child]`.
    let leaf = table_path.len() - 1;
    let mut cursor: &mut Item = doc.as_item_mut();
    for (i, segment) in table_path.iter().enumerate() {
        let tbl = cursor
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("expected `{segment}` to be a table in {}", patch.file))?;
        if !tbl.contains_key(segment) {
            let mut new_table = Table::new();
            new_table.set_implicit(i != leaf);
            tbl.insert(segment, Item::Table(new_table));
        }
        cursor = tbl.get_mut(segment).expect("just inserted");
    }
    let tbl = cursor
        .as_table_mut()
        .expect("walk terminates on a Table we either found or created");

    let new_item = te_value(toml_value_to_edit(value));

    if let Some(existing) = tbl.get(name) {
        if items_equal(existing, &new_item) {
            let on_disk = blake3_file(file_path).unwrap_or_default();
            return Ok(ApplyOutcome::Noop(AppliedPatch {
                index: idx,
                file: patch.file.clone(),
                content_hash: on_disk,
            }));
        }
        return Ok(ApplyOutcome::Refused {
            reason: format!(
                "`[{}].{}` already has a different value; leaving user's edit in place",
                table_path.join("."),
                name
            ),
        });
    }

    tbl.insert(name, new_item);
    let new_text = doc.to_string();
    if !opts.dry_run {
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(file_path, &new_text)?;
    }
    Ok(ApplyOutcome::Applied(AppliedPatch {
        index: idx,
        file: patch.file.clone(),
        content_hash: blake3_bytes(new_text.as_bytes()),
    }))
}

fn ledger_entry(idx: usize, patch: &PackagePatch, content: &str) -> AppliedPatch {
    AppliedPatch {
        index: idx,
        file: patch.file.clone(),
        content_hash: blake3_bytes(content.as_bytes()),
    }
}

fn read_toml_doc(path: &Path) -> Result<DocumentMut> {
    fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?
        .parse::<DocumentMut>()
        .with_context(|| format!("Failed to parse {} as TOML", path.display()))
}

fn items_equal(a: &Item, b: &Item) -> bool {
    a.to_string().trim() == b.to_string().trim()
}

/// Convert a `toml::Value` into a `toml_edit::Value` by recursing over
/// variants. A string round-trip doesn't work: `toml::to_string` on a
/// `Value::Table` emits bare keys (`k = v\n`), not inline-table syntax
/// (`{ k = v }`), so naive wrapping produces invalid TOML.
fn toml_value_to_edit(v: &toml::Value) -> toml_edit::Value {
    use toml_edit::{Array, InlineTable, Value as TeValue};
    match v {
        toml::Value::String(s) => TeValue::from(s.as_str()),
        toml::Value::Integer(i) => TeValue::from(*i),
        toml::Value::Float(f) => TeValue::from(*f),
        toml::Value::Boolean(b) => TeValue::from(*b),
        toml::Value::Datetime(dt) => TeValue::from(dt.to_string()),
        toml::Value::Array(arr) => {
            let mut out = Array::new();
            for item in arr {
                out.push(toml_value_to_edit(item));
            }
            TeValue::Array(out)
        }
        toml::Value::Table(tbl) => {
            let mut out = InlineTable::new();
            for (k, v) in tbl {
                out.insert(k, toml_value_to_edit(v));
            }
            TeValue::InlineTable(out)
        }
    }
}

fn blake3_file(path: &Path) -> Result<String> {
    Ok(blake3_bytes(&fs::read(path)?))
}

fn blake3_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

#[cfg(test)]
mod tests;
