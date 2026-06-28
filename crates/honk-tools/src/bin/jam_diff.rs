use std::collections::HashSet;
use std::{env, fs, process};

use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::utils::NOCK_STACK_SIZE_MEDIUM;
use nockvm::ext::NounExt;
use nockvm::mem::NockStack;
use nockvm::noun::{Noun, NounSpace};

fn usage(program: &str) -> String {
    format!(
        "Usage: {program} <left.jam> <right.jam> [max_nodes] [left_axis] [right_axis]\n\
         Usage: {program} --extract-axis <input.jam> <axis> <output.jam>"
    )
}

fn first_byte_diff(left: &[u8], right: &[u8]) -> Option<usize> {
    let max = left.len().min(right.len());
    for idx in 0..max {
        if left[idx] != right[idx] {
            return Some(idx);
        }
    }
    if left.len() != right.len() {
        Some(max)
    } else {
        None
    }
}

fn cue(stack: &mut NockStack, jam: &[u8], label: &str) -> Noun {
    <Noun as NounExt>::cue_bytes_slice(stack, jam)
        .unwrap_or_else(|err| panic!("failed to cue {label}: {err:?}"))
}

fn rejam(jam: &[u8], label: &str) -> Vec<u8> {
    let mut stack = NockStack::new(NOCK_STACK_SIZE_MEDIUM, 0);
    let noun = cue(&mut stack, jam, label);
    let space = stack.noun_space();
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let root = slab.copy_into(noun, &space);
    slab.set_root(root);
    slab.jam().to_vec()
}

fn jam_axis(input: &[u8], axis: &str) -> Option<Vec<u8>> {
    let mut stack = NockStack::new(NOCK_STACK_SIZE_MEDIUM, 0);
    let noun = cue(&mut stack, input, "input");
    let space = stack.noun_space();
    let selected = axis_at(noun, axis, &space)?;
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let root = slab.copy_into(selected, &space);
    slab.set_root(root);
    Some(slab.jam().to_vec())
}

fn noun_head(noun: Noun, space: &NounSpace) -> Option<Noun> {
    Some(noun.in_space(space).as_cell().ok()?.head().noun())
}

fn noun_tail(noun: Noun, space: &NounSpace) -> Option<Noun> {
    Some(noun.in_space(space).as_cell().ok()?.tail().noun())
}

fn atom_u64(noun: Noun, space: &NounSpace) -> Option<u64> {
    noun.in_space(space).as_atom().ok()?.as_u64().ok()
}

fn atom_bytes(noun: Noun, space: &NounSpace) -> Option<Vec<u8>> {
    Some(noun.in_space(space).as_atom().ok()?.as_ne_bytes().to_vec())
}

fn atom_text(bytes: &[u8]) -> Option<String> {
    let bytes = bytes
        .iter()
        .copied()
        .rev()
        .skip_while(|byte| *byte == 0)
        .collect::<Vec<_>>();
    let bytes = bytes.into_iter().rev().collect::<Vec<_>>();
    if bytes.is_empty()
        || !bytes
            .iter()
            .copied()
            .all(|byte| byte.is_ascii_graphic() || byte == b' ')
    {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).to_string())
}

fn preview(noun: Noun, space: &NounSpace) -> String {
    preview_with_depth(noun, 4, space)
}

fn preview_with_depth(noun: Noun, depth: usize, space: &NounSpace) -> String {
    if let Some(bytes) = atom_bytes(noun, space) {
        if let Some(text) = atom_text(&bytes) {
            return format!("%{text}");
        }
        if let Some(value) = atom_u64(noun, space) {
            return value.to_string();
        }
        let mut hex = String::new();
        for byte in bytes.iter().take(16) {
            use std::fmt::Write as _;
            let _ = write!(&mut hex, "{byte:02x}");
        }
        return format!("0x{hex}..({} bytes)", bytes.len());
    }
    if depth == 0 {
        return "[..]".to_string();
    }
    let Some(head) = noun_head(noun, space) else {
        return "<invalid>".to_string();
    };
    let Some(tail) = noun_tail(noun, space) else {
        return "<invalid>".to_string();
    };
    format!(
        "[{} {}]",
        preview_with_depth(head, depth.saturating_sub(1), space),
        preview_with_depth(tail, depth.saturating_sub(1), space)
    )
}

fn atom_string(noun: Noun, space: &NounSpace) -> Option<String> {
    atom_text(&atom_bytes(noun, space)?)
}

fn help_summary(help: Noun, space: &NounSpace) -> Option<String> {
    let crib = noun_tail(help, space)?;
    atom_string(noun_head(crib, space)?, space)
}

