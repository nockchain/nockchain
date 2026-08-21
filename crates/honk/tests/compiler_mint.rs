#![allow(
    dead_code, clippy::enum_variant_names, clippy::manual_flatten, clippy::needless_match,
    clippy::needless_range_loop, clippy::result_large_err, clippy::type_complexity,
    clippy::useless_format
)]

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use chumsky::Parser;
use hatch::ast::hoon::{
    BaseType, Hoon, Limb, Nock, NounExpr, ParsedAtom, Pint, Skin, Spec, Spot, TermOrPair, Type,
};
use hatch::native_parser;
use hatch::utils::LineMap;
use honk::native::hot::native_hot_state;
use honk::Compiler;
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::utils::{create_context, NOCK_STACK_SIZE};
use nockvm::ext::NounExt;
use nockvm::interpreter::{interpret, Context};
use nockvm::jets::cold::Cold;
use nockvm::mem::NockStack;
use nockvm::noun::{Noun, NounAllocator, NounSpace, D, T};
mod errors {
    pub use honk::errors::{CompilerError, Result};
}

mod native {
    pub mod hot {
        pub use honk::native::hot::native_hot_state;
    }
}

mod arm_runner {
    use nockapp::utils::NOCK_STACK_SIZE;
    use nockvm::ext::NounExt;
    use nockvm::interpreter::Context;
    use nockvm::jets::cold::Cold;
    use nockvm::jets::hot::HotEntry;
    use nockvm::jets::util::kick;
    use nockvm::mem::NockStack;
    use nockvm::noun::{Noun, D, T};
    use nockvm::trace::TraceInfo;

    use crate::errors::{CompilerError, Result};
    use crate::native::hot::native_hot_state;

    #[derive(Debug, Clone)]
    pub struct ArmRun {
        pub result: Noun,
    }

    pub struct ArmRunner {
        context: Context,
    }

    impl ArmRunner {
        pub fn new() -> Result<Self> {
            let context = init_context(None);
            Ok(Self { context })
        }

        pub fn run_arm_jam(
            &mut self,
            core_jam: &[u8],
            axis: u64,
            sample_jam: &[u8],
        ) -> Result<ArmRun> {
            let core = Noun::cue_bytes_slice(&mut self.context.stack, core_jam)
                .map_err(|err| CompilerError::Noun(err.to_string()))?;
            let sample = Noun::cue_bytes_slice(&mut self.context.stack, sample_jam)
                .map_err(|err| CompilerError::Noun(err.to_string()))?;
            self.run_arm(core, axis, sample)
        }

        pub fn run_arm(&mut self, core: Noun, axis: u64, sample: Noun) -> Result<ArmRun> {
            let space = self.context.stack.noun_space();
            let core_cell = core
                .in_space(&space)
                .as_cell()
                .map_err(|err| CompilerError::Noun(err.to_string()))?;
            let battery = core_cell.head().noun();
            let payload = core_cell.tail().noun();
            let new_payload = match payload.in_space(&space).as_cell() {
                Ok(payload_cell) => {
                    let tail = payload_cell.tail().noun();
                    T(&mut self.context.stack, &[sample, tail])
                }
                Err(_) => payload,
            };
            let updated_core = T(&mut self.context.stack, &[battery, new_payload]);
            let result = kick(&mut self.context, updated_core, D(axis))
                .map_err(|err| CompilerError::Noun(err.to_string()))?;
            Ok(ArmRun { result })
        }

        pub fn result_atom_u64(&self, run: &ArmRun) -> std::result::Result<u64, String> {
            let space = self.context.stack.noun_space();
            let atom = run
                .result
                .in_space(&space)
                .as_atom()
                .map_err(|err| err.to_string())?;
            atom.as_u64().map_err(|err| err.to_string())
        }
    }

    fn init_context(extra_hot_state: Option<&[HotEntry]>) -> Context {
        let mut stack: NockStack = NockStack::new(NOCK_STACK_SIZE, 0);
        let cold = Cold::new(&mut stack);
        let constant_hot_state = if let Some(hot_state) = extra_hot_state {
            [native_hot_state(), hot_state].concat()
        } else {
            native_hot_state().to_vec()
        };
        let trace_info: Option<TraceInfo> = None;
        nockapp::utils::create_context(
            stack,
            &constant_hot_state,
            cold,
            trace_info,
            vec![],
            nockvm::jets::JetDispatchMode::HintBlind,
        )
    }
}

use arm_runner::ArmRunner;

fn repo_path(path: &str) -> PathBuf {
    if let Ok(test_srcdir) = std::env::var("TEST_SRCDIR") {
        let workspace = std::env::var("TEST_WORKSPACE").unwrap_or_else(|_| "_main".to_string());
        let runfile_path = PathBuf::from(test_srcdir).join(workspace).join(path);
        if runfile_path.exists() {
            return runfile_path;
        }
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
}

fn parser_dbug_enabled() -> bool {
    std::env::var("HOON_TEST_PARSER_DBUG")
        .ok()
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            !(normalized == "0" || normalized == "false" || normalized == "off")
        })
        .unwrap_or(true)
}

fn parse_expr(src: &str) -> Hoon {
    let linemap = Arc::new(LineMap::new(src));
    native_parser(vec!["test".to_string()], parser_dbug_enabled(), linemap)
        .parse(src)
        .into_result()
        .expect("parse failed")
}

fn wer_from_source_path(path: &Path) -> Vec<String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let components: Vec<String> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(segment) => Some(segment.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if let Some(idx) = components.iter().rposition(|segment| segment == "hoon") {
        components[idx + 1..].to_vec()
    } else {
        components
    }
}

fn parse_expr_with_path(src: &str, path: &Path) -> Hoon {
    let wer = wer_from_source_path(path);
    let linemap = Arc::new(LineMap::new(src));
    native_parser(wer, parser_dbug_enabled(), linemap)
        .parse(src)
        .into_result()
        .expect("parse failed")
}

fn hoon_test_source_path(rel: &str) -> PathBuf {
    repo_path(&format!("hoon/tests/hoon-compiler/{rel}"))
}

fn parse_hoon_test_source_expr(rel: &str) -> Hoon {
    let path = hoon_test_source_path(rel);
    let src = fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!("failed to read hoon test source {}: {err}", path.display());
    });
    parse_expr_with_path(src.as_str(), path.as_path())
}

fn create_native_test_context() -> Context {
    let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
    let cold = Cold::new(&mut stack);
    create_context(
        stack,
        native_hot_state(),
        cold,
        None,
        vec![],
        nockvm::jets::JetDispatchMode::HintBlind,
    )
}

#[tokio::test]
async fn core_extension_preserves_previous_arms_native() {
    // This mirrors the hoon-138 "layering" pattern:
    // a core (`|%`) is compiled, then a second `|%` is compiled in that core-subject,
    // and should still be able to resolve earlier arms (unadorned, without `^`).
    let expr = parse_hoon_test_source_expr("core_extension_preserves_previous_arms.hoon");
    let mut compiler = native_compiler().await;
    let compiled = compiler.compile_expr(&expr).expect("compile failed");
    assert!(
        compiled.arm_map().axis_for("use").is_some(),
        "compiled core should expose use arm"
    );
}

#[test]
fn trap_recursion_can_update_sample_field_native() {
    // hoon-138 `++met` uses a `|-` trap and recurses with `$()` updates targeting sample faces.
    // This should resolve wings like `b` correctly even when the trap subject has been extended
    // via `=+` bindings.
    let expr = parse_expr(
        r#"|=  [a=@ b=@]
^-  @
=+  c=0
|-
?:  =(0 b)  c
$(b 0, c +(c))
"#,
    );

    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    let result = ut.mint_noun(sut, gol, &expr);
    assert!(
        result.is_ok(),
        "trap recursion should compile: {:?}",
        result.err()
    );
}

#[test]
fn trap_recursion_sample_field_with_alias_mold_native() {
    // Like `trap_recursion_can_update_sample_field_native`, but with a sample face whose type
    // is a named mold (`bloq`) defined in the surrounding core. This matches hoon-138 `++met`.
    let expr = parse_expr(
        r#"|%
+$  bloq  @
++  met
  |=  [a=bloq b=@]
  ^-  @
  =+  c=0
  |-
  ?:  =(0 b)  c
  $(b 0, c +(c))
--
"#,
    );

    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    let result = ut.mint_noun(sut, gol, &expr);
    assert!(
        result.is_ok(),
        "trap recursion with alias mold in sample should compile: {:?}",
        result.err()
    );
}

#[test]
fn trap_recursion_sample_field_with_container_molds_native() {
    // Reproduces the surrounding hoon-138 "containers" molds (`bloq`, `step`, `bite`) to ensure
    // `$()` recursion still resolves sample faces in the presence of nested faced molds.
    let expr = parse_expr(
        r#"|%
+$  bloq  @
++  step  _`@u`1
+$  bite  $@(bloq [=bloq =step])
++  met
  |=  [a=bloq b=@]
  ^-  @
  =+  c=0
  |-
  ?:  =(0 b)  c
  $(b 0, c +(c))
--
"#,
    );

    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    let result = ut.mint_noun(sut, gol, &expr);
    assert!(
        result.is_ok(),
        "trap recursion with container molds should compile: {:?}",
        result.err()
    );
}

#[test]
fn layered_core_trap_recursion_resolves_sample_faces_native() {
    // Same idea as hoon-138 layering: build a core (layer1), then extend it (layer2) and ensure
    // a trapped `$()` recursion inside a layer2 arm can still resolve sample faces.
    let expr = parse_expr(
        r#"=>  |%
+$  bloq  @
++  step  _`@u`1
+$  bite  $@(bloq [=bloq =step])
--
|%
++  met
  |=  [a=bloq b=@]
  ^-  @
  =+  c=0
  |-
  ?:  =(0 b)  c
  $(b 0, c +(c))
--
"#,
    );

    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    let result = ut.mint_noun(sut, gol, &expr);
    assert!(
        result.is_ok(),
        "layered core trap recursion should compile: {:?}",
        result.err()
    );
}

async fn native_compiler() -> Compiler {
    Compiler::new().await.expect("native compiler init failed")
}

fn compiled_type_tag(compiled: &honk::Compiled) -> Option<String> {
    let space = compiled.noun_space();
    compiled.ty().tag(&space)
}

async fn compile_native_smoke(expr: &Hoon) -> Vec<u8> {
    let mut native = native_compiler().await;
    let mut compiled = native.compile_expr(expr).expect("native compile failed");

    let jam = compiled.jam();
    assert!(!jam.is_empty(), "compiled jam should be non-empty");
    jam
}

fn eval_formula_jam(jam: &[u8]) -> Vec<u8> {
    let mut context = create_native_test_context();
    let formula = <nockvm::noun::Noun as NounExt>::cue_bytes_slice(&mut context.stack, jam)
        .expect("cue failed");
    let core = interpret(&mut context, D(0), formula).expect("interpret failed");
    core.jam_self(&mut context.stack).0.to_vec()
}

fn eval_formula_atom(jam: &[u8]) -> u64 {
    let mut context = create_native_test_context();
    let formula = <nockvm::noun::Noun as NounExt>::cue_bytes_slice(&mut context.stack, jam)
        .expect("cue failed");
    let formula_space = context.stack.noun_space();
    let formula_desc = noun_desc(formula, &formula_space, 10);
    let value = interpret(&mut context, D(0), formula).unwrap_or_else(|err| {
        panic!("interpret failed: {err:?}; formula={formula_desc}");
    });
    let value_space = context.stack.noun_space();
    value
        .in_space(&value_space)
        .as_atom()
        .expect("result not atom")
        .as_u64()
        .expect("result too large")
}

fn noun_desc(noun: Noun, space: &NounSpace, depth: usize) -> String {
    if depth == 0 {
        return "...".to_string();
    }
    let noun = noun.in_space(space);
    if let Ok(atom) = noun.as_atom() {
        if let Ok(value) = atom.as_u64() {
            return value.to_string();
        }
        return "<big-atom>".to_string();
    }
    if let Ok(cell) = noun.as_cell() {
        let head = noun_desc(cell.head().noun(), space, depth - 1);
        let tail = noun_desc(cell.tail().noun(), space, depth - 1);
        return format!("[{head} {tail}]");
    }
    "<opaque>".to_string()
}

