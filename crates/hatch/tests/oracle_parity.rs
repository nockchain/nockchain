use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use chumsky::Parser;
use either::Either;
use hatch::native_parser;
use hatch::utils::{hoon_to_noun, print_noun, LineMap};
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockvm::ext::noun_equality;
use nockvm::noun::{Noun, NounAllocator, NounSpace, T};
use nockvm_macros::tas;

const HOON_DIR: &str = "../../hoon";

fn repo_hoon_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(HOON_DIR);
    Ok(path.canonicalize()?)
}

fn parser_oracle_source_path(case: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(repo_hoon_dir()?
        .join("tests")
        .join("parser-oracle")
        .join(format!("{case}.hoon"))
        .canonicalize()?)
}

fn hoon_138_source_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../hoonc/hoon/hoon-138.hoon")
        .canonicalize()?)
}

fn required_oracle_fixture(case: &str, suffix: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test-assets")
        .join("arbitrary-jams")
        .join(format!("{case}.{suffix}.jam"));
    fs::read(&path).unwrap_or_else(|err| {
        panic!(
            "missing parser oracle fixture {}\nrun `make build-hatch-test-assets`\n(Bazel target: //open/crates/hatch/test-assets:oracle_parse_ast_jams)\nread error: {err}",
            path.display()
        )
    })
}

fn required_oracle_parse_ast_fixture(case: &str) -> Vec<u8> {
    required_oracle_fixture(case, "parse-ast")
}

fn hoon_path_for_file(
    path: &Path,
    deps_dir: &Path,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let rel = path.strip_prefix(deps_dir).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "native parser path is not under hoon dir: {}",
                path.display()
            ),
        )
    })?;
    Ok(rel
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect())
}