fn collect_help_summaries(noun: Noun, max_nodes: usize, space: &NounSpace) -> (usize, Vec<String>) {
    let mut todo = vec![noun];
    let mut seen = HashSet::new();
    let mut compared = 0usize;
    let mut summaries = Vec::new();
    while let Some(noun) = todo.pop() {
        compared += 1;
        if compared > max_nodes {
            break;
        }
        let raw = unsafe { noun.as_raw() };
        if !seen.insert(raw) {
            continue;
        }
        let Some(head) = noun_head(noun, space) else {
            continue;
        };
        let Some(tail) = noun_tail(noun, space) else {
            continue;
        };
        if atom_string(head, space).as_deref() == Some("help") {
            if let Some(summary) = help_summary(tail, space) {
                summaries.push(summary);
            } else {
                summaries.push(format!("<unparsed:{}>", preview_with_depth(tail, 5, space)));
            }
        }
        todo.push(tail);
        todo.push(head);
    }
    summaries.sort();
    (compared, summaries)
}

fn collect_help_summary_axes(
    noun: Noun,
    needle: &str,
    max_nodes: usize,
    space: &NounSpace,
) -> Vec<(String, String)> {
    let mut todo = vec![(noun, "1".to_string())];
    let mut seen = HashSet::new();
    let mut compared = 0usize;
    let mut out = Vec::new();
    while let Some((noun, axis)) = todo.pop() {
        compared += 1;
        if compared > max_nodes {
            break;
        }
        let raw = unsafe { noun.as_raw() };
        if !seen.insert(raw) {
            continue;
        }
        let Some(head) = noun_head(noun, space) else {
            continue;
        };
        let Some(tail) = noun_tail(noun, space) else {
            continue;
        };
        if atom_string(head, space).as_deref() == Some("help") {
            let summary = help_summary(tail, space)
                .unwrap_or_else(|| format!("<unparsed:{}>", preview_with_depth(tail, 5, space)));
            if summary.contains(needle) {
                out.push((axis.clone(), preview_with_depth(noun, 10, space)));
            }
        }
        todo.push((tail, format!("{axis}.3")));
        todo.push((head, format!("{axis}.2")));
    }
    out
}

type MultisetDelta = (Vec<(String, isize)>, Vec<(String, isize)>);

fn multiset_delta(left: &[String], right: &[String]) -> MultisetDelta {
    let mut left_idx = 0usize;
    let mut right_idx = 0usize;
    let mut only_left = Vec::new();
    let mut only_right = Vec::new();
    while left_idx < left.len() || right_idx < right.len() {
        match (left.get(left_idx), right.get(right_idx)) {
            (Some(left_value), Some(right_value)) if left_value == right_value => {
                let value = left_value.clone();
                let left_start = left_idx;
                let right_start = right_idx;
                while left.get(left_idx) == Some(&value) {
                    left_idx += 1;
                }
                while right.get(right_idx) == Some(&value) {
                    right_idx += 1;
                }
                let delta = (right_idx - right_start) as isize - (left_idx - left_start) as isize;
                if delta < 0 {
                    only_left.push((value, -delta));
                } else if delta > 0 {
                    only_right.push((value, delta));
                }
            }
            (Some(left_value), Some(right_value)) if left_value < right_value => {
                let value = left_value.clone();
                let start = left_idx;
                while left.get(left_idx) == Some(&value) {
                    left_idx += 1;
                }
                only_left.push((value, (left_idx - start) as isize));
            }
            (Some(_), Some(right_value)) => {
                let value = right_value.clone();
                let start = right_idx;
                while right.get(right_idx) == Some(&value) {
                    right_idx += 1;
                }
                only_right.push((value, (right_idx - start) as isize));
            }
            (Some(left_value), None) => {
                let value = left_value.clone();
                let start = left_idx;
                while left.get(left_idx) == Some(&value) {
                    left_idx += 1;
                }
                only_left.push((value, (left_idx - start) as isize));
            }
            (None, Some(right_value)) => {
                let value = right_value.clone();
                let start = right_idx;
                while right.get(right_idx) == Some(&value) {
                    right_idx += 1;
                }
                only_right.push((value, (right_idx - start) as isize));
            }
            (None, None) => break,
        }
    }
    (only_left, only_right)
}

fn nock_spot_hint_preview(noun: Noun, space: &NounSpace) -> Option<String> {
    let head = atom_u64(noun_head(noun, space)?, space)?;
    if head != 11 {
        return None;
    }
    let rest = noun_tail(noun, space)?;
    let hint = noun_head(rest, space)?;
    let tag = atom_text(&atom_bytes(noun_head(hint, space)?, space)?)?;
    if tag != "spot" {
        return None;
    }
    Some(preview_with_depth(noun_tail(hint, space)?, 8, space))
}