fn eval_formula_desc(jam: &[u8]) -> String {
    let mut context = create_native_test_context();
    let formula = match <nockvm::noun::Noun as NounExt>::cue_bytes_slice(&mut context.stack, jam) {
        Ok(formula) => formula,
        Err(err) => return format!("<cue-error:{err:?}>"),
    };
    let value = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        interpret(&mut context, D(0), formula)
    })) {
        Ok(Ok(value)) => value,
        Ok(Err(err)) => return format!("<interpret-error:{err:?}>"),
        Err(_) => return "<interpret-panic>".to_string(),
    };
    let value_space = context.stack.noun_space();
    noun_desc(value, &value_space, 8)
}

fn jam_atom(value: u64) -> Vec<u8> {
    let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
    D(value).jam_self(&mut stack).0.to_vec()
}

fn jam_cell(head: u64, tail: u64) -> Vec<u8> {
    let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
    let noun = T(&mut stack, &[D(head), D(tail)]);
    noun.jam_self(&mut stack).0.to_vec()
}

fn jam_blake3_hex(jam: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(jam);
    hasher.finalize().to_hex().to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NounShapeKind {
    Atom,
    Cell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NounAxisPath {
    // 2=head, 3=tail from the root axis 1.
    steps: Vec<u8>,
}

impl NounAxisPath {
    fn root() -> Self {
        Self { steps: Vec::new() }
    }

    fn head(&self) -> Self {
        let mut steps = self.steps.clone();
        steps.push(2);
        Self { steps }
    }

    fn tail(&self) -> Self {
        let mut steps = self.steps.clone();
        steps.push(3);
        Self { steps }
    }

    fn axis_u128(&self) -> Option<u128> {
        let mut axis = 1u128;
        for step in &self.steps {
            axis = axis.checked_mul(2)?;
            if *step == 3 {
                axis = axis.checked_add(1)?;
            }
        }
        Some(axis)
    }

    fn display(&self) -> String {
        if let Some(axis) = self.axis_u128() {
            return axis.to_string();
        }
        if self.steps.is_empty() {
            return "1".to_string();
        }
        let suffix = self
            .steps
            .iter()
            .map(|step| step.to_string())
            .collect::<Vec<_>>()
            .join(".");
        format!("1.{suffix}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NounAxisDiffKind {
    Shape {
        left: NounShapeKind,
        right: NounShapeKind,
    },
    AtomValue {
        left: String,
        right: String,
    },
    DepthLimit {
        left: String,
        right: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NounAxisDiffEntry {
    axis: NounAxisPath,
    kind: NounAxisDiffKind,
    left_preview: String,
    right_preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NounStructuralDiff {
    entries: Vec<NounAxisDiffEntry>,
    compared_nodes: usize,
    truncated: bool,
    max_depth_reached: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NounDiffOptions {
    max_nodes: usize,
    max_diffs: usize,
    max_depth: usize,
    ignore_nock_hints: bool,
}

impl Default for NounDiffOptions {
    fn default() -> Self {
        Self {
            max_nodes: 50_000,
            max_diffs: 64,
            max_depth: 64,
            ignore_nock_hints: false,
        }
    }
}

fn noun_diff_logging_enabled() -> bool {
    std::env::var("HOON_TEST_NOUN_DIFF")
        .ok()
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            !(normalized.is_empty()
                || normalized == "0"
                || normalized == "false"
                || normalized == "off")
        })
        .unwrap_or(false)
}

fn atom_value_sig(noun: Noun, space: &NounSpace) -> Option<String> {
    let atom = noun.in_space(space).as_atom().ok()?;
    if let Ok(v) = atom.as_u64() {
        return Some(v.to_string());
    }
    let bytes = atom.as_ne_bytes();
    let preview_len = bytes.len().min(16);
    let mut hex = String::with_capacity(preview_len * 2);
    for b in &bytes[..preview_len] {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{b:02x}");
    }
    Some(format!("0x{hex}..({} bytes)", bytes.len()))
}

fn atom_nouns_equal(left: Noun, right: Noun, space: &NounSpace) -> bool {
    let Ok(left_atom) = left.in_space(space).as_atom() else {
        return false;
    };
    let Ok(right_atom) = right.in_space(space).as_atom() else {
        return false;
    };
    left_atom.as_ne_bytes() == right_atom.as_ne_bytes()
}

fn noun_shape(noun: Noun) -> NounShapeKind {
    if noun.as_atom().is_ok() {
        NounShapeKind::Atom
    } else {
        NounShapeKind::Cell
    }
}

fn structural_nouns_equal(left: Noun, right: Noun, space: &NounSpace) -> bool {
    let mut todo = vec![(left, right)];
    while let Some((left, right)) = todo.pop() {
        if unsafe { left.raw_equals(&right) } {
            continue;
        }
        match (
            left.in_space(space).as_atom(),
            right.in_space(space).as_atom(),
        ) {
            (Ok(left_atom), Ok(right_atom)) => {
                if left_atom.as_ne_bytes() != right_atom.as_ne_bytes() {
                    return false;
                }
            }
            (Err(_), Err(_)) => {
                let Ok(left_cell) = left.in_space(space).as_cell() else {
                    return false;
                };
                let Ok(right_cell) = right.in_space(space).as_cell() else {
                    return false;
                };
                todo.push((left_cell.tail().noun(), right_cell.tail().noun()));
                todo.push((left_cell.head().noun(), right_cell.head().noun()));
            }
            _ => return false,
        }
    }
    true
}

fn strip_nock_hint_wrappers(mut noun: Noun, space: &NounSpace) -> Noun {
    loop {
        let Ok(cell) = noun.in_space(space).as_cell() else {
            return noun;
        };
        let Ok(head_atom) = cell.head().as_atom() else {
            return noun;
        };
        let Ok(head_value) = head_atom.as_u64() else {
            return noun;
        };
        if head_value != 11 {
            return noun;
        }
        let Ok(rest) = cell.tail().as_cell() else {
            return noun;
        };
        noun = rest.tail().noun();
    }
}

fn normalized_diff_node(noun: Noun, ignore_nock_hints: bool, space: &NounSpace) -> Noun {
    if ignore_nock_hints {
        strip_nock_hint_wrappers(noun, space)
    } else {
        noun
    }
}

fn refine_first_diff_axis_in_nouns(
    left_root: Noun,
    right_root: Noun,
    start_axis: u64,
    ignore_nock_hints: bool,
    max_steps: usize,
    space: &NounSpace,
) -> u64 {
    let mut axis = start_axis;
    for _ in 0..max_steps {
        let Some(left_node) =
            slot_axis_with_optional_hint_stripping(left_root, axis, ignore_nock_hints, space)
        else {
            break;
        };
        let Some(right_node) =
            slot_axis_with_optional_hint_stripping(right_root, axis, ignore_nock_hints, space)
        else {
            break;
        };

        let (Ok(left_cell), Ok(right_cell)) = (
            left_node.in_space(space).as_cell(),
            right_node.in_space(space).as_cell(),
        ) else {
            break;
        };

        let left_head = normalized_diff_node(left_cell.head().noun(), ignore_nock_hints, space);
        let right_head = normalized_diff_node(right_cell.head().noun(), ignore_nock_hints, space);
        let left_tail = normalized_diff_node(left_cell.tail().noun(), ignore_nock_hints, space);
        let right_tail = normalized_diff_node(right_cell.tail().noun(), ignore_nock_hints, space);

        let head_diff = !structural_nouns_equal(left_head, right_head, space);
        let tail_diff = !structural_nouns_equal(left_tail, right_tail, space);

        let choose_head = match (head_diff, tail_diff) {
            (true, false) => Some(true),
            (false, true) => Some(false),
            (false, false) => None,
            (true, true) => {
                let head_informative = left_head.as_atom().is_ok() != right_head.as_atom().is_ok()
                    || (left_head.as_atom().is_ok() && right_head.as_atom().is_ok())
                    || (left_head.as_cell().is_err() != right_head.as_cell().is_err());
                let tail_informative = left_tail.as_atom().is_ok() != right_tail.as_atom().is_ok()
                    || (left_tail.as_atom().is_ok() && right_tail.as_atom().is_ok())
                    || (left_tail.as_cell().is_err() != right_tail.as_cell().is_err());
                Some(match (head_informative, tail_informative) {
                    (true, false) => true,
                    (false, true) => false,
                    _ => true,
                })
            }
        };

        let Some(choose_head) = choose_head else {
            break;
        };
        axis = if choose_head {
            axis.saturating_mul(2)
        } else {
            axis.saturating_mul(2).saturating_add(1)
        };
    }
    axis
}

fn noun_structural_diff(
    left: Noun,
    right: Noun,
    options: NounDiffOptions,
    space: &NounSpace,
) -> NounStructuralDiff {
    let mut entries: Vec<NounAxisDiffEntry> = Vec::new();
    let mut compared_nodes = 0usize;
    let mut truncated = false;
    let mut max_depth_reached = false;
    let mut todo: Vec<(Noun, Noun, NounAxisPath, usize)> =
        vec![(left, right, NounAxisPath::root(), 0)];

    while let Some((left_node, right_node, axis, depth)) = todo.pop() {
        let left_node = normalized_diff_node(left_node, options.ignore_nock_hints, space);
        let right_node = normalized_diff_node(right_node, options.ignore_nock_hints, space);
        compared_nodes += 1;
        if compared_nodes > options.max_nodes {
            truncated = true;
            break;
        }
        if entries.len() >= options.max_diffs {
            truncated = true;
            break;
        }
        if depth >= options.max_depth {
            max_depth_reached = true;
            let left_preview = noun_desc(left_node, space, 2);
            let right_preview = noun_desc(right_node, space, 2);
            if left_preview != right_preview {
                entries.push(NounAxisDiffEntry {
                    axis,
                    kind: NounAxisDiffKind::DepthLimit {
                        left: left_preview.clone(),
                        right: right_preview.clone(),
                    },
                    left_preview,
                    right_preview,
                });
            }
            continue;
        }

        match (
            left_node.in_space(space).as_atom(),
            right_node.in_space(space).as_atom(),
        ) {
            (Ok(_), Ok(_)) => {
                if atom_nouns_equal(left_node, right_node, space) {
                    continue;
                }
                let left_sig = atom_value_sig(left_node, space)
                    .unwrap_or_else(|| "<atom:unavailable>".to_string());
                let right_sig = atom_value_sig(right_node, space)
                    .unwrap_or_else(|| "<atom:unavailable>".to_string());
                entries.push(NounAxisDiffEntry {
                    axis,
                    kind: NounAxisDiffKind::AtomValue {
                        left: left_sig.clone(),
                        right: right_sig.clone(),
                    },
                    left_preview: left_sig,
                    right_preview: right_sig,
                });
            }
            (Err(_), Err(_)) => {
                let Ok(left_cell) = left_node.in_space(space).as_cell() else {
                    continue;
                };
                let Ok(right_cell) = right_node.in_space(space).as_cell() else {
                    continue;
                };
                let children: [(Noun, Noun, NounAxisPath); 2] = [
                    (
                        left_cell.head().noun(),
                        right_cell.head().noun(),
                        axis.head(),
                    ),
                    (
                        left_cell.tail().noun(),
                        right_cell.tail().noun(),
                        axis.tail(),
                    ),
                ];
                let mut mismatching_children = Vec::new();
                for (left_child, right_child, child_axis) in children {
                    let left_child =
                        normalized_diff_node(left_child, options.ignore_nock_hints, space);
                    let right_child =
                        normalized_diff_node(right_child, options.ignore_nock_hints, space);
                    if structural_nouns_equal(left_child, right_child, space) {
                        continue;
                    }
                    if entries.len() >= options.max_diffs {
                        truncated = true;
                        break;
                    }
                    if depth + 1 >= options.max_depth {
                        max_depth_reached = true;
                        let left_preview = noun_desc(left_child, space, 2);
                        let right_preview = noun_desc(right_child, space, 2);
                        entries.push(NounAxisDiffEntry {
                            axis: child_axis,
                            kind: NounAxisDiffKind::DepthLimit {
                                left: left_preview.clone(),
                                right: right_preview.clone(),
                            },
                            left_preview,
                            right_preview,
                        });
                        continue;
                    }
                    mismatching_children.push((left_child, right_child, child_axis));
                }
                for (left_child, right_child, child_axis) in mismatching_children.into_iter().rev()
                {
                    todo.push((left_child, right_child, child_axis, depth + 1));
                }
            }
            _ => {
                entries.push(NounAxisDiffEntry {
                    axis,
                    kind: NounAxisDiffKind::Shape {
                        left: noun_shape(left_node),
                        right: noun_shape(right_node),
                    },
                    left_preview: noun_desc(left_node, space, 3),
                    right_preview: noun_desc(right_node, space, 3),
                });
            }
        }
    }

    NounStructuralDiff {
        entries,
        compared_nodes,
        truncated,
        max_depth_reached,
    }
}

#[test]
fn refine_first_diff_axis_descends_into_mismatching_subtree() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let shared = T(&mut slab, &[D(1), D(2)]);
    let left_tail = T(&mut slab, &[D(3), D(4)]);
    let right_tail = T(&mut slab, &[D(3), D(9)]);
    let left_inner = T(&mut slab, &[shared, left_tail]);
    let right_inner = T(&mut slab, &[shared, right_tail]);
    let left = T(&mut slab, &[D(0), left_inner]);
    let right = T(&mut slab, &[D(0), right_inner]);

    let space = slab.noun_space();
    let refined = refine_first_diff_axis_in_nouns(left, right, 3, false, 16, &space);
    assert_eq!(refined, 15);
}

fn noun_structural_diff_from_jams(
    left_jam: &[u8],
    right_jam: &[u8],
    options: NounDiffOptions,
) -> Option<NounStructuralDiff> {
    let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
    let left = <nockvm::noun::Noun as NounExt>::cue_bytes_slice(&mut stack, left_jam).ok()?;
    let right = <nockvm::noun::Noun as NounExt>::cue_bytes_slice(&mut stack, right_jam).ok()?;
    let space = stack.noun_space();
    Some(noun_structural_diff(left, right, options, &space))
}

fn slot_axis_with_optional_hint_stripping(
    mut node: Noun,
    axis: u64,
    strip_hints: bool,
    space: &NounSpace,
) -> Option<Noun> {
    if axis == 0 {
        return None;
    }
    if strip_hints {
        node = strip_nock_hint_wrappers(node, space);
    }
    if axis == 1 {
        return Some(node);
    }
    let depth = 63usize.saturating_sub(axis.leading_zeros() as usize);
    for bit_index in (0..depth).rev() {
        if strip_hints {
            node = strip_nock_hint_wrappers(node, space);
        }
        let cell = node.in_space(space).as_cell().ok()?;
        node = if ((axis >> bit_index) & 1) == 0 {
            cell.head().noun()
        } else {
            cell.tail().noun()
        };
    }
    if strip_hints {
        node = strip_nock_hint_wrappers(node, space);
    }
    Some(node)
}

fn log_noun_structural_diff(label: &str, diff: &NounStructuralDiff) {
    eprintln!(
        "[noun-diff:{label}] compared_nodes={} diffs={} truncated={} max_depth_reached={}",
        diff.compared_nodes,
        diff.entries.len(),
        diff.truncated,
        diff.max_depth_reached
    );
    for (idx, entry) in diff.entries.iter().enumerate() {
        let axis = entry.axis.display();
        match &entry.kind {
            NounAxisDiffKind::Shape { left, right } => {
                eprintln!(
                    "  [{idx}] axis={axis} shape {:?} != {:?}\n      left={}\n      right={}",
                    left, right, entry.left_preview, entry.right_preview
                );
            }
            NounAxisDiffKind::AtomValue { left, right } => {
                eprintln!(
                    "  [{idx}] axis={axis} atom {left} != {right}\n      left={}\n      right={}",
                    entry.left_preview, entry.right_preview
                );
            }
            NounAxisDiffKind::DepthLimit { left, right } => {
                eprintln!(
                    "  [{idx}] axis={axis} depth-limit left={left} right={right}\n      left={}\n      right={}",
                    entry.left_preview, entry.right_preview
                );
            }
        }
    }
}

#[test]
fn noun_structural_diff_descends_to_first_concrete_mismatches() {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let left_head = T(&mut slab, &[D(1), D(2)]);
    let left_tail = T(&mut slab, &[D(3), D(4)]);
    let left = T(&mut slab, &[left_head, left_tail]);
    let right_head = T(&mut slab, &[D(1), D(9)]);
    let right_tail = T(&mut slab, &[D(3), D(8)]);
    let right = T(&mut slab, &[right_head, right_tail]);

    let space = slab.noun_space();
    let diff = noun_structural_diff(
        left,
        right,
        NounDiffOptions {
            max_nodes: 128,
            max_diffs: 8,
            max_depth: 8,
            ignore_nock_hints: false,
        },
        &space,
    );

    let axes = diff
        .entries
        .iter()
        .map(|entry| entry.axis.display())
        .collect::<Vec<_>>();
    assert_eq!(axes, vec!["5", "7"]);
    assert!(matches!(
        diff.entries[0].kind,
        NounAxisDiffKind::AtomValue { .. }
    ));
    assert!(matches!(
        diff.entries[1].kind,
        NounAxisDiffKind::AtomValue { .. }
    ));
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NounAtomStringEntry {
    axis: NounAxisPath,
    text: String,
}

fn try_printable_atom_text(noun: Noun, space: &NounSpace) -> Option<String> {
    let atom = noun.in_space(space).as_atom().ok()?;
    let bytes = atom.as_ne_bytes();
    let text = std::str::from_utf8(bytes)
        .ok()?
        .trim_end_matches('\0')
        .to_string();
    if text.len() < 3 || text.len() > 96 {
        return None;
    }
    if !text
        .bytes()
        .all(|byte| byte == b'\t' || byte == b'\n' || (0x20..=0x7e).contains(&byte))
    {
        return None;
    }
    Some(text)
}

fn collect_printable_atom_strings(
    root: Noun,
    space: &NounSpace,
    max_nodes: usize,
    max_strings: usize,
    max_depth: usize,
) -> Vec<NounAtomStringEntry> {
    let mut out = Vec::new();
    let mut todo: Vec<(Noun, NounAxisPath, usize)> = vec![(root, NounAxisPath::root(), 0)];
    let mut visited = 0usize;
    while let Some((node, axis, depth)) = todo.pop() {
        visited += 1;
        if visited > max_nodes || out.len() >= max_strings {
            break;
        }
        if depth > max_depth {
            continue;
        }
        if let Some(text) = try_printable_atom_text(node, space) {
            out.push(NounAtomStringEntry { axis, text });
            continue;
        }
        let Ok(cell) = node.in_space(space).as_cell() else {
            continue;
        };
        todo.push((cell.tail().noun(), axis.tail(), depth + 1));
        todo.push((cell.head().noun(), axis.head(), depth + 1));
    }
    out
}

fn collect_printable_atom_strings_from_jam(
    jam: &[u8],
    max_nodes: usize,
    max_strings: usize,
    max_depth: usize,
) -> Option<Vec<NounAtomStringEntry>> {
    let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
    let root = <nockvm::noun::Noun as NounExt>::cue_bytes_slice(&mut stack, jam).ok()?;
    let space = stack.noun_space();
    Some(collect_printable_atom_strings(
        root, &space, max_nodes, max_strings, max_depth,
    ))
}

fn hoon_atom(value: u64) -> Hoon {
    Hoon::Sand(
        "ud".to_string(),
        NounExpr::ParsedAtom(ParsedAtom::Small(value as u128)),
    )
}

fn hoon_bool(value: bool) -> Hoon {
    let atom = if value { 0 } else { 1 };
    Hoon::Sand(
        "f".to_string(),
        NounExpr::ParsedAtom(ParsedAtom::Small(atom as u128)),
    )
}

fn bind_face(name: &str, value: Hoon, body: Hoon) -> Hoon {
    Hoon::TisLus(
        Box::new(Hoon::KetTis(Skin::Term(name.to_string()), Box::new(value))),
        Box::new(body),
    )
}

fn set_subject(value: Hoon, body: Hoon) -> Hoon {
    Hoon::TisGar(Box::new(value), Box::new(body))
}

fn simple_spot() -> Spot {
    Spot {
        p: vec!["test".to_string()],
        q: Pint {
            p: (0, 0),
            q: (0, 0),
        },
    }
}

fn atom_string(noun: Noun, space: &NounSpace) -> Option<String> {
    let atom = noun.in_space(space).as_atom().ok()?;
    std::str::from_utf8(atom.as_ne_bytes())
        .ok()
        .map(|s| s.trim_end_matches('\0').to_string())
}

fn find_first_fast_hint(formula: Noun, space: &NounSpace) -> Option<(Noun, Noun)> {
    // Returns (clue, body) for the first `[11 [%fast clue] body]` found.
    let mut todo: Vec<Noun> = vec![formula];
    while let Some(node) = todo.pop() {
        let Ok(cell) = node.in_space(space).as_cell() else {
            continue;
        };
        let head = cell.head().noun();
        let tail = cell.tail().noun();
        todo.push(head);
        todo.push(tail);

        if !unsafe { head.raw_equals(&D(11)) } {
            continue;
        }
        let Ok(rest) = tail.in_space(space).as_cell() else {
            continue;
        };
        let hint = rest.head().noun();
        let body = rest.tail().noun();
        let Ok(hint_cell) = hint.in_space(space).as_cell() else {
            continue;
        };
        let tag = hint_cell.head().noun();
        let clue = hint_cell.tail().noun();
        if atom_string(tag, space).as_deref() == Some("fast") {
            return Some((clue, body));
        }
    }
    None
}

#[test]
fn compile_sigfas_fast_hint_clue_has_parent_axis_7_native() {
    let expr = parse_expr(
        r#"~/  %foo
|=  a=@
a"#,
    );

    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    ut.set_vet(false);
    let (_ty, formula) = ut.mint_noun(sut, gol, &expr).expect("native mint failed");

    let space = slab.noun_space();
    let (clue_formula, _body) = find_first_fast_hint(formula, &space).expect("no %fast hint found");
    // Clue is wrapped in [1 clue_data] (constant formula), matching hoon-138's desugaring.
    let clue_cell = clue_formula
        .in_space(&space)
        .as_cell()
        .expect("clue not cell");
    let clue_head = clue_cell.head().noun();
    assert!(
        unsafe { clue_head.raw_equals(&D(1)) },
        "expected clue formula [1 ...], got head {}",
        noun_desc(clue_head, &space, 2)
    );
    let clue_data = clue_cell.tail().noun();
    let chum = clue_data
        .in_space(&space)
        .slot(2)
        .expect("clue_data.slot(2) failed")
        .noun();
    assert_eq!(atom_string(chum, &space).as_deref(), Some("foo"));

    let parent = clue_data
        .in_space(&space)
        .slot(6)
        .expect("clue_data.slot(6) failed")
        .noun();
    let parent_cell = parent.in_space(&space).as_cell().expect("parent not cell");
    let parent_head = parent_cell.head().noun();
    let parent_tail = parent_cell.tail().noun();
    assert!(
        unsafe { parent_head.raw_equals(&D(0)) },
        "expected parent op 0, got {}",
        noun_desc(parent_head, &space, 2)
    );
    assert!(
        unsafe { parent_tail.raw_equals(&D(7)) },
        "expected parent axis 7, got {}",
        noun_desc(parent_tail, &space, 2)
    );
}

#[test]
fn compile_wuthep_switch_narrows_subject_for_branch_wings_native() {
    // Minimal repro from hoon-138 `++fli` ("flip sign"):
    // `?-(-.a %f a(s !s.a), %i a(s !s.a), %n a)`
    //
    // Without per-branch subject refinement in `mint_wthp`, the `%f`/`%i` branches fail to resolve
    // the `s` wing, since `a` is still the unrefined `$%` union.
    let expr = parse_expr(
        r#"|%
++  fn
  $%  [%f s=? e=@s a=@u]
      [%i s=?]
      [%n ~]
  ==
++  fli
  |=  [a=fn]  ^-  fn
  ?-(-.a %f a(s !s.a), %i a(s !s.a), %n a)
--"#,
    );

    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    ut.set_vet(false);

    ut.mint_noun(sut, gol, &expr)
        .expect("native mint of ?- branch wing narrowing repro failed");
}

fn prelude_core() -> Hoon {
    parse_expr(
        "|%\n  ++  noah  |=  a=*  a\n  ++  cain  |=  a=*  a\n  ++  onan  |=  a=*  a\n  ++  abel  |=  a=*  a\n  ++  levi  |=  [a=* b=*]  &\n--\n",
    )
}

fn with_prelude(prelude: &Hoon, expr: Hoon) -> Hoon {
    Hoon::TisGar(Box::new(prelude.clone()), Box::new(expr))
}

#[tokio::test]
async fn compile_representative_native_semantics() {
    enum Expect {
        Atom(u64),
        Desc(&'static str),
    }

    let cases: Vec<(&str, Hoon, Option<&'static str>, Expect)> = vec![
        ("atom", hoon_atom(42), Some("atom"), Expect::Atom(42)),
        (
            "subject-push",
            Hoon::TisLus(Box::new(hoon_atom(42)), Box::new(Hoon::Axis((2u64).into()))),
            None,
            Expect::Atom(42),
        ),
        (
            "conditional",
            Hoon::WutCol(
                Box::new(hoon_bool(true)),
                Box::new(hoon_atom(7)),
                Box::new(hoon_atom(9)),
            ),
            None,
            Expect::Atom(7),
        ),
        (
            "face-wing",
            Hoon::TisGar(
                Box::new(Hoon::KetTis(
                    Skin::Term("a".to_string()),
                    Box::new(hoon_atom(42)),
                )),
                Box::new(Hoon::Limb("a".to_string())),
            ),
            None,
            Expect::Atom(42),
        ),
        (
            "bunt-null",
            Hoon::KetTar(Box::new(Spec::Base(BaseType::Null))),
            None,
            Expect::Atom(0),
        ),
        (
            "positive-assert",
            Hoon::WutGar(Box::new(hoon_bool(true)), Box::new(hoon_atom(7))),
            None,
            Expect::Atom(7),
        ),
        (
            "zap-tis",
            Hoon::ZapTis(Box::new(hoon_atom(7))),
            Some("noun"),
            Expect::Desc("[1 7]"),
        ),
    ];

    for (name, expr, expected_tag, expected) in cases {
        let mut native = native_compiler().await;
        let mut compiled = native
            .compile_expr(&expr)
            .unwrap_or_else(|err| panic!("native compile failed for {name}: {err}"));
        if let Some(expected_tag) = expected_tag {
            assert_eq!(
                compiled_type_tag(&compiled).as_deref(),
                Some(expected_tag),
                "unexpected type tag for {name}"
            );
        }
        match expected {
            Expect::Atom(value) => assert_eq!(
                eval_formula_atom(&compiled.jam()),
                value,
                "unexpected evaluated atom for {name}"
            ),
            Expect::Desc(desc) => assert_eq!(
                eval_formula_desc(&compiled.jam()),
                desc,
                "unexpected evaluated shape for {name}"
            ),
        }
    }
}

#[tokio::test]
async fn compile_tsls_bardot_native_fast_repro() {
    let expr = Hoon::TisLus(
        Box::new(hoon_atom(42)),
        Box::new(Hoon::BarDot(Box::new(Hoon::Axis((2u64).into())))),
    );

    let mut native = native_compiler().await;
    let mut compiled = native.compile_expr(&expr).expect("native compile failed");

    assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("core"));
    let desc = eval_formula_desc(&compiled.jam());
    assert_eq!(desc, "[[0 2] [42 0]]");
}

#[tokio::test]
async fn compile_tsls_bardot_wing_native_fast_repro() {
    let expr = Hoon::TisLus(
        Box::new(hoon_atom(42)),
        Box::new(Hoon::BarDot(Box::new(Hoon::Wing(vec![Limb::Axis(
            (2u64).into(),
        )])))),
    );

    let mut native = native_compiler().await;
    let mut compiled = native.compile_expr(&expr).expect("native compile failed");

    assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("core"));
    let desc = eval_formula_desc(&compiled.jam());
    assert_eq!(desc, "[[0 2] [42 0]]");
}

#[tokio::test]
async fn compile_bloq_example_native_fast_repro() {
    let expr = parse_expr(
        r#"|%
+$  bloq  @
--
*bloq
"#,
    );

    let mut native = native_compiler().await;
    let mut compiled = native.compile_expr(&expr).expect("native compile failed");

    // hoon-138's ++fire always returns [%hold core hoon], so the top-level type
    // tag is "hold".  The hold expands (via repo) to a %hint-wrapped atom type,
    // but mint/blow return the hold as-is — matching hoon-138 semantics.
    assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("hold"));
    // The formula evaluates to 0 (the bunt of `@`) because `++musk` constant-folds
    // the `=<($ bloq)` example to `[1 0]`, yielding the atom default value.
    let desc = eval_formula_desc(&compiled.jam());
    assert_eq!(desc, "0", "unexpected bloq formula result shape: {desc}");
}

#[tokio::test]
async fn compile_tsls_cnts_hotspot_native_fast_repro() {
    let subject = Hoon::Pair(
        Box::new(hoon_atom(1)),
        Box::new(Hoon::Pair(Box::new(hoon_atom(2)), Box::new(hoon_atom(3)))),
    );
    let update = Hoon::CenTis(
        vec![Limb::Parent(0, None), Limb::Axis((2u64).into())],
        vec![(
            vec![Limb::Axis((2u64).into())],
            Hoon::TisGar(
                Box::new(Hoon::Axis((3u64).into())),
                Box::new(Hoon::Axis((6u64).into())),
            ),
        )],
    );
    let expr = set_subject(
        subject,
        Hoon::TisLus(Box::new(hoon_atom(42)), Box::new(update)),
    );

    let mut native = native_compiler().await;
    let mut compiled = native.compile_expr(&expr).expect("native compile failed");

    assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("void"));
    let jam = compiled.jam();
    let hash = jam_blake3_hex(&jam);
    assert_eq!(
        hash,
        "8a2c263ad0510de191de4cd364289c168b1812a3f32aaad045a348db0bf97f0b"
    );
}

#[tokio::test]
async fn compile_tsls_cnts_axis3_hotspot_native_fast_repro() {
    let subject = Hoon::Pair(
        Box::new(hoon_atom(1)),
        Box::new(Hoon::Pair(Box::new(hoon_atom(2)), Box::new(hoon_atom(3)))),
    );
    let update = Hoon::CenTis(
        vec![Limb::Axis((3u64).into())],
        vec![(
            vec![Limb::Axis((2u64).into())],
            Hoon::TisGar(
                Box::new(Hoon::Axis((3u64).into())),
                Box::new(Hoon::Axis((6u64).into())),
            ),
        )],
    );
    let expr = set_subject(
        subject,
        Hoon::TisLus(Box::new(hoon_atom(42)), Box::new(update)),
    );

    let mut native = native_compiler().await;
    let mut compiled = native.compile_expr(&expr).expect("native compile failed");

    assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("cell"));
    let desc = eval_formula_desc(&compiled.jam());
    assert_eq!(desc, "[2 [2 3]]");
}

#[tokio::test]
async fn compile_tsls_cnts_wing_patch_hotspot_native_fast_repro() {
    let subject = Hoon::Pair(
        Box::new(hoon_atom(1)),
        Box::new(Hoon::Pair(Box::new(hoon_atom(2)), Box::new(hoon_atom(3)))),
    );
    let update = Hoon::CenTis(
        vec![Limb::Parent(0, None), Limb::Axis((2u64).into())],
        vec![(
            vec![Limb::Axis((2u64).into())],
            Hoon::TisGar(
                Box::new(Hoon::Axis((3u64).into())),
                Box::new(Hoon::Wing(vec![Limb::Axis((6u64).into())])),
            ),
        )],
    );
    let expr = set_subject(
        subject,
        Hoon::TisLus(Box::new(hoon_atom(42)), Box::new(update)),
    );

    let mut native = native_compiler().await;
    let mut compiled = native.compile_expr(&expr).expect("native compile failed");

    assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("void"));
    let jam = compiled.jam();
    let hash = jam_blake3_hex(&jam);
    assert_eq!(
        hash,
        "8a2c263ad0510de191de4cd364289c168b1812a3f32aaad045a348db0bf97f0b"
    );
}

#[tokio::test]
async fn compile_parent_limb_caret_native_fast_repro() {
    let expr = bind_face(
        "a",
        hoon_atom(1),
        bind_face(
            "a",
            hoon_atom(2),
            Hoon::Wing(vec![Limb::Parent(1, Some("a".to_string()))]),
        ),
    );

    let jam = compile_native_smoke(&expr).await;
    let value = eval_formula_atom(&jam);
    assert_eq!(value, 1);
}

#[tokio::test]
async fn compile_parent_limb_comma_native_fast_repro() {
    let expr = bind_face(
        "a",
        hoon_atom(1),
        bind_face(
            "a",
            hoon_atom(2),
            Hoon::Wing(vec![Limb::Parent(0, None), Limb::Term("a".to_string())]),
        ),
    );

    let jam = compile_native_smoke(&expr).await;
    let value = eval_formula_atom(&jam);
    assert_eq!(value, 2);
}

#[tokio::test]
async fn compile_fits_atom_native_fast_repro() {
    let fits = Hoon::Fits(Box::new(hoon_atom(1)), vec![Limb::Axis((1u64).into())]);
    let expr = set_subject(hoon_atom(7), fits);

    let jam = compile_native_smoke(&expr).await;
    let got = eval_formula_atom(&jam);
    assert_eq!(got, 0);
}

#[tokio::test]
async fn compile_fits_cell_native_fast_repro() {
    let fits = Hoon::Fits(
        Box::new(Hoon::Pair(Box::new(hoon_atom(1)), Box::new(hoon_atom(2)))),
        vec![Limb::Axis((1u64).into())],
    );
    let subject = Hoon::Pair(Box::new(hoon_atom(7)), Box::new(hoon_atom(8)));
    let expr = set_subject(subject, fits);

    let jam = compile_native_smoke(&expr).await;
    let got = eval_formula_atom(&jam);
    assert_eq!(got, 0);
}

#[tokio::test]
async fn compile_wthx_atom_native_fast_repro() {
    let skin = Skin::Base(BaseType::Atom("@".to_string()));
    let expr = set_subject(
        hoon_atom(7),
        Hoon::WutHax(skin, vec![Limb::Axis((1u64).into())]),
    );

    let jam = compile_native_smoke(&expr).await;
    let got = eval_formula_atom(&jam);
    assert_eq!(got, 0);
}

#[tokio::test]
async fn compile_wthx_cell_native_fast_repro() {
    let skin = Skin::Cell(
        Box::new(Skin::Base(BaseType::Atom("@".to_string()))),
        Box::new(Skin::Base(BaseType::Atom("@".to_string()))),
    );
    let subject = Hoon::Pair(Box::new(hoon_atom(7)), Box::new(hoon_atom(8)));
    let expr = set_subject(subject, Hoon::WutHax(skin, vec![Limb::Axis((1u64).into())]));

    let jam = compile_native_smoke(&expr).await;
    let got = eval_formula_atom(&jam);
    assert_eq!(got, 0);
}

#[tokio::test]
async fn compile_wthx_atom_and_cell_tests_on_noun_subject_are_dynamic_native() {
    let cases = [
        ("?#(@ a)", "|=  a=*\n  ?#(@ a)", 0, 1),
        ("?#([* *] a)", "|=  a=*\n  ?#([* *] a)", 1, 0),
        ("?#(^ a)", "|=  a=*\n  ?#(^ a)", 1, 0),
    ];

    for (name, src, expected_atom, expected_cell) in cases {
        let expr = parse_expr(src);
        let mut compiler = native_compiler().await;
        let mut compiled = compiler.compile_expr(&expr).expect("compile failed");
        let core_jam = eval_formula_jam(&compiled.jam());
        let dollar_axis = compiled
            .arm_map()
            .axis_for("$")
            .expect("gate $ arm axis missing");
        let dollar_axis = u64::try_from(dollar_axis).expect("test arm axis exceeds u64");

        let mut runner = ArmRunner::new().expect("runner init failed");
        let atom = runner
            .run_arm_jam(&core_jam, dollar_axis, &jam_atom(7))
            .expect("atom sample run failed");
        let cell = runner
            .run_arm_jam(&core_jam, dollar_axis, &jam_cell(7, 8))
            .expect("cell sample run failed");

        let atom_value = runner
            .result_atom_u64(&atom)
            .expect("atom result not atom or too large");
        let cell_value = runner
            .result_atom_u64(&cell)
            .expect("cell result not atom or too large");

        assert_eq!(atom_value, expected_atom, "atom sample for {name}");
        assert_eq!(cell_value, expected_cell, "cell sample for {name}");
    }
}

#[tokio::test]
async fn compile_wthx_flag_skin_parses_and_checks_dynamic_native() {
    let expr = parse_expr("|=  a=*\n  ?#(? a)");
    let mut compiler = native_compiler().await;
    let mut compiled = compiler.compile_expr(&expr).expect("compile failed");
    let core_jam = eval_formula_jam(&compiled.jam());
    let dollar_axis = compiled
        .arm_map()
        .axis_for("$")
        .expect("gate $ arm axis missing");
    let dollar_axis = u64::try_from(dollar_axis).expect("test arm axis exceeds u64");

    let mut runner = ArmRunner::new().expect("runner init failed");
    let cases = [
        ("false literal", jam_atom(0), 0),
        ("true literal", jam_atom(1), 0),
        ("non-flag atom", jam_atom(2), 1),
        ("cell", jam_cell(7, 8), 1),
    ];

    for (name, sample, expected) in cases {
        let result = runner
            .run_arm_jam(&core_jam, dollar_axis, &sample)
            .unwrap_or_else(|err| panic!("{name} sample run failed: {err}"));
        let value = runner
            .result_atom_u64(&result)
            .unwrap_or_else(|err| panic!("{name} result not atom or too large: {err}"));
        assert_eq!(value, expected, "{name} ?#(? a) result mismatch");
    }
}

#[tokio::test]
async fn compile_wing_unknown_term_on_noun_errors_in_strict_mode() {
    let expr = Hoon::Wing(vec![Limb::Term("x".to_string())]);
    let mut compiler = native_compiler().await;
    let compiled = compiler.compile_expr(&expr);
    assert!(
        compiled.is_err(),
        "strict mode should reject unresolved noun limb"
    );
}

#[tokio::test]
async fn compile_wing_parent_name_on_noun_errors_in_strict_mode() {
    let expr = Hoon::Wing(vec![Limb::Parent(1, Some("lth".to_string()))]);
    let mut compiler = native_compiler().await;
    let compiled = compiler.compile_expr(&expr);
    assert!(
        compiled.is_err(),
        "strict mode should reject unresolved parent-name wing",
    );
}

#[tokio::test]
async fn arm_map_axes_run_arms() {
    let expr = parse_expr("|%\n  ++  foo  1\n  ++  bar  2\n--\n");
    let mut compiler = native_compiler().await;
    let mut compiled = compiler.compile_expr(&expr).expect("compile failed");

    let foo_axis = compiled
        .arm_map()
        .axis_for("foo")
        .expect("foo axis missing");
    let foo_axis = u64::try_from(foo_axis).expect("test arm axis exceeds u64");
    let bar_axis = compiled
        .arm_map()
        .axis_for("bar")
        .expect("bar axis missing");
    let bar_axis = u64::try_from(bar_axis).expect("test arm axis exceeds u64");

    let formula_jam = compiled.jam();
    let core_jam = eval_formula_jam(&formula_jam);
    let sample_jam = jam_atom(0);

    let mut runner = ArmRunner::new().expect("runner init failed");
    let foo = runner
        .run_arm_jam(&core_jam, foo_axis, &sample_jam)
        .expect("foo run failed");
    let bar = runner
        .run_arm_jam(&core_jam, bar_axis, &sample_jam)
        .expect("bar run failed");

    let foo_val = runner
        .result_atom_u64(&foo)
        .expect("foo result not atom or too large");
    let bar_val = runner
        .result_atom_u64(&bar)
        .expect("bar result not atom or too large");

    assert_eq!(foo_val, 1);
    assert_eq!(bar_val, 2);
}

#[test]
fn wide_tuple_axes_do_not_wrap_at_the_u64_boundary() {
    std::thread::Builder::new()
        .name("wide-axis-regression".to_string())
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime")
                .block_on(async {
                    let mut jams = Vec::new();
                    let cases = [
                        (
                            62,
                            include_str!("../test-assets/axis-overflow/wide-tuple-62.hoon"),
                        ),
                        (
                            63,
                            include_str!("../test-assets/axis-overflow/wide-tuple-63.hoon"),
                        ),
                        (
                            64,
                            include_str!("../test-assets/axis-overflow/wide-tuple-64.hoon"),
                        ),
                    ];
                    for (slots, source) in cases {
                        let expr = parse_expr(source);
                        let mut compiler = native_compiler().await;
                        let mut compiled = compiler
                            .compile_expr(&expr)
                            .unwrap_or_else(|err| panic!("{slots}-slot tuple failed: {err}"));
                        jams.push(compiled.jam());
                    }
                    assert_ne!(jams[0], jams[1], "62 and 63 slots must not alias");
                    assert_ne!(jams[1], jams[2], "63 and 64 slots must not alias");
                });
        })
        .expect("spawn wide-axis regression")
        .join()
        .expect("wide-axis regression thread");
}

