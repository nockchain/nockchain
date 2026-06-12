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
    if axis == "1" {
        return Some(noun);
    }
    let mut cur = noun;
    for part in axis.split('.').skip(1) {
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

fn main() {
    let program = env::args().next().unwrap_or_else(|| "jam-diff".to_string());
    let args = env::args().skip(1).collect::<Vec<_>>();
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