fn nearest_spot_preview(root: Noun, axis: &str, space: &NounSpace) -> Option<String> {
    let parts = axis.split('.').collect::<Vec<_>>();
    for len in (1..=parts.len()).rev() {
        let prefix = parts[..len].join(".");
        let Some(noun) = axis_at(root, &prefix, space) else {
            continue;
        };
        if let Some(spot) = nock_spot_hint_preview(noun, space) {
            return Some(format!("{prefix} {spot}"));
        }
    }
    None
}

fn ancestor_context(root: Noun, axis: &str, count: usize, space: &NounSpace) -> String {
    let parts = axis.split('.').collect::<Vec<_>>();
    let mut out = Vec::new();
    for len in (1..parts.len()).rev().take(count) {
        let prefix = parts[..len].join(".");
        if let Some(noun) = axis_at(root, &prefix, space) {
            out.push(format!("{prefix}:{}", preview_with_depth(noun, 6, space)));
        }
    }
    if out.is_empty() {
        "<none>".to_string()
    } else {
        out.join(" | ")
    }
}

fn structural_diff(
    left: Noun,
    right: Noun,
    max_nodes: usize,
    left_space: &NounSpace,
    right_space: &NounSpace,
) -> (usize, bool, Option<String>) {
    let left_root = left;
    let right_root = right;
    let mut todo = vec![(left, right, "1".to_string(), 0usize, None::<String>)];
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut compared = 0usize;
    while let Some((left, right, axis, depth, parent)) = todo.pop() {
        compared += 1;
        if compared > max_nodes {
            return (compared, true, None);
        }
        let raw_pair = unsafe { (left.as_raw(), right.as_raw()) };
        if !seen.insert(raw_pair) {
            continue;
        }
        match (atom_bytes(left, left_space), atom_bytes(right, right_space)) {
            (Some(left_bytes), Some(right_bytes)) => {
                if left_bytes != right_bytes {
                    let left_spot = nearest_spot_preview(left_root, &axis, left_space)
                        .unwrap_or_else(|| "<none>".to_string());
                    let right_spot = nearest_spot_preview(right_root, &axis, right_space)
                        .unwrap_or_else(|| "<none>".to_string());
                    let left_context = ancestor_context(left_root, &axis, 5, left_space);
                    let right_context = ancestor_context(right_root, &axis, 5, right_space);
                    return (
                        compared,
                        false,
                        Some(format!(
                            "axis={axis} depth={depth} atom left={} right={} left_spot={} right_spot={} parent={} left_context={} right_context={}",
                            preview(left, left_space),
                            preview(right, right_space),
                            left_spot,
                            right_spot,
                            parent.as_deref().unwrap_or("<none>"),
                            left_context,
                            right_context
                        )),
                    );
                }
            }
            (None, None) => {
                let Some(left_head) = noun_head(left, left_space) else {
                    return (
                        compared,
                        false,
                        Some(format!(
                            "axis={axis} depth={depth} invalid left cell parent={}",
                            parent.as_deref().unwrap_or("<none>")
                        )),
                    );
                };
                let Some(left_tail) = noun_tail(left, left_space) else {
                    return (
                        compared,
                        false,
                        Some(format!(
                            "axis={axis} depth={depth} invalid left cell parent={}",
                            parent.as_deref().unwrap_or("<none>")
                        )),
                    );
                };
                let Some(right_head) = noun_head(right, right_space) else {
                    return (
                        compared,
                        false,
                        Some(format!(
                            "axis={axis} depth={depth} invalid right cell parent={}",
                            parent.as_deref().unwrap_or("<none>")
                        )),
                    );
                };
                let Some(right_tail) = noun_tail(right, right_space) else {
                    return (
                        compared,
                        false,
                        Some(format!(
                            "axis={axis} depth={depth} invalid right cell parent={}",
                            parent.as_deref().unwrap_or("<none>")
                        )),
                    );
                };
                let context = Some(format!(
                    "left_parent={} right_parent={}",
                    preview_with_depth(left, 5, left_space),
                    preview_with_depth(right, 5, right_space)
                ));
                todo.push((
                    left_tail,
                    right_tail,
                    format!("{axis}.3"),
                    depth + 1,
                    context.clone(),
                ));
                todo.push((
                    left_head,
                    right_head,
                    format!("{axis}.2"),
                    depth + 1,
                    context,
                ));
            }
            _ => {
                let left_spot = nearest_spot_preview(left_root, &axis, left_space)
                    .unwrap_or_else(|| "<none>".to_string());
                let right_spot = nearest_spot_preview(right_root, &axis, right_space)
                    .unwrap_or_else(|| "<none>".to_string());
                return (
                    compared,
                    false,
                    Some(format!(
                        "axis={axis} depth={depth} shape left={} right={} left_spot={} right_spot={} parent={}",
                        preview(left, left_space),
                        preview(right, right_space),
                        left_spot,
                        right_spot,
                        parent.as_deref().unwrap_or("<none>")
                    )),
                );
            }
        }
    }
    (compared, false, None)
}