#[tokio::test]
async fn compile_dtkt_native_fast_repro() {
    let expr = Hoon::DotKet(
        Box::new(Spec::Base(BaseType::NounExpr)),
        Box::new(hoon_atom(1)),
    );
    let mut native = native_compiler().await;
    let mut compiled = native.compile_expr(&expr).expect("compile failed");

    assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("noun"));
    let desc = eval_formula_desc(&compiled.jam());
    assert_eq!(desc, "<interpret-error:Deterministic(Exit, 0)>");
}

#[tokio::test]
async fn compile_ket_variance_native_fast_repro() {
    let cases = [
        ("ketbar", Hoon::KetBar(Box::new(hoon_atom(7)))),
        ("ketpam", Hoon::KetPam(Box::new(hoon_atom(7)))),
        ("ketwut", Hoon::KetWut(Box::new(hoon_atom(7)))),
    ];
    for (name, expr) in cases {
        let mut native = native_compiler().await;
        let mut compiled = native.compile_expr(&expr).expect("compile failed");
        assert_eq!(
            compiled_type_tag(&compiled).as_deref(),
            Some("atom"),
            "unexpected type tag for {name}"
        );
        let value = eval_formula_atom(&compiled.jam());
        assert_eq!(value, 7, "unexpected evaluated value for {name}");
    }
}