fn hoon_path_for_absolute(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(seg) => Some(seg.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn hoon_path_for_any(path: &Path, deps_dir: &Path) -> Vec<String> {
    hoon_path_for_file(path, deps_dir).unwrap_or_else(|_| hoon_path_for_absolute(path))
}

fn parse_native_ast_jam(
    path: &Path,
    deps_dir: &Path,
    dbug: bool,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let source = fs::read_to_string(path)?;
    let linemap = Arc::new(LineMap::new(&source));
    let wer = hoon_path_for_any(path, deps_dir);
    let parsed = native_parser(wer, dbug, linemap)
        .parse(source.as_str())
        .into_result()
        .map_err(|errs| {
            std::io::Error::other(format!(
                "native parser failed for {}: {:?}",
                path.display(),
                errs
            ))
        })?;
    let mut slab = NounSlab::new();
    let noun = hoon_to_noun(&mut slab, &parsed);
    slab.set_root(noun);
    Ok(slab.jam().to_vec())
}

struct OwnedCue {
    slab: NounSlab<NockJammer>,
    root: Noun,
}

fn cue_root(bytes: &[u8]) -> Result<OwnedCue, Box<dyn std::error::Error>> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let root = slab.cue_into(Bytes::copy_from_slice(bytes))?;
    Ok(OwnedCue { slab, root })
}
fn expect_cell(noun: Noun, space: &NounSpace, context: &str) -> Option<(Noun, Noun)> {
    let cell = noun.in_space(space).as_cell().ok()?;
    let _ = context;
    Some((cell.head().noun(), cell.tail().noun()))
}

fn atom_string(noun: Noun, space: &NounSpace) -> Option<String> {
    let atom = noun.in_space(space).as_atom().ok()?;
    let text = atom.into_string().ok()?;
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn path_noun_to_string(noun: Noun, space: &NounSpace) -> Option<String> {
    let mut parts = Vec::new();
    let mut cursor = noun;
    loop {
        if cursor
            .in_space(space)
            .as_atom()
            .ok()
            .and_then(|atom| atom.as_u64().ok())
            == Some(0)
        {
            break;
        }
        let (head, tail) = expect_cell(cursor, space, "path")?;
        parts.push(atom_string(head, space)?);
        cursor = tail;
    }
    Some(format!("/{}", parts.join("/")))
}

fn skip_dbug(mut noun: Noun, space: &NounSpace) -> Noun {
    loop {
        let Ok(cell) = noun.in_space(space).as_cell() else {
            return noun;
        };
        let Ok(head) = cell.head().as_atom() else {
            return noun;
        };
        if head.as_u64().ok() != Some(tas!(b"dbug")) {
            return noun;
        }
        let Ok(tail_cell) = cell.tail().as_cell() else {
            return noun;
        };
        noun = tail_cell.tail().noun();
    }
}

fn strip_dbug_tree(slab: &mut NounSlab, noun: Noun, space: &NounSpace) -> Noun {
    let noun = skip_dbug(noun, space);
    match noun.in_space(space).as_either_atom_cell() {
        Either::Left(_) => slab.copy_into(noun, space),
        Either::Right(cell) => {
            let head = strip_dbug_tree(slab, cell.head().noun(), space);
            let tail = strip_dbug_tree(slab, cell.tail().noun(), space);
            T(slab, &[head, tail])
        }
    }
}

struct Mismatch {
    axis: u64,
    expected: Noun,
    actual: Noun,
    parent_axis: Option<u64>,
    parent_expected: Option<Noun>,
    parent_actual: Option<Noun>,
}

impl Mismatch {
    fn with_parent(mut self, axis: u64, expected: Noun, actual: Noun) -> Self {
        if self.parent_axis.is_none() {
            self.parent_axis = Some(axis);
            self.parent_expected = Some(expected);
            self.parent_actual = Some(actual);
        }
        self
    }
}

type SourcePosition = (u64, u64);
type Spot = (String, SourcePosition, SourcePosition);

fn decode_pint(noun: Noun, space: &NounSpace) -> Option<(SourcePosition, SourcePosition)> {
    let (p, q) = expect_cell(noun, space, "pint")?;
    let (pl, pc) = expect_cell(p, space, "pint p")?;
    let (ql, qc) = expect_cell(q, space, "pint q")?;
    Some((
        (
            pl.in_space(space).as_atom().ok()?.as_u64().ok()?,
            pc.in_space(space).as_atom().ok()?.as_u64().ok()?,
        ),
        (
            ql.in_space(space).as_atom().ok()?.as_u64().ok()?,
            qc.in_space(space).as_atom().ok()?.as_u64().ok()?,
        ),
    ))
}

fn decode_spot(noun: Noun, space: &NounSpace) -> Option<Spot> {
    let (path, pint) = expect_cell(noun, space, "spot")?;
    let (start, end) = decode_pint(pint, space)?;
    Some((path_noun_to_string(path, space)?, start, end))
}

fn line_excerpt(path: &str, line: u64) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    contents
        .lines()
        .nth(line.saturating_sub(1) as usize)
        .map(|s| s.to_string())
}

fn noun_at_axis(mut noun: Noun, space: &NounSpace, axis: u64) -> Option<Noun> {
    if axis == 1 {
        return Some(noun);
    }
    let mut bits = Vec::new();
    let mut cursor = axis;
    while cursor > 1 {
        bits.push(cursor & 1);
        cursor >>= 1;
    }
    for bit in bits.into_iter().rev() {
        let cell = noun.in_space(space).as_cell().ok()?;
        noun = if bit == 0 {
            cell.head().noun()
        } else {
            cell.tail().noun()
        };
    }
    Some(noun)
}

fn describe_nearest_dbug_with_excerpt(noun: Noun, space: &NounSpace, axis: u64) -> Option<String> {
    let mut cursor = axis;
    loop {
        let node = noun_at_axis(noun, space, cursor)?;
        if let Ok(cell) = node.in_space(space).as_cell() {
            if let Ok(atom) = cell.head().as_atom() {
                if atom.as_u64().ok() == Some(tas!(b"dbug")) {
                    let (spot, _rest) = expect_cell(cell.tail().noun(), space, "dbug tail")?;
                    if let Some((path, (pl, pc), (ql, qc))) = decode_spot(spot, space) {
                        if let Some(text) = line_excerpt(&path, pl) {
                            return Some(format!("{path} [{pl} {pc}] [{ql} {qc}] :: {text}"));
                        }
                        return Some(format!("{path} [{pl} {pc}] [{ql} {qc}]"));
                    }
                }
            }
        }
        if cursor == 1 {
            break;
        }
        cursor >>= 1;
    }
    None
}

fn noun_equal(
    expected: Noun,
    expected_space: &NounSpace,
    actual: Noun,
    actual_space: &NounSpace,
) -> bool {
    noun_equality(
        expected.in_space(expected_space),
        actual.in_space(actual_space),
    )
}

fn find_mismatch_axis(
    expected: Noun,
    expected_space: &NounSpace,
    actual: Noun,
    actual_space: &NounSpace,
    axis: u64,
) -> Option<Mismatch> {
    let expected = skip_dbug(expected, expected_space);
    let actual = skip_dbug(actual, actual_space);
    if noun_equal(expected, expected_space, actual, actual_space) {
        return None;
    }
    match (
        expected.in_space(expected_space).as_either_atom_cell(),
        actual.in_space(actual_space).as_either_atom_cell(),
    ) {
        (Either::Right(ec), Either::Right(ac)) => {
            if let Some(mismatch) = find_mismatch_axis(
                ec.head().noun(),
                expected_space,
                ac.head().noun(),
                actual_space,
                axis * 2,
            ) {
                return Some(mismatch.with_parent(axis, expected, actual));
            }
            if let Some(mismatch) = find_mismatch_axis(
                ec.tail().noun(),
                expected_space,
                ac.tail().noun(),
                actual_space,
                axis * 2 + 1,
            ) {
                return Some(mismatch.with_parent(axis, expected, actual));
            }
            Some(Mismatch {
                axis,
                expected,
                actual,
                parent_axis: None,
                parent_expected: None,
                parent_actual: None,
            })
        }
        _ => Some(Mismatch {
            axis,
            expected,
            actual,
            parent_axis: None,
            parent_expected: None,
            parent_actual: None,
        }),
    }
}

fn find_mismatch_axis_raw(
    expected: Noun,
    expected_space: &NounSpace,
    actual: Noun,
    actual_space: &NounSpace,
    axis: u64,
) -> Option<Mismatch> {
    if noun_equal(expected, expected_space, actual, actual_space) {
        return None;
    }
    match (
        expected.in_space(expected_space).as_either_atom_cell(),
        actual.in_space(actual_space).as_either_atom_cell(),
    ) {
        (Either::Right(ec), Either::Right(ac)) => {
            if let Some(mismatch) = find_mismatch_axis_raw(
                ec.head().noun(),
                expected_space,
                ac.head().noun(),
                actual_space,
                axis * 2,
            ) {
                return Some(mismatch.with_parent(axis, expected, actual));
            }
            if let Some(mismatch) = find_mismatch_axis_raw(
                ec.tail().noun(),
                expected_space,
                ac.tail().noun(),
                actual_space,
                axis * 2 + 1,
            ) {
                return Some(mismatch.with_parent(axis, expected, actual));
            }
            Some(Mismatch {
                axis,
                expected,
                actual,
                parent_axis: None,
                parent_expected: None,
                parent_actual: None,
            })
        }
        _ => Some(Mismatch {
            axis,
            expected,
            actual,
            parent_axis: None,
            parent_expected: None,
            parent_actual: None,
        }),
    }
}

fn assert_native_ast_matches_oracle_fixture(
    case: &str,
    target: &Path,
    deps_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let expected_bytes = required_oracle_parse_ast_fixture(case);
    let actual_bytes = parse_native_ast_jam(target, deps_dir, true)?;

    let expected_owned = cue_root(&expected_bytes)?;
    let actual_owned = cue_root(&actual_bytes)?;
    let expected_space = expected_owned.slab.noun_space();
    let actual_space = actual_owned.slab.noun_space();
    let expected_raw = expected_owned.root;
    let actual_raw = actual_owned.root;

    if noun_equal(expected_raw, &expected_space, actual_raw, &actual_space) {
        return Ok(());
    }

    let mut expected_clean_slab = NounSlab::new();
    let expected_clean = strip_dbug_tree(&mut expected_clean_slab, expected_raw, &expected_space);
    let expected_clean_space = expected_clean_slab.noun_space();
    let mut actual_clean_slab = NounSlab::new();
    let actual_clean = strip_dbug_tree(&mut actual_clean_slab, actual_raw, &actual_space);
    let actual_clean_space = actual_clean_slab.noun_space();

    let raw = find_mismatch_axis_raw(expected_raw, &expected_space, actual_raw, &actual_space, 1);
    let clean = find_mismatch_axis(expected_raw, &expected_space, actual_raw, &actual_space, 1);

    let mut message = format!(
        "parser oracle parity failed for case={case} target={}\n",
        target.display()
    );

    if let Some(raw) = raw {
        message.push_str(&format!("raw mismatch axis={}\n", raw.axis));
        message.push_str(&format!(
            "raw expected@{}: {}\n",
            raw.axis,
            print_noun(raw.expected.in_space(&expected_space), 12, 0)
        ));
        message.push_str(&format!(
            "raw actual@{}:   {}\n",
            raw.axis,
            print_noun(raw.actual.in_space(&actual_space), 12, 0)
        ));
        if let Some(desc) =
            describe_nearest_dbug_with_excerpt(expected_raw, &expected_space, raw.axis)
        {
            message.push_str(&format!("raw expected nearest dbug: {desc}\n"));
        }
        if let Some(desc) = describe_nearest_dbug_with_excerpt(actual_raw, &actual_space, raw.axis)
        {
            message.push_str(&format!("raw actual nearest dbug:   {desc}\n"));
        }
    }

    if let Some(clean) = clean {
        message.push_str(&format!("clean mismatch axis={}\n", clean.axis));
        message.push_str(&format!(
            "clean expected@{}: {}\n",
            clean.axis,
            print_noun(clean.expected.in_space(&expected_space), 12, 0)
        ));
        message.push_str(&format!(
            "clean actual@{}:   {}\n",
            clean.axis,
            print_noun(clean.actual.in_space(&actual_space), 12, 0)
        ));
    } else {
        message.push_str("clean mismatch axis=<none>\n");
    }

    message.push_str(&format!(
        "clean_equal={}\n",
        noun_equal(expected_clean, &expected_clean_space, actual_clean, &actual_clean_space,)
    ));
    Err(std::io::Error::other(message).into())
}

#[test]
#[ignore = "oracle parity repro; run explicitly after building parser test assets"]
fn native_ast_matches_oracle_parse_fixture_for_qual_tuple() -> Result<(), Box<dyn std::error::Error>>
{
    let deps_dir = repo_hoon_dir()?;
    let target = parser_oracle_source_path("qual_tuple")?;
    assert_native_ast_matches_oracle_fixture("qual_tuple", &target, &deps_dir)
}

#[test]
#[ignore = "oracle parity repro; run explicitly after building parser test assets"]
fn native_ast_matches_oracle_parse_fixture_for_hoon_138() -> Result<(), Box<dyn std::error::Error>>
{
    let deps_dir = repo_hoon_dir()?;
    let target = hoon_138_source_path()?;
    assert_native_ast_matches_oracle_fixture("hoon_138", &target, &deps_dir)
}