fn axis_at(noun: Noun, axis: &str, space: &NounSpace) -> Option<Noun> {
    let mut parts = axis.split('.');
    if parts.next()? != "1" {
        return None;
    }
    let mut cur = noun;
    for part in parts {
        cur = match part {
            "2" => noun_head(cur, space)?,
            "3" => noun_tail(cur, space)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Search for spot nouns `[[path...] [[line col] [line col]]]` whose start
/// line matches, printing each match's path-to-root context heads.
fn find_spot(jam: &[u8], file_frag: &str, line: u64) {
    let mut stack = NockStack::new(NOCK_STACK_SIZE_MEDIUM * 8, 0);
    let root = cue(&mut stack, jam, "input");
    let space = stack.noun_space();
    // Iterative DFS carrying (noun, depth); track parent op heads in a side
    // stack keyed by depth.
    let mut work: Vec<(Noun, usize)> = vec![(root, 0)];
    let mut parents: Vec<u64> = Vec::new();
    let mut found = 0usize;
    let mut seen: HashSet<u64> = HashSet::new();
    while let Some((noun, depth)) = work.pop() {
        parents.truncate(depth);
        let Ok(cell) = noun.in_space(&space).as_cell() else {
            continue;
        };
        if !seen.insert(unsafe { noun.as_raw() }) {
            continue;
        }
        let head = cell.head().noun();
        let tail = cell.tail().noun();
        //

        // Does this cell look like [[path] [[l c] [l c]]] with l == line and
        // path containing file_frag?
        let is_spot = (|| {
            let pq = tail.in_space(&space).as_cell().ok()?;
            let start = pq.head().noun().in_space(&space).as_cell().ok()?;
            let l = start
                .head()
                .noun()
                .in_space(&space)
                .as_atom()
                .ok()?
                .as_u64()
                .ok()?;
            if l != line {
                return None;
            }
            // path: cell list of cords; check any cord contains frag.
            let mut p = head;
            let mut matched = false;
            for _ in 0..8 {
                let Ok(pc) = p.in_space(&space).as_cell() else {
                    break;
                };
                if let Ok(atom) = pc.head().noun().in_space(&space).as_atom() {
                    let bytes = atom.as_ne_bytes().to_vec();
                    if String::from_utf8_lossy(&bytes).contains(file_frag) {
                        matched = true;
                    }
                }
                p = pc.tail().noun();
            }
            matched.then_some(())
        })();
        if is_spot.is_some() {
            found += 1;
            let context: Vec<String> = parents
                .iter()
                .rev()
                .take(6)
                .map(|h| h.to_string())
                .collect();
            println!(
                "spot match #{found} at depth {depth}; nearest parent heads (closest first): {}",
                context.join(", ")
            );
            if found >= 12 {
                println!("(stopping after 12 matches)");
                return;
            }
            continue;
        }
        let head_atom = head
            .in_space(&space)
            .as_atom()
            .ok()
            .and_then(|a| a.as_u64().ok())
            .unwrap_or(u64::MAX);
        parents.push(head_atom);
        work.push((tail, depth + 1));
        work.push((head, depth + 1));
    }
    println!("total matches: {found}");
}

struct AtomDiff {
    axis: String,
    left_u64: Option<u64>,
    right_u64: Option<u64>,
}

/// Walk both trees collecting atom-leaf differences (up to `cap`), instead
/// of stopping at the first mismatch like `structural_diff`. A shape
/// mismatch (atom vs cell) is an immediate error: kernel parity tolerates
/// differing atom values (the dir-hash leaf), never differing structure.
fn collect_atom_diffs(
    left: Noun,
    right: Noun,
    left_space: &NounSpace,
    right_space: &NounSpace,
    cap: usize,
) -> Result<Vec<AtomDiff>, String> {
    let mut todo = vec![(left, right, "1".to_string())];
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut diffs = Vec::new();
    while let Some((left, right, axis)) = todo.pop() {
        let raw_pair = unsafe { (left.as_raw(), right.as_raw()) };
        if !seen.insert(raw_pair) {
            continue;
        }
        match (atom_bytes(left, left_space), atom_bytes(right, right_space)) {
            (Some(left_bytes), Some(right_bytes)) => {
                if left_bytes != right_bytes {
                    diffs.push(AtomDiff {
                        axis: axis.clone(),
                        left_u64: atom_u64(left, left_space),
                        right_u64: atom_u64(right, right_space),
                    });
                    if diffs.len() > cap {
                        return Err(format!("more than {cap} atom differences; aborting"));
                    }
                }
            }
            (None, None) => {
                let (Some(lh), Some(lt), Some(rh), Some(rt)) = (
                    noun_head(left, left_space),
                    noun_tail(left, left_space),
                    noun_head(right, right_space),
                    noun_tail(right, right_space),
                ) else {
                    return Err(format!("axis={axis} invalid cell"));
                };
                todo.push((lt, rt, format!("{axis}.3")));
                todo.push((lh, rh, format!("{axis}.2")));
            }
            _ => {
                return Err(format!(
                    "axis={axis} shape mismatch: left={} right={}",
                    preview(left, left_space),
                    preview(right, right_space)
                ));
            }
        }
    }
    Ok(diffs)
}

/// Rebuild `root` with the atom at dotted `axis` replaced by `value`.
fn substitute_at_axis(
    stack: &mut NockStack,
    root: Noun,
    axis: &str,
    value: Noun,
    space: &NounSpace,
) -> Option<Noun> {
    use nockvm::noun::T;
    fn go(
        stack: &mut NockStack,
        cur: Noun,
        parts: &[&str],
        value: Noun,
        space: &NounSpace,
    ) -> Option<Noun> {
        let Some((first, rest)) = parts.split_first() else {
            return Some(value);
        };
        let head = noun_head(cur, space)?;
        let tail = noun_tail(cur, space)?;
        match *first {
            "2" => {
                let new_head = go(stack, head, rest, value, space)?;
                Some(T(stack, &[new_head, tail]))
            }
            "3" => {
                let new_tail = go(stack, tail, rest, value, space)?;
                Some(T(stack, &[head, new_tail]))
            }
            _ => None,
        }
    }
    let parts: Vec<&str> = axis.split('.').skip(1).collect();
    go(stack, root, &parts, value, space)
}

/// Kernel parity gate: exact byte equality, or equality modulo exactly one
/// direct-atom leaf (the `dir-hash=@uvI` directory mug, which differs
/// between Bazel-sandboxed and local builds). On the dir-hash-only path,
/// additionally substitutes the reference value into the candidate and
/// proves rejam byte-equality through a single canonical jammer.
fn kernel_parity(reference_path: &str, candidate_path: &str) -> i32 {
    let reference = match fs::read(reference_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("failed to read {reference_path}: {err}");
            return 2;
        }
    };
    let candidate = match fs::read(candidate_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("failed to read {candidate_path}: {err}");
            return 2;
        }
    };

    if reference == candidate {
        println!("PASS (exact): {candidate_path} == {reference_path}");
        return 0;
    }

    let mut reference_stack = NockStack::new(NOCK_STACK_SIZE_MEDIUM, 0);
    let mut candidate_stack = NockStack::new(NOCK_STACK_SIZE_MEDIUM, 0);
    let reference_noun = cue(&mut reference_stack, &reference, "reference");
    let candidate_noun = cue(&mut candidate_stack, &candidate, "candidate");
    let reference_space = reference_stack.noun_space();
    let candidate_space = candidate_stack.noun_space();

    let diffs = match collect_atom_diffs(
        reference_noun, candidate_noun, &reference_space, &candidate_space, 16,
    ) {
        Ok(diffs) => diffs,
        Err(err) => {
            eprintln!("FAIL (structure): {candidate_path} vs {reference_path}: {err}");
            return 1;
        }
    };

    match diffs.as_slice() {
        [] => {
            // Structurally identical but bytes differ: encoding difference
            // (padding/sharing). Prove equivalence through one jammer.
            let reference_rejam = rejam(&reference, "reference");
            let candidate_rejam = rejam(&candidate, "candidate");
            if reference_rejam == candidate_rejam {
                println!("PASS (rejam): {candidate_path} == {reference_path} modulo encoding");
                0
            } else {
                eprintln!(
                    "FAIL (encoding): {candidate_path} vs {reference_path}: structurally equal but rejams differ"
                );
                1
            }
        }
        [diff] => {
            let (Some(reference_value), Some(candidate_value)) = (diff.left_u64, diff.right_u64)
            else {
                eprintln!(
                    "FAIL: single differing leaf at axis={} is not a direct-atom pair (dir-hash must be @uvI)",
                    diff.axis
                );
                return 1;
            };
            if reference_value > u32::MAX as u64 || candidate_value > u32::MAX as u64 {
                eprintln!(
                    "FAIL: single differing leaf at axis={} exceeds 32 bits (left={reference_value} right={candidate_value}); not a dir-hash",
                    diff.axis
                );
                return 1;
            }
            // Substitute the reference dir-hash into the candidate and
            // require byte-identical canonical jams.
            let substituted = substitute_at_axis(
                &mut candidate_stack,
                candidate_noun,
                &diff.axis,
                nockvm::noun::D(reference_value),
                &candidate_space,
            );
            let Some(substituted) = substituted else {
                eprintln!("FAIL: could not substitute dir-hash at axis={}", diff.axis);
                return 1;
            };
            let candidate_space = candidate_stack.noun_space();
            let mut slab: NounSlab<NockJammer> = NounSlab::new();
            let root = slab.copy_into(substituted, &candidate_space);
            slab.set_root(root);
            let substituted_jam = slab.jam().to_vec();
            let reference_rejam = rejam(&reference, "reference");
            if substituted_jam == reference_rejam {
                println!(
                    "PASS (dir-hash only): {candidate_path} == {reference_path} modulo dir-hash at axis={} (reference={reference_value:#x} candidate={candidate_value:#x})",
                    diff.axis
                );
                0
            } else {
                eprintln!(
                    "FAIL: dir-hash substitution at axis={} does not reconcile the artifacts",
                    diff.axis
                );
                1
            }
        }
        many => {
            eprintln!(
                "FAIL: {} differing atom leaves between {candidate_path} and {reference_path}:",
                many.len()
            );
            for diff in many.iter().take(16) {
                eprintln!(
                    "  axis={} left={:?} right={:?}",
                    diff.axis, diff.left_u64, diff.right_u64
                );
            }
            1
        }
    }
}

fn scan_semi(jam_path: &str) {
    let jam = fs::read(jam_path).unwrap_or_else(|e| {
        eprintln!("read {jam_path}: {e}");
        process::exit(1);
    });
    let mut stack = NockStack::new(NOCK_STACK_SIZE_MEDIUM, 0);
    let root = cue(&mut stack, &jam, "scan");
    let space = stack.noun_space();
    // Walk: find seminoun cells `[[%full 0] data]`; classify data as bare fragment
    // `[0 X]`, spotted fragment `[11 [%spot] [0 X]]`, or other. Report fragment axis
    // X + tree depth to derive hoonc's strip rule.
    let mut bare: Vec<(u64, usize)> = Vec::new();
    let mut spotted: Vec<(u64, usize)> = Vec::new();
    let mut stackv: Vec<(Noun, usize)> = vec![(root, 0)];
    let mut seen = 0usize;
    let mut truncated = false;
    while let Some((n, depth)) = stackv.pop() {
        seen += 1;
        if seen > 8_000_000 {
            truncated = true;
            break;
        }
        let Some(head) = noun_head(n, &space) else {
            continue;
        };
        let Some(tail) = noun_tail(n, &space) else {
            continue;
        };
        // is head == [%full 0]?
        let is_full = noun_head(head, &space)
            .and_then(|h| atom_string(h, &space))
            .map(|s| s == "full")
            .unwrap_or(false)
            && noun_tail(head, &space).and_then(|t| atom_u64(t, &space)) == Some(0);
        if is_full {
            // tail is the seminoun data
            if let Some(dh) = noun_head(tail, &space) {
                if atom_u64(dh, &space) == Some(0) {
                    if let Some(ax) = noun_tail(tail, &space).and_then(|t| atom_u64(t, &space)) {
                        bare.push((ax, depth));
                    }
                } else if atom_u64(dh, &space) == Some(11) {
                    // [11 [hint inner]] ; hint=[%spot ..]; inner=[0 X]
                    if let Some(rest) = noun_tail(tail, &space) {
                        let hint = noun_head(rest, &space);
                        let inner = noun_tail(rest, &space);
                        let is_spot = hint
                            .and_then(|h| noun_head(h, &space))
                            .and_then(|h| atom_string(h, &space))
                            .map(|s| s == "spot")
                            .unwrap_or(false);
                        if is_spot {
                            if let Some(inner) = inner {
                                if noun_head(inner, &space).and_then(|h| atom_u64(h, &space))
                                    == Some(0)
                                {
                                    if let Some(ax) =
                                        noun_tail(inner, &space).and_then(|t| atom_u64(t, &space))
                                    {
                                        spotted.push((ax, depth));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        stackv.push((head, depth + 1));
        stackv.push((tail, depth + 1));
    }
    let axes = |v: &[(u64, usize)]| {
        let mut a: Vec<u64> = v.iter().map(|x| x.0).collect();
        a.sort();
        a.dedup();
        a
    };
    let depths = |v: &[(u64, usize)]| -> Option<(usize, usize)> {
        let mut depths = v.iter().map(|(_, depth)| *depth);
        let first = depths.next()?;
        Some(depths.fold((first, first), |(mn, mx), depth| {
            (mn.min(depth), mx.max(depth))
        }))
    };
    if truncated {
        println!("(truncated at {seen} nodes)");
    }
    println!(
        "bare-fragment seminouns: {} | axes={:?} | depth-range={:?}",
        bare.len(),
        axes(&bare),
        depths(&bare)
    );
    println!(
        "spotted-fragment seminouns: {} | axes={:?} | depth-range={:?}",
        spotted.len(),
        axes(&spotted),
        depths(&spotted)
    );
    println!(
        "bare samples (axis,depth): {:?}",
        bare.iter().take(20).collect::<Vec<_>>()
    );
    println!(
        "spotted samples (axis,depth): {:?}",
        spotted.iter().take(20).collect::<Vec<_>>()
    );
}

fn count_tags(jam_path: &str) {
    let jam = fs::read(jam_path).unwrap_or_else(|e| {
        eprintln!("read {jam_path}: {e}");
        process::exit(1);
    });
    let mut stack = NockStack::new(NOCK_STACK_SIZE_MEDIUM, 0);
    let root = cue(&mut stack, &jam, "count");
    let space = stack.noun_space();
    use std::collections::{HashMap, HashSet};
    let mut counts: HashMap<String, u64> = HashMap::new();
    let mut visited = HashSet::new();
    let mut stackv: Vec<Noun> = vec![root];
    let mut seen = 0usize;
    let tags = ["hold", "cell", "atom", "core", "face", "fork", "hint", "void", "noun"];
    while let Some(n) = stackv.pop() {
        let raw = unsafe { n.as_raw() };
        if !visited.insert(raw) {
            continue;
        }
        seen += 1;
        if seen > 30_000_000 {
            println!("(truncated at {seen} nodes)");
            break;
        }
        let Some(head) = noun_head(n, &space) else {
            continue;
        };
        if let Some(s) = atom_string(head, &space) {
            if tags.contains(&s.as_str()) {
                *counts.entry(s).or_insert(0) += 1;
            }
        }
        if let Some(tail) = noun_tail(n, &space) {
            stackv.push(tail);
        }
        if let Some(head) = noun_head(n, &space) {
            stackv.push(head);
        }
    }
    println!("nodes walked: {seen}");
    let mut sorted: Vec<_> = counts.into_iter().collect();
    sorted.sort();
    for (k, v) in sorted {
        println!("  %{k}: {v}");
    }
}

fn preview_axis(jam_path: &str, axis: &str, depth: usize) {
    let jam = fs::read(jam_path).unwrap_or_else(|e| {
        eprintln!("read {jam_path}: {e}");
        process::exit(1);
    });
    let mut stack = NockStack::new(NOCK_STACK_SIZE_MEDIUM, 0);
    let root = cue(&mut stack, &jam, "preview");
    let space = stack.noun_space();
    let Some(node) = axis_at(root, axis, &space) else {
        eprintln!("axis {axis} not found");
        process::exit(1);
    };
    println!("{}", preview_with_depth(node, depth, &space));
}

fn main() {
    let program = env::args().next().unwrap_or_else(|| "jam-diff".to_string());
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.first().map(String::as_str) == Some("--preview") {
        if args.len() < 3 {
            eprintln!("usage: {program} --preview <jam> <axis> [depth]");
            process::exit(2);
        }
        let depth = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(8);
        preview_axis(&args[1], &args[2], depth);
        return;
    }
    if args.first().map(String::as_str) == Some("--count-tags") {
        if args.len() != 2 {
            eprintln!("usage: {program} --count-tags <jam>");
            process::exit(2);
        }
        count_tags(&args[1]);
        return;
    }
    if args.first().map(String::as_str) == Some("--scan-semi") {
        if args.len() != 2 {
            eprintln!("usage: {program} --scan-semi <jam>");
            process::exit(2);
        }
        scan_semi(&args[1]);
        return;
    }
    if args.first().map(String::as_str) == Some("--kernel-parity") {
        if args.len() != 3 {
            eprintln!("usage: {program} --kernel-parity <reference.jam> <candidate.jam>");
            process::exit(2);
        }
        process::exit(kernel_parity(&args[1], &args[2]));
    }
    if args.first().map(String::as_str) == Some("--find-spot") {
        if args.len() != 4 {
            eprintln!("usage: {program} --find-spot <input.jam> <file-frag> <line>");
            process::exit(2);
        }
        let input = fs::read(&args[1]).unwrap_or_else(|err| {
            eprintln!("failed to read {}: {err}", args[1]);
            process::exit(1);
        });
        let line = args[3].parse::<u64>().unwrap_or_else(|err| {
            eprintln!("bad line: {err}");
            process::exit(2);
        });
        find_spot(&input, &args[2], line);
        return;
    }
    if args.first().map(String::as_str) == Some("--extract-axis") {
        if args.len() != 4 {
            eprintln!("{}", usage(&program));
            process::exit(2);
        }
        let input = fs::read(&args[1]).unwrap_or_else(|err| {
            eprintln!("failed to read {}: {err}", args[1]);
            process::exit(1);
        });
        let output = jam_axis(&input, &args[2]).unwrap_or_else(|| {
            eprintln!("axis {} not found in {}", args[2], args[1]);
            process::exit(1);
        });
        fs::write(&args[3], output).unwrap_or_else(|err| {
            eprintln!("failed to write {}: {err}", args[3]);
            process::exit(1);
        });
        return;
    }
    if args.len() < 2 || args.len() > 5 {
        eprintln!("{}", usage(&program));
        process::exit(2);
    }
    let max_nodes = args
        .get(2)
        .map(|value| value.parse::<usize>())
        .transpose()
        .unwrap_or_else(|err| {
            eprintln!("invalid max_nodes: {err}");
            process::exit(2);
        })
        .unwrap_or(5_000_000);

    let left = fs::read(&args[0]).unwrap_or_else(|err| {
        eprintln!("failed to read {}: {err}", args[0]);
        process::exit(1);
    });
    let right = fs::read(&args[1]).unwrap_or_else(|err| {
        eprintln!("failed to read {}: {err}", args[1]);
        process::exit(1);
    });

    println!(
        "raw: left_len={} right_len={} first_byte_diff={:?}",
        left.len(),
        right.len(),
        first_byte_diff(&left, &right)
    );

    let left_rejam = rejam(&left, "left");
    let right_rejam = rejam(&right, "right");
    println!(
        "rejam: left_len={} right_len={} first_byte_diff={:?}",
        left_rejam.len(),
        right_rejam.len(),
        first_byte_diff(&left_rejam, &right_rejam)
    );

    let mut left_stack = NockStack::new(NOCK_STACK_SIZE_MEDIUM, 0);
    let mut right_stack = NockStack::new(NOCK_STACK_SIZE_MEDIUM, 0);
    let left_noun = cue(&mut left_stack, &left, "left");
    let right_noun = cue(&mut right_stack, &right, "right");
    let left_space = left_stack.noun_space();
    let right_space = right_stack.noun_space();
    let (left_help_nodes, left_help) = collect_help_summaries(left_noun, max_nodes, &left_space);
    let (right_help_nodes, right_help) =
        collect_help_summaries(right_noun, max_nodes, &right_space);
    let left_map = left_help
        .iter()
        .filter(|summary| summary.as_str() == "(map)")
        .count();
    let right_map = right_help
        .iter()
        .filter(|summary| summary.as_str() == "(map)")
        .count();
    println!(
        "help: left_nodes={} left_count={} left_map={} right_nodes={} right_count={} right_map={}",
        left_help_nodes,
        left_help.len(),
        left_map,
        right_help_nodes,
        right_help.len(),
        right_map
    );
    if left_map != right_map || left_help.len() != right_help.len() {
        let left_preview = left_help.iter().take(12).cloned().collect::<Vec<_>>();
        let right_preview = right_help.iter().take(12).cloned().collect::<Vec<_>>();
        let (only_left, only_right) = multiset_delta(&left_help, &right_help);
        println!("help_preview: left={left_preview:?} right={right_preview:?}");
        println!(
            "help_delta: only_left={:?} only_right={:?}",
            only_left.into_iter().take(24).collect::<Vec<_>>(),
            only_right.into_iter().take(24).collect::<Vec<_>>()
        );
    }
    if let Some(left_axis) = args.get(3) {
        if let Some(needle) = left_axis.strip_prefix("help:") {
            let left_matches = collect_help_summary_axes(left_noun, needle, max_nodes, &left_space);
            let right_matches =
                collect_help_summary_axes(right_noun, needle, max_nodes, &right_space);
            println!("help_axes left={left_matches:?}");
            println!("help_axes right={right_matches:?}");
            return;
        }
        let right_axis = args.get(4).unwrap_or(left_axis);
        let left_axis_noun = axis_at(left_noun, left_axis, &left_space);
        let right_axis_noun = axis_at(right_noun, right_axis, &right_space);
        println!(
            "axis: left_axis={} left={} right_axis={} right={}",
            left_axis,
            left_axis_noun
                .map(|noun| preview_with_depth(noun, 8, &left_space))
                .unwrap_or_else(|| "<missing>".to_string()),
            right_axis,
            right_axis_noun
                .map(|noun| preview_with_depth(noun, 8, &right_space))
                .unwrap_or_else(|| "<missing>".to_string())
        );
        if let (Some(left_axis_noun), Some(right_axis_noun)) = (left_axis_noun, right_axis_noun) {
            let (axis_compared, axis_truncated, axis_diff) = structural_diff(
                left_axis_noun, right_axis_noun, max_nodes, &left_space, &right_space,
            );
            println!(
                "axis_structural: compared_nodes={} truncated={} diff={}",
                axis_compared,
                axis_truncated,
                axis_diff.as_deref().unwrap_or("<none>")
            );
        }
    }
    let (compared, truncated, diff) =
        structural_diff(left_noun, right_noun, max_nodes, &left_space, &right_space);
    println!(
        "structural: compared_nodes={} truncated={} diff={}",
        compared,
        truncated,
        diff.as_deref().unwrap_or("<none>")
    );
}