#[tokio::test]
async fn compile_ketcol_native_fast_repro() {
    let expr = Hoon::KetCol(Box::new(Spec::Base(BaseType::NounExpr)));
    let mut native = native_compiler().await;
    let mut compiled = native.compile_expr(&expr).expect("compile failed");

    assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("core"));
    let desc = eval_formula_desc(&compiled.jam());
    assert!(
        desc.starts_with("[[8 "),
        "unexpected ketcol formula result shape: {desc}"
    );
    assert!(
        desc.ends_with("[0 0]]"),
        "unexpected ketcol formula result tail: {desc}"
    );
}

#[tokio::test]
async fn compile_siggar_native_fast_repro() {
    let expr = Hoon::SigGar(TermOrPair::Term("note".to_string()), Box::new(hoon_atom(7)));
    let mut native = native_compiler().await;
    let mut compiled = native.compile_expr(&expr).expect("compile failed");

    assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("atom"));
    let value = eval_formula_atom(&compiled.jam());
    assert_eq!(value, 7);
}

#[tokio::test]
async fn compile_sigzap_native_fast_repro() {
    let expr = Hoon::SigZap(Box::new(Hoon::Axis((1u64).into())), Box::new(hoon_atom(7)));
    let mut native = native_compiler().await;
    let mut compiled = native.compile_expr(&expr).expect("compile failed");

    assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("atom"));
    let value = eval_formula_atom(&compiled.jam());
    assert_eq!(value, 7);
}

#[tokio::test]
async fn compile_zpcom_native_fast_repro() {
    let expr = Hoon::ZapCom(Box::new(hoon_atom(1)), Box::new(hoon_atom(42)));
    let mut native = native_compiler().await;
    let mut compiled = native.compile_expr(&expr).expect("compile failed");

    assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("atom"));
    let desc = eval_formula_desc(&compiled.jam());
    assert_eq!(desc, "[1684955507 [25717 42]]");
}

#[tokio::test]
async fn compile_zpmc_native_fast_repro() {
    let expr = Hoon::ZapMic(Box::new(hoon_atom(1)), Box::new(hoon_atom(2)));
    let mut native = native_compiler().await;
    let mut compiled = native.compile_expr(&expr).expect("compile failed");

    assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("cell"));
    let desc = eval_formula_desc(&compiled.jam());
    assert_eq!(desc, "[[1836020833 [25717 0]] 2]");
}

#[tokio::test]
async fn compile_zpgl_native_fast_repro() {
    let prelude = prelude_core();
    let expr = with_prelude(
        &prelude,
        Hoon::ZapGal(
            Box::new(Spec::Base(BaseType::NounExpr)),
            Box::new(Hoon::ZapGar(Box::new(hoon_atom(5)))),
        ),
    );
    let mut native = native_compiler().await;
    let mut compiled = native.compile_expr(&expr).expect("compile failed");

    assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("noun"));
    let value = eval_formula_atom(&compiled.jam());
    assert_eq!(value, 5);
}

#[tokio::test]
async fn compile_zpts_native_fast_repro() {
    let expr = Hoon::ZapTis(Box::new(hoon_atom(7)));
    let mut native = native_compiler().await;
    let mut compiled = native.compile_expr(&expr).expect("compile failed");

    assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("noun"));
    let desc = eval_formula_desc(&compiled.jam());
    assert_eq!(desc, "[1 7]");
}

#[tokio::test]
async fn compile_hand_native_fast_repro() {
    let expr = Hoon::Hand(Box::new(Type::NounExpr), Nock::AxisSelect((1u64).into()));
    let mut native = native_compiler().await;
    let mut compiled = native.compile_expr(&expr).expect("compile failed");

    assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("noun"));
    let desc = eval_formula_desc(&compiled.jam());
    assert_eq!(desc, "<interpret-error:Deterministic(Exit, 0)>");
}

#[tokio::test]
async fn compile_opened_runes_native_fast_repro() {
    let ok_cases: Vec<(&str, Hoon)> = vec![
        ("base", Hoon::Base(BaseType::NounExpr)),
        ("leaf", Hoon::Leaf("foo".to_string(), ParsedAtom::Small(1))),
        ("bardot", Hoon::BarDot(Box::new(hoon_atom(3)))),
        (
            "sigcab",
            Hoon::SigCab(Box::new(hoon_atom(1)), Box::new(hoon_atom(2))),
        ),
        ("zapzap", Hoon::ZapZap),
    ];
    for (name, expr) in ok_cases {
        let mut native = native_compiler().await;
        let mut compiled = native.compile_expr(&expr).expect("native compile failed");
        match name {
            "base" => {
                assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("core"));
                let desc = eval_formula_desc(&compiled.jam());
                assert!(desc.ends_with("[0 0]]"), "unexpected base desc: {desc}");
            }
            "leaf" => {
                assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("core"));
                let desc = eval_formula_desc(&compiled.jam());
                assert!(desc.ends_with("[1 0]]"), "unexpected leaf desc: {desc}");
            }
            "bardot" => {
                assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("core"));
                let desc = eval_formula_desc(&compiled.jam());
                assert_eq!(desc, "[[1 3] 0]");
            }
            "sigcab" => {
                assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("atom"));
                let value = eval_formula_atom(&compiled.jam());
                assert_eq!(value, 2);
            }
            "zapzap" => {
                assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("void"));
                let desc = eval_formula_desc(&compiled.jam());
                assert_eq!(desc, "<interpret-error:Deterministic(Exit, 0)>");
            }
            _ => panic!("unexpected case {name}"),
        }
    }

    let err_cases: Vec<(&str, Hoon)> = vec![("ketdot", parse_expr("^.(|=(a=@ud @ud) 1)"))];
    for (name, expr) in err_cases {
        let mut native = native_compiler().await;
        let result = native.compile_expr(&expr);
        assert!(
            result.is_err(),
            "expected native compile to fail for {name}"
        );
    }

    let mut native = native_compiler().await;
    let sigpam_no_prelude = Hoon::SigPam(1, Box::new(hoon_atom(2)), Box::new(hoon_atom(3)));
    let result = native.compile_expr(&sigpam_no_prelude);
    assert!(
        result.is_err(),
        "strict mode should reject sigpam-no-prelude without lexical context",
    );
}

#[tokio::test]
async fn compile_prelude_runes_native_fast_repro() {
    let prelude = prelude_core();
    let cases: Vec<(&str, Hoon)> = vec![
        (
            "sigpam",
            with_prelude(
                &prelude,
                Hoon::SigPam(1, Box::new(hoon_atom(2)), Box::new(hoon_atom(3))),
            ),
        ),
        (
            "sigwut",
            with_prelude(
                &prelude,
                Hoon::SigWut(
                    1,
                    Box::new(hoon_bool(true)),
                    Box::new(hoon_atom(3)),
                    Box::new(hoon_atom(4)),
                ),
            ),
        ),
        (
            "zapgar",
            with_prelude(&prelude, Hoon::ZapGar(Box::new(hoon_atom(1)))),
        ),
    ];

    for (name, expr) in cases {
        let mut native = native_compiler().await;
        let mut compiled = native.compile_expr(&expr).expect("native compile failed");
        match name {
            "sigpam" => assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("atom")),
            "sigwut" => assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("atom")),
            // Normalized type should not wrap a singleton fork.
            "zapgar" => assert_eq!(compiled_type_tag(&compiled).as_deref(), Some("hold")),
            _ => panic!("unexpected case {name}"),
        }
        match name {
            "sigpam" => {
                let value = eval_formula_atom(&compiled.jam());
                assert_eq!(value, 3);
            }
            "sigwut" => {
                let value = eval_formula_atom(&compiled.jam());
                assert_eq!(value, 4);
            }
            "zapgar" => {
                let desc = eval_formula_desc(&compiled.jam());
                assert_eq!(desc, "[[1836020833 [25717 0]] 1]");
            }
            _ => panic!("unexpected case {name}"),
        }
    }
}

#[tokio::test]
async fn compile_lenient_runes_native() {
    let cases: Vec<(&str, Hoon)> = vec![("ketdot", parse_expr("^.(|=(a=@ud @ud) 1)"))];

    for (name, expr) in cases {
        let mut native = native_compiler().await;
        let result = native.compile_expr_with_vet(&expr, false);
        assert!(result.is_ok(), "lenient compile failed for {name}");
    }
}

#[tokio::test]
async fn compile_lost_errors_with_vet() {
    let expr = Hoon::Lost(Box::new(hoon_atom(1)));
    let mut native = native_compiler().await;
    assert!(
        native.compile_expr(&expr).is_err(),
        "expected lost to error with vet on"
    );
}

#[tokio::test]
async fn compile_lost_error_includes_dbug_location_metadata() {
    let expr = Hoon::Dbug(simple_spot(), Box::new(Hoon::Lost(Box::new(hoon_atom(1)))));
    let mut native = native_compiler().await;
    let err = match native.compile_expr(&expr) {
        Ok(_) => panic!("expected lost expression to fail"),
        Err(err) => err,
    };

    let metadata = err
        .metadata()
        .expect("expected structured metadata on decorated mint error");
    let location = metadata
        .location
        .as_ref()
        .expect("expected dbug-derived location metadata");
    assert_eq!(location.file.as_deref(), Some("test"));
    assert_eq!(location.start_line, Some(0));
    assert_eq!(location.start_col, Some(0));
    assert_eq!(location.end_line, Some(0));
    assert_eq!(location.end_col, Some(0));
}

/// Minimal reproduction of the kethep-before-barhep bug:
/// `^-` placed before `=+ c=0 |-` causes "find failed for wing [Term("c")]"
#[test]
fn compile_kethep_barhep_wing_c_native() {
    // This is the minimal repro from hoon-138's ++dvr arm
    let src = r#"|=  [a=@ b=@]
^-  [p=@ q=@]
=+  c=0
|-
?:  =(a b)  [c a]
$(c +(c))"#;
    let linemap = Arc::new(LineMap::new(src));
    let expr = native_parser(vec!["test".to_string()], parser_dbug_enabled(), linemap)
        .parse(src)
        .into_result()
        .expect("parse failed");

    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    let result = ut.mint_noun(sut, gol, &expr);
    assert!(
        result.is_ok(),
        "kethep-before-barhep should compile: {:?}",
        result.err()
    );
}

/// Minimal repro from hoon-138 list-logic style traps:
/// `|-` with a `^+ a` goal should be able to resolve wing `a` from the outer sample.
#[test]
fn compile_barhep_ktsl_wing_a_native() {
    let src = r#"|=  a=@
|-
^+  a
a"#;
    let linemap = Arc::new(LineMap::new(src));
    let expr = native_parser(vec!["test".to_string()], parser_dbug_enabled(), linemap)
        .parse(src)
        .into_result()
        .expect("parse failed");

    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    ut.set_vet(false);
    let result = ut.mint_noun(sut, gol, &expr);
    assert!(
        result.is_ok(),
        "`|- ^+ a` should compile: {:?}",
        result.err()
    );
}

/// Same as `compile_barhep_ktsl_wing_a_native`, but using a wet gate (`|*`) which is how hoon-138
/// list logic is written.
#[test]
fn compile_bartar_barhep_ktsl_wing_a_native() {
    let src = r#"|*  a=@
|-
^+  a
a"#;
    let linemap = Arc::new(LineMap::new(src));
    let expr = native_parser(vec!["test".to_string()], parser_dbug_enabled(), linemap)
        .parse(src)
        .into_result()
        .expect("parse failed");

    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    ut.set_vet(false);
    let result = ut.mint_noun(sut, gol, &expr);
    assert!(
        result.is_ok(),
        "`|* ... |- ^+ a` should compile: {:?}",
        result.err()
    );
}

/// Minimal hoon-138 style reproduction: `++flop` uses `|*`, `=> .(a (homo a))`, `^+ a`, and a `|-`
/// trap that references `a` (and `t.a` / `i.a`).
///
/// Currently, hoon-138 list-logic fails in `++flop` with:
/// `native mint: find failed for wing [Term("a")]`.
#[test]
fn compile_list_flop_trap_wing_a_native() {
    let src = r#"|%
++  list
  |$  [item]
  $@(~ [i=item t=(list item)])
++  homo
  |*  a=(list)
  a
++  flop
  |*  a=(list)
  =>  .(a (homo a))
  ^+  a
  =+  b=`_a`~
  |-
  ?~  a  b
  $(a t.a, b [i.a b])
--
"#;
    let linemap = Arc::new(LineMap::new(src));
    let expr = native_parser(vec!["test".to_string()], parser_dbug_enabled(), linemap)
        .parse(src)
        .into_result()
        .expect("parse failed");

    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    ut.set_vet(false);
    let result = ut.mint_noun(sut, gol, &expr);
    assert!(
        result.is_ok(),
        "list flop trap should compile: {:?}",
        result.err()
    );
}

#[test]
fn compile_chained_layers_fl_rou_rau_vet_true_native() {
    // Hoon-138 defines `++fn` in an earlier core layer than `++fl`.  Ensure strict mode can:
    // - resolve `fn` via the parent chain
    // - compile `++fl`'s nested `=> ~% ... |%` structure
    // - compile `++rou` calling `++rau` where `++rau` uses `a.a`
    let expr = parse_expr(
        r#"=>
  |%
  ++  fn
    $%  [%f s=? e=@s a=@u]
        [%i s=?]
        [%n ~]
    ==
  --
  |%
  ++  fl
    =/  [[p=@u v=@s w=@u] r=$?(%n %u %d %z %a) d=$?(%d %f %i)]
      [[1 --0 0] %n %d]
    =>
      ~%  %cofl  +>  ~
      |%
      ++  rou
        |=  [a=[e=@s a=@u]]  ^-  fn
        (rau a &)
      ++  rau
        |=  [a=[e=@s a=@u] t=?]  ^-  fn
        ?-  r
          %n  [%f & e.a a.a]
          %u  [%f & e.a a.a]
          %d  [%f & e.a a.a]
          %z  [%f & e.a a.a]
          %a  [%f & e.a a.a]
        ==
      --
    ~
  --"#,
    );

    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    let result = ut.mint_noun(sut, gol, &expr);
    assert!(
        result.is_ok(),
        "vet=true should compile chained layers fl/rou/rau: {:?}",
        result.err()
    );
}

/// Match hoonc for a strict-mode sample that looks close to hoon-138 `++fl`, but is invalid:
/// after `?~  a.a`, the true branch refines `%f.a` to `%~`, which does not nest under the
/// declared `@u` field in `fn`.
#[test]
fn compile_gate_sample_null_refinement_rejected_vet_true_native() {
    let expr = parse_expr(
        r#"|%
++  fn
  $%  [%f s=? e=@s a=@u]
      [%i s=?]
      [%n ~]
  ==
++  inner
  |%
  ++  rou
    |=  [a=fn]  ^-  fn
    ?.  ?=([%f *] a)  a
    ?~  a.a  a
    a
  --
--"#,
    );

    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    let result = ut.mint_noun(sut, gol, &expr);
    let err = result.expect_err("hoonc rejects this strict-mode sample, so native should too");
    let err_text = format!("{err:?}");
    assert!(
        err_text.contains("mint-nice"),
        "expected native rejection to fail at mint-nice like hoonc nest-fail, got: {err_text}"
    );
}

/// Regression test for hoon-138 `++year` failure in strict mode:
/// `date` refers to `tarp` before `tarp` is defined, and `++year` then accesses `d.t.det`.
#[test]
fn compile_year_wing_d_from_forward_ref_tarp_in_date_vet_true_native() {
    let expr = parse_expr(
        r#"|%
+$  date  [[a=? y=@ud] m=@ud t=tarp]
+$  tarp  [d=@ud h=@ud m=@ud s=@ud f=@ux]
++  year
  |=  det=date
  ^-  @ud
  d.t.det
--"#,
    );
    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    let result = ut.mint_noun(sut, gol, &expr);
    assert!(
        result.is_ok(),
        "vet=true should compile `++year` accessing `d.t.det` through forward-referenced molds: {:?}",
        result.err()
    );
}

/// Layer-3 fast repro: bind `rip=(yell now)` where `++yell` contains a local `|-` trap,
/// then resolve `d.rip` in `++yore`.
#[test]
fn compile_yore_yell_trap_result_wing_d_vet_true_native() {
    let expr = parse_expr(
        r#"|%
+$  tarp  [d=@ud h=@ud m=@ud s=@ud f=@ux]
++  yell
  |=  now=@da
  ^-  tarp
  =+  ^=  fan
      =+  [muc=0 raw=0x0]
      |-  ^-  @ux
      ?:  =(4 muc)
        raw
      =>  .(muc +(muc))
      $(raw +(raw))
  [1 2 3 4 fan]
++  yall
  |=  day=@ud
  [y=day m=1 d=1]
++  yore
  |=  now=@da
  =+  rip=(yell now)
  =+  ger=(yall d.rip)
  y.ger
--"#,
    );

    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    let result = ut.mint_noun(sut, gol, &expr);
    assert!(
        result.is_ok(),
        "vet=true should compile `d.rip` from `(yell now)` with trap recursion: {:?}",
        result.err()
    );
}

/// Ensure `vet=true` preserves unit faces through `?~` narrowing in the simplest case.
/// If this fails, the bug is not specific to `%~` / `~(get ...)` door-calls.
#[test]
fn compile_wing_u_after_wutsig_unit_gate_vet_true_native() {
    let src = r#"=>
|%
++  unit
  |$  [item]
  $@(~ [~ u=item])
--
|=  a=(unit @)
?~  a  0
u.a
"#;
    let linemap = Arc::new(LineMap::new(src));
    let expr = native_parser(vec!["test".to_string()], parser_dbug_enabled(), linemap)
        .parse(src)
        .into_result()
        .expect("parse failed");

    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    let result = ut.mint_noun(sut, gol, &expr);
    assert!(
        result.is_ok(),
        "vet=true should compile basic unit `?~` narrowing: {:?}",
        result.err()
    );
}

/// `%set-logic`-style `++dif` recursion shape with local `d`/`e` bindings and `n.d`/`r.d` access.
/// Minimal repro: |- with $ recursion
#[test]
fn compile_barhep_dollar_recursion_minimal() {
    let expr = parse_expr(
        r#"|=  a=@
|-
?:  =(a 0)
  a
$(a 1)"#,
    );
    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    ut.set_vet(false);
    let result = ut.mint_noun(sut, gol, &expr);
    assert!(
        result.is_ok(),
        "minimal |- with $ recursion should compile: {:?}",
        result.err()
    );
}

/// This is intended as a fast repro for wing-resolution regressions in the `in/dif` path.
#[test]
fn compile_set_logic_dif_local_d_recursion_vet_false_native() {
    let expr = parse_expr(
        r#"|%
+$  tree  $@(~ [n=* l=tree r=tree])
++  mor
  |=  [a=* b=*]
  |
++  in
  =|  a=tree
  |@
  ++  bif
    |*  b=*
    ^+  [l=a r=a]
    [b ~ ~]
  ++  dif
    ~/  %dif
    |*  b=_a
    |-  ^+  a
    ?~  b
      a
    =+  c=(bif n.b)
    ?>  ?=(^ c)
    =+  d=$(a l.c, b l.b)
    =+  e=$(a r.c, b r.b)
    |-  ^-  [$?(~ _a)]
    ?~  d  e
    ?~  e  d
    ?:  (mor n.d n.e)
      d(r $(d r.d))
    e(l $(e l.e))
  --
--"#,
    );

    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    ut.set_vet(false);
    let result = ut.mint_noun(sut, gol, &expr);
    assert!(
        result.is_ok(),
        "vet=false should compile dif-style local d recursion: {:?}",
        result.err()
    );
}

/// Same as `compile_wing_u_after_wutsig_tisbar_unit_make_vet_true_native`, but with a recursive
/// `list` mold to match hoon-138-style `unit (list @)` samples.
#[test]
fn compile_wing_u_after_wutsig_tisbar_unit_list_make_vet_true_native() {
    let src = r#"=>
|%
++  list
  |$  [item]
  $@(~ [i=item t=(list item)])
++  unit
  |$  [item]
  $@(~ [~ u=item])
--
=|  a=(unit (list @))
?~(a ~ u.a)
"#;
    let linemap = Arc::new(LineMap::new(src));
    let expr = native_parser(vec!["test".to_string()], parser_dbug_enabled(), linemap)
        .parse(src)
        .into_result()
        .expect("parse failed");

    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    let result = ut.mint_noun(sut, gol, &expr);
    assert!(
        result.is_ok(),
        "vet=true should compile `=| a=(unit (list @))` then `?~(a ~ u.a)`: {:?}",
        result.err()
    );
}

/// Like `compile_wing_u_after_wutsig_via_tilde_get_vet_true_native`, but with an outer subject that
/// does *not* itself contain a `u` face. This catches cases where we accidentally type the result
/// of `~(get ...)` as the gate core rather than the *unit* returned by slamming it.
#[test]
fn compile_wing_u_after_wutsig_via_tilde_get_outer_subject_has_no_u_vet_true_native() {
    let src = r#"|%
++  unit
  |$  [item]
  $@(~ [~ u=item])
++  by
  =|  a=@
  |@
  ++  get
    |*  b=@
    [~ u=b]
  --
++  jar
  =|  a=@
  |@
  ++  get
    |*  b=@
    =+  c=(~(get by a) b)
    ?~(c ~ u.c)
  --
--
"#;
    let linemap = Arc::new(LineMap::new(src));
    let expr = native_parser(vec!["test".to_string()], parser_dbug_enabled(), linemap)
        .parse(src)
        .into_result()
        .expect("parse failed");

    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    let result = ut.mint_noun(sut, gol, &expr);
    assert!(
        result.is_ok(),
        "vet=true should compile `?~(c ~ u.c)` when `c` comes from `~(get ...)` and outer subject has no `u`: {:?}",
        result.err()
    );
}

// =========================================================================
// Targeted strict semantic parity tests
//
// These are source-Hoon, parse-valid samples chosen to exercise the strict
// rejection/acceptance surfaces where honk can drift from canonical ++mint,
// ++fire, and ++mull behavior.  They intentionally do not run hoonc; each case
// was chosen as a stable semantic oracle and the automated test only runs honk.
// The helper calls ++mint rather than ++play because ++play disables vet recursively.
// =========================================================================

fn mint_source_with_vet(src: &str, vet: bool) -> Result<(), String> {
    let expr = parse_expr(src);
    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    ut.set_vet(vet);
    ut.mint_noun(sut, gol, &expr)
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

fn assert_same_mint_acceptance(name: &str, left: &str, right: &str, vet: bool) {
    let left_result = mint_source_with_vet(left, vet);
    let right_result = mint_source_with_vet(right, vet);
    assert_eq!(
        left_result.is_ok(),
        right_result.is_ok(),
        "metamorphic acceptance mismatch for {name} with vet={vet}\nleft:\n{left}\nleft result: {left_result:?}\nright:\n{right}\nright result: {right_result:?}"
    );
}

fn assert_mint_acceptance_differs(name: &str, left: &str, right: &str, vet: bool) {
    let left_result = mint_source_with_vet(left, vet);
    let right_result = mint_source_with_vet(right, vet);
    assert_ne!(
        left_result.is_ok(),
        right_result.is_ok(),
        "metamorphic negative control was not sensitive for {name} with vet={vet}\nleft:\n{left}\nleft result: {left_result:?}\nright:\n{right}\nright result: {right_result:?}"
    );
}

fn assert_mint_acceptance_differs_by_vet(name: &str, src: &str) {
    let strict = mint_source_with_vet(src, true);
    let relaxed = mint_source_with_vet(src, false);
    assert_ne!(
        strict.is_ok(),
        relaxed.is_ok(),
        "vet-mode negative control was not sensitive for {name}\nsource:\n{src}\nstrict result: {strict:?}\nrelaxed result: {relaxed:?}"
    );
}

fn wrap_debug_hint(src: &str) -> String {
    format!("~_  leaf+\"mt\"\n{src}")
}

fn wrap_identity_subject(src: &str) -> String {
    format!("=>\n.\n{src}")
}

fn metamorphic_seed_sources() -> Vec<(&'static str, String)> {
    let mut cases = vec![
        ("pair_literal", ":-  1  2".to_string()),
        ("atom_gate", "|=  a=@\na".to_string()),
        ("strict_cast_positive", "|=  a=@\n^-  @\na".to_string()),
        ("strict_cast_negative", "|=  a=@\n^-  @\n[a a]".to_string()),
        (
            "wtts_atom_branch_positive",
            "|=  a=*\n?:  ?=(@ a)\n  a\n0".to_string(),
        ),
        (
            "wtts_atom_branch_negative",
            "|=  a=*\n?:  ?=(@ a)\n  +.a\n0".to_string(),
        ),
        (
            "wthx_named_pair_positive",
            "=>\n|%\n+$  pair  [@ @]\n--\n|=  a=pair\n?#(pair a)".to_string(),
        ),
    ];

    // Finite property-style generator over simple molds.  Each generated source has the
    // same semantic shape; the cross-product exercises parser lowering, ++play, ++mint,
    // and ++nice with different mold structures without invoking hoonc in steady-state tests.
    for (idx, spec) in ["@", "[@ @]", "?", "(unit @)", "$?(~ [@ @])"]
        .into_iter()
        .enumerate()
    {
        cases.push(("generated_gate_pair", format!("|=  a={spec}\n^-  *\n[a a]")));
        cases.push(("generated_wtts_self", format!("|=  a={spec}\n?=({spec} a)")));
        cases.push((
            "generated_identity_subject",
            format!("=>\n|%\n+$  mold{idx}  {spec}\n--\n|=  a=mold{idx}\na"),
        ));
    }

    cases
}

#[test]
fn metamorphic_nonsemantic_wrappers_preserve_acceptance() {
    for (name, src) in metamorphic_seed_sources() {
        let debug_wrapped = wrap_debug_hint(&src);
        let identity_wrapped = wrap_identity_subject(&src);
        let composed = wrap_debug_hint(&identity_wrapped);
        for vet in [true, false] {
            assert_same_mint_acceptance(&format!("{name}/debug"), &src, &debug_wrapped, vet);
            assert_same_mint_acceptance(
                &format!("{name}/identity-subject"),
                &src,
                &identity_wrapped,
                vet,
            );
            assert_same_mint_acceptance(
                &format!("{name}/debug-after-identity"),
                &src,
                &composed,
                vet,
            );
        }
    }
}

#[test]
fn metamorphic_strict_success_implies_vetless_success() {
    for (name, src) in metamorphic_seed_sources() {
        if mint_source_with_vet(&src, true).is_ok() {
            let relaxed = mint_source_with_vet(&src, false);
            assert!(
                relaxed.is_ok(),
                "strict compile success must imply vet=false success for {name}: {relaxed:?}\n{src}"
            );
        }
    }
}

#[test]
fn metamorphic_branch_swap_preserves_acceptance() {
    let cases = [
        ("atom_positive", "?=(@ a)", "a", "0"),
        ("atom_negative", "?=(@ a)", "+.a", "0"),
        ("cell_positive", "?=([@ @] a)", "+.a", "0"),
        ("cell_negative", "?=([@ @] a)", "a", "+.a"),
        (
            "atom_cell_partition_casts", "?=(@ a)", "^-  @\n  a", "^-  [* *]\na",
        ),
        ("null_unit", "?=(~ a)", "0", "0"),
        ("pair_or_null", "?=($?(~ [@ @]) a)", "0", "0"),
    ];

    for (name, condition, yes, no) in cases {
        let normal = format!("|=  a=*\n?:  {condition}\n  {yes}\n{no}");
        let swapped = format!("|=  a=*\n?.  {condition}\n  {no}\n{yes}");
        assert_same_mint_acceptance(name, &normal, &swapped, true);
    }
}

#[test]
fn metamorphic_wut_sugar_matches_explicit_type_tests() {
    let cases = [
        (
            "wutpat_atom_positive", "|=  a=*\n?@  a\n  a\n0", "|=  a=*\n?:  ?=(@ a)\n  a\n0",
        ),
        (
            "wutpat_atom_negative", "|=  a=*\n?@  a\n  +.a\n0", "|=  a=*\n?:  ?=(@ a)\n  +.a\n0",
        ),
        (
            "wutpat_atom_cell_partition_casts", "|=  a=*\n?@  a\n  ^-  @\n  a\n^-  [* *]\na",
            "|=  a=*\n?:  ?=(@ a)\n  ^-  @\n  a\n^-  [* *]\na",
        ),
        (
            "wutsig_unit_positive", "|=  a=$?(~ [~ u=@])\n?~  a\n  0\nu.a",
            "|=  a=$?(~ [~ u=@])\n?:  ?=(~ a)\n  0\nu.a",
        ),
        (
            "wutsig_unit_negative", "|=  a=$?(~ [~ u=@])\n?~  a\n  u.a\n0",
            "|=  a=$?(~ [~ u=@])\n?:  ?=(~ a)\n  u.a\n0",
        ),
    ];

    for (name, sugar, explicit) in cases {
        assert_same_mint_acceptance(name, sugar, explicit, true);
    }
}

#[test]
fn metamorphic_negative_controls_are_sensitive() {
    let valid_cast = "|=  a=@\n^-  @\na";
    let invalid_cast_mutant = "|=  a=@\n^-  @\n[a a]";
    assert_mint_acceptance_differs("goal-cast-mutant", valid_cast, invalid_cast_mutant, true);

    let valid_name = "=>\n|%\n+$  mycell  [@ @]\n--\n|=  a=mycell\n?=(mycell a)";
    let invalid_name_mutant = "=>\n|%\n+$  mycell  [@ @]\n--\n|=  a=mycell\n?=(othercell a)";
    assert_mint_acceptance_differs("renaming-mutant", valid_name, invalid_name_mutant, true);
}

#[test]
fn metamorphic_mutation_controls_cover_broken_transformations() {
    // A `?.` implementation that forgets to swap the arms should be rejected by
    // the branch-swap MR: each branch depends on the matching atom/cell narrowing.
    let normal_branch = "|=  a=*\n?:  ?=(@ a)\n  ^-  @\n  a\n^-  [* *]\na";
    let unswapped_wutdot_mutant = "|=  a=*\n?.  ?=(@ a)\n  ^-  @\n  a\n^-  [* *]\na";
    assert_mint_acceptance_differs(
        "branch-swap-without-arm-swap", normal_branch, unswapped_wutdot_mutant, true,
    );

    // A `?@` lowering to a cell test instead of an atom test should similarly
    // put the atom and cell casts under the wrong refinements.
    let wutpat = "|=  a=*\n?@  a\n  ^-  @\n  a\n^-  [* *]\na";
    let cell_test_mutant = "|=  a=*\n?:  ?=([* *] a)\n  ^-  @\n  a\n^-  [* *]\na";
    assert_mint_acceptance_differs("wutpat-cell-test-mutant", wutpat, cell_test_mutant, true);

    // A `?~` lowering to a generic cell test accepts the unit case but misses
    // the null case, where `u.a` must remain unavailable.
    let wutsig = "|=  a=$?(~ [~ u=@])\n?~  a\n  0\nu.a";
    let generic_cell_mutant = "|=  a=$?(~ [~ u=@])\n?:  ?=([* *] a)\n  0\nu.a";
    assert_mint_acceptance_differs(
        "wutsig-generic-cell-mutant", wutsig, generic_cell_mutant, true,
    );

    // The vet monotonicity MR is meaningful only if the corpus contains cases
    // that strict mode rejects and relaxed mode accepts.
    assert_mint_acceptance_differs_by_vet(
        "strict-goal-cast-rejected-relaxed-accepted", "|=  a=@\n^-  @\n[a a]",
    );
}

#[test]
fn metamorphic_consistent_renaming_preserves_acceptance() {
    let templates = [
        "|=  SAMPLE=MOLD\n?=(MOLD SAMPLE)", "|=  SAMPLE=MOLD\n?#(MOLD SAMPLE)",
        "|=  SAMPLE=*\n?:  ?=(MOLD SAMPLE)\n  +.SAMPLE\n0",
    ];
    let renamings = [("pair", "a"), ("duo", "b"), ("cellar", "c")];

    for template in templates {
        let base = format!(
            "=>\n|%\n+$  pair  [@ @]\n--\n{}",
            template.replace("MOLD", "pair").replace("SAMPLE", "a")
        );
        for (mold, sample) in renamings {
            let renamed = format!(
                "=>\n|%\n+$  {mold}  [@ @]\n--\n{}",
                template.replace("MOLD", mold).replace("SAMPLE", sample)
            );
            assert_same_mint_acceptance(
                &format!("rename/{mold}/{sample}/{template}"),
                &base,
                &renamed,
                true,
            );
        }
    }
}

#[test]
fn invalid_sand_flag_and_null_literals_reject() {
    let cases = [
        (
            "flag",
            Hoon::Sand("f".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(2))),
            "sand-flag",
        ),
        (
            "null",
            Hoon::Sand("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(1))),
            "sand-null",
        ),
    ];

    for (name, expr, expected) in cases {
        let mut slab = nockapp::noun::slab::NounSlab::new();
        let sut = honk::native::ut::ty_noun(&mut slab);
        let gol = honk::native::ut::ty_noun(&mut slab);
        let mut ut = honk::native::ut::Ut::new(&mut slab);
        let err = ut
            .mint_noun(sut, gol, &expr)
            .expect_err("invalid %sand literal should reject");
        let err = format!("{err:?}");
        assert!(
            err.contains(expected),
            "invalid %sand {name} should contain {expected:?}, got: {err}"
        );
    }
}

#[test]
fn zpts_inner_error_restores_strict_vet_for_later_mint() {
    let mut slab = nockapp::noun::slab::NounSlab::new();
    let sut = honk::native::ut::ty_noun(&mut slab);
    let gol = honk::native::ut::ty_noun(&mut slab);
    let mut ut = honk::native::ut::Ut::new(&mut slab);
    ut.set_vet(true);

    let zpts_with_error = parse_expr(
        r#"=+  a=[p=1 q=2]
!=  r.a"#,
    );
    let err = ut
        .mint_noun(sut, gol, &zpts_with_error)
        .expect_err("!= inner compile error should reject");
    let err = format!("{err:?}");
    assert!(
        err.contains("find failed"),
        "unexpected != inner error: {err}"
    );

    let strict_rejection = parse_expr(
        r#"|=  a=@
^-  @
[a a]"#,
    );
    let err = ut
        .mint_noun(sut, gol, &strict_rejection)
        .expect_err("vet should still be true after failed != mint");
    let err = format!("{err:?}");
    assert!(
        err.contains("mint-nice"),
        "vet leaked off after failed != mint; got: {err}"
    );
}

#[test]
fn strict_source_positive_semantic_parity_cases_compile() {
    let cases = [
        (
            "goal-cast-atom", r#"|=  a=@
^-  @
a"#,
        ),
        (
            "branch-goal-both-arms-atom", r#"|=  a=?
^-  @
?:  a
  1
2"#,
        ),
        (
            "dry-gate-fire-with-compatible-sample", r#"|=  x=@
=+  ^=  g
  |=  a=@
  a
(g 1)"#,
        ),
        (
            "wuttis-narrows-fork-arms",
            r#"|=  a=$?([%foo b=@] [%bar c=@])
^-  @
?:  ?=([%foo *] a)
  b.a
c.a"#,
        ),
        (
            "wuthep-switch-branches-nest", r#"|=  a=$?(%foo %bar)
^-  @
?-  a
  %foo  1
  %bar  2
=="#,
        ),
        (
            "unit-hold-wutsig-exposes-u-in-non-null-branch",
            r#"=>
|%
++  unit
  |$  [item]
  $@(~ [~ u=item])
--
|=  a=(unit @)
^-  @
?~  a
  0
u.a"#,
        ),
        (
            "recursive-hold-rest-exposes-list-head-after-null-test",
            r#"|%
+$  list  $@(~ [i=@ t=list])
++  head
  |=  a=list
  ?~  a  0
  i.a
--"#,
        ),
        (
            "projection-through-faced-sample", r#"|=  a=[p=@ q=@]
q.a"#,
        ),
        (
            "cnts-update-existing-face", r#"|=  a=[p=@ q=@]
a(q 3)"#,
        ),
        (
            "wutgar-assertion-narrows-continuation",
            r#"|=  a=$?(%foo [p=%bar q=@])
^-  @
?>  ?=([%bar *] a)
q.a"#,
        ),
        (
            "wthx-core-generic-cell-tail-preserves-arms",
            r#"|=  x=@
=+  ^=  a
  =>  7
  |%
  ++  foo  1
  --
^-  @
?:  ?#([@ *] a)
  foo.a
!!"#,
        ),
        (
            "core-goal-fork-same-chapter-count-compiles",
            r#"|=  flag=?
^+  ?:  flag
      |%
      ++  foo  1
      --
    |%
    ++  bar  2
    --
|%
++  foo  1
--"#,
        ),
        (
            "recursive-list-indexes-feed-fixed-list-gate",
            r#"|%
+$  list  $@(~ [i=@ t=list])
++  g
  |=  [a=@ b=@ ~]
  a
++  test
  |=  [y=list]
  (g ~[&1.y &2.y])
--"#,
        ),
        (
            "wthx-term-skin-resolves-through-spec-path",
            r#"=>
|%
+$  pair  [@ @]
--
|=  a=pair
?#(pair a)"#,
        ),
        (
            "wet-arm-fired-with-atom-and-cell-samples",
            r#"|%
++  id
  |*  a=*
  a
++  use-atom
  (id 1)
++  use-cell
  (id [2 3])
--"#,
        ),
    ];

    for (name, src) in cases {
        let result = mint_source_with_vet(src, true);
        assert!(
            result.is_ok(),
            "strict semantic positive case {name} should compile: {:?}",
            result.err()
        );
    }
}

#[test]
fn strict_source_negative_semantic_parity_cases_reject() {
    let cases = [
        (
            "goal-cast-cell-as-atom", r#"|=  a=@
^-  @
[a a]"#, "mint-nice",
        ),
        (
            "branch-goal-cell-arm", r#"|=  a=?
^-  @
?:  a
  1
[2 3]"#, "mint-nice",
        ),
        (
            "dry-gate-fire-with-cell-sample", r#"|=  x=@
=+  ^=  g
  |=  a=@
  a
(g [1 2])"#,
            "fire-dry",
        ),
        (
            "dry-gate-fire-with-void-sample", r#"|=  x=@
=+  ^=  g
  |=  a=@
  a
(g !!)"#, "fire-core",
        ),
        (
            "wuttis-true-branch-loses-other-variant",
            r#"|=  a=$?([%foo b=@] [%bar c=@])
^-  @
?:  ?=([%foo *] a)
  c.a
b.a"#, "find failed",
        ),
        (
            "wuthep-switch-branch-goal-mismatch",
            r#"|=  a=$?(%foo %bar)
^-  @
?-  a
  %foo  1
  %bar  [2 3]
=="#, "mint-nice",
        ),
        (
            "unit-null-branch-does-not-have-u",
            r#"=>
|%
++  unit
  |$  [item]
  $@(~ [~ u=item])
--
|=  a=(unit @)
^-  @
?~  a
  u.a
0"#,
            "find failed",
        ),
        (
            "recursive-hold-null-branch-does-not-have-i",
            r#"|%
+$  list  $@(~ [i=@ t=list])
++  bad
  |=  a=list
  ?~  a  i.a
  0
--"#, "find failed",
        ),
        (
            "projection-missing-face", r#"|=  a=[p=@ q=@]
r.a"#, "find failed",
        ),
        (
            "cnts-update-missing-face", r#"|=  a=[p=@ q=@]
a(r 3)"#, "find failed",
        ),
        (
            "wutgar-assertion-crops-away-cell-fields",
            r#"|=  a=$?(%foo [p=%bar q=@])
^-  @
?>  ?=(%foo a)
q.a"#, "find failed",
        ),
        (
            "wthx-core-cell-gain-drops-arms",
            r#"|=  x=@
=+  ^=  a
  =>  7
  |%
  ++  foo  1
  --
^-  @
?:  ?#([@ @] a)
  foo.a
!!"#,
            "find failed",
        ),
        (
            "wthx-core-cell-lose-drops-arms",
            r#"|=  x=*
=+  ^=  a
  =>  x
  |%
  ++  foo  1
  --
^-  @
?:  ?#([@ @] a)
  !!
foo.a"#,
            "find failed",
        ),
        (
            "wet-mull-nice-rejects-generic-cell-as-atom",
            r#"|%
++  bad
  |*  a=*
  ^-  @
  [a a]
++  use
  (bad 1)
--"#, "mull-nice",
        ),
        (
            "core-goal-fork-rejects-non-core-options",
            r#"|=  flag=?
^+  ?:  flag
      |%
      ++  foo  1
      --
    7
|%
++  foo  1
--"#,
            "core-nice",
        ),
        (
            "core-goal-fork-requires-all-chapter-counts",
            r#"|=  flag=?
^+  ?:  flag
      |%
      ++  foo  1
      --
    |%
    ++  bar  2
    +|  %two
    ++  baz  3
    --
|%
++  foo  1
--"#,
            "core-number-of-chapters",
        ),
    ];

    for (name, src, expected) in cases {
        let err = mint_source_with_vet(src, true)
            .expect_err("strict semantic negative case should reject");
        assert!(
            err.contains(expected),
            "strict semantic negative case {name} should contain {expected:?}, got: {err}"
        );
    }
}

#[test]
fn strict_source_vet_only_rejections_compile_without_vet() {
    let cases = [
        (
            "goal-cast-cell-as-atom", r#"|=  a=@
^-  @
[a a]"#,
        ),
        (
            "branch-goal-cell-arm", r#"|=  a=?
^-  @
?:  a
  1
[2 3]"#,
        ),
        (
            "dry-gate-fire-with-cell-sample", r#"|=  x=@
=+  ^=  g
  |=  a=@
  a
(g [1 2])"#,
        ),
        (
            "wuthep-switch-branch-goal-mismatch",
            r#"|=  a=$?(%foo %bar)
^-  @
?-  a
  %foo  1
  %bar  [2 3]
=="#,
        ),
        (
            "wet-mull-nice-rejects-generic-cell-as-atom",
            r#"|%
++  bad
  |*  a=*
  ^-  @
  [a a]
++  use
  (bad 1)
--"#,
        ),
    ];

    for (name, src) in cases {
        let result = mint_source_with_vet(src, false);
        assert!(
            result.is_ok(),
            "case {name} should only be rejected by strict vet checks: {:?}",
            result.err()
        );
    }
}

// =========================================================================
// Wet polymorphism (++mull) tests
//
// These test the complete ++mull implementation. Tests are structured as:
// 1. Unit tests: verify mull succeeds/fails for specific wet gate patterns
// 2. Oracle parity tests: compare native compiler output against pre-generated
//    Bazel artifacts for wet polymorphism hoon files
// =========================================================================

// --- Unit tests: basic wet gate compilation with vet=true ---

/// Helper: parse and compile with vet=true (mull is active for wet arms)
fn compile_wet_with_vet(src: &str) -> Result<(), String> {
    mint_source_with_vet(src, true)
}

/// Helper: parse and compile with vet=false
fn compile_wet_without_vet(src: &str) -> Result<(), String> {
    mint_source_with_vet(src, false)
}

#[test]
fn mull_wet_representative_cases_compile() {
    let cases = [
        ("identity", "|*  a=*\na"),
        ("pair", "|*  a=*\n[a a]"),
        ("cast", "|*  a=@\n^+  a\na"),
        ("trap", "|*  a=@\n|-\n^+  a\na"),
        (
            "wet-core-inside-dry", "|%\n++  identity\n  |*  a=*\n  a\n--",
        ),
        (
            "list-homo-pattern",
            r#"|%
++  list
  |$  [item]
  $@(~ [i=item t=(list item)])
++  homo
  |*  a=(list)
  a
--"#,
        ),
        (
            "unit-biff-pattern",
            r#"|%
++  unit
  |$  [item]
  $@(~ [~ u=item])
++  biff
  |*  [a=(unit) b=$-(* (unit))]
  ?~  a  ~
  (b u.a)
--"#,
        ),
    ];

    for (name, src) in cases {
        let result = compile_wet_with_vet(src);
        assert!(
            result.is_ok(),
            "wet representative {name} should compile: {:?}",
            result.err()
        );
    }
}

#[test]
fn mull_wet_vet_off_compiles() {
    let result = compile_wet_without_vet("|*  a=*\na");
    assert!(
        result.is_ok(),
        "wet with vet=false should compile: {:?}",
        result.err()
    );
}
