use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use lazy_static::lazy_static;
use nockvm::ext::{IndirectAtomExt, NounExt};
use nockvm::mem::{NockStack, NOCK_STACK_SIZE_TINY};
use nockvm::noun::{AllocLocation, Cell, IndirectAtom, Noun, D};
use nockvm::pma::{Pma, PmaCopy};
use nockvm::serialization::cue_into_stack_pointer_form;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use tempfile::TempDir;

const PMA_WORDS: usize = 1 << 25; // 256 MiB PMA arena for synthetic cases
const LEAF_COUNT: usize = 5_120;
const RANDOM_SEED_BASE: u64 = 0x5eed_ba11_0000_0000;
const DUMB_JAM_PATH: &str = "../../../../assets/dumb.jam";

#[derive(Clone, Copy)]
enum TreeShape {
    Balanced,
    RightAssoc,
    Random(u64),
}

#[derive(Clone, Copy)]
enum OffsetMix {
    Stack100,
    Stack50,
    Stack90,
    Stack10,
}

fn make_stack() -> NockStack {
    NockStack::new(NOCK_STACK_SIZE_TINY, 0)
}

#[allow(dead_code)]
fn make_kernel_stack() -> NockStack {
    NockStack::new(NOCK_STACK_SIZE_TINY, 0)
}

fn make_pma(size_words: usize) -> (TempDir, Pma) {
    let dir = TempDir::new().expect("create temporary PMA directory");
    let path = dir.path().join("bench.pma");
    let pma = Pma::new(size_words, path).expect("create benchmark PMA");
    (dir, pma)
}

fn small_indirect(stack: &mut NockStack, seed: u64) -> Noun {
    let mut buf = [0u8; 24];
    buf[..8].copy_from_slice(&seed.to_le_bytes());
    buf[8..16].copy_from_slice(&seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).to_le_bytes());
    buf[16..24].copy_from_slice(&(seed ^ 0x55aa_55aa_55aa_55aa).to_le_bytes());
    unsafe { IndirectAtom::new_raw_bytes(stack, buf.len(), buf.as_ptr()).as_noun() }
}

fn large_indirect(stack: &mut NockStack, seed: u64, words: usize) -> Noun {
    debug_assert!((5..=1000).contains(&words));
    let mut data = vec![0u64; words];
    for (idx, slot) in data.iter_mut().enumerate() {
        *slot = seed
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .wrapping_add(idx as u64 ^ 0xfeed_face_dead_beef);
    }
    // Keep the tail non-zero so normalization does not shrink the atom.
    data[words - 1] |= 1;
    let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() << 3) };
    let atom = unsafe { IndirectAtom::new_raw_bytes(stack, bytes.len(), bytes.as_ptr()) };
    atom.as_noun()
}

fn build_tree_balanced(stack: &mut NockStack, mut leaves: Vec<Noun>) -> Noun {
    assert!(!leaves.is_empty());
    while leaves.len() > 1 {
        let mut next = Vec::with_capacity(leaves.len().div_ceil(2));
        for chunk in leaves.chunks(2) {
            if chunk.len() == 2 {
                let cell = Cell::new(stack, chunk[0], chunk[1]);
                next.push(cell.as_noun());
            } else {
                next.push(chunk[0]);
            }
        }
        leaves = next;
    }
    leaves.pop().expect("non-empty tree")
}

fn build_tree_right_assoc(stack: &mut NockStack, leaves: Vec<Noun>) -> Noun {
    assert!(!leaves.is_empty());
    let mut iter = leaves.into_iter().rev();
    let mut acc = iter
        .next()
        .expect("at least one element when building right-associated tree");
    for leaf in iter {
        acc = Cell::new(stack, leaf, acc).as_noun();
    }
    acc
}

fn build_tree_random(stack: &mut NockStack, leaves: Vec<Noun>, seed: u64) -> Noun {
    assert!(!leaves.is_empty());
    let mut rng = StdRng::seed_from_u64(seed);
    let mut nodes = leaves;
    while nodes.len() > 1 {
        let i = rng.random_range(0..nodes.len());
        let a = nodes.swap_remove(i);
        let j = rng.random_range(0..nodes.len());
        let b = nodes.swap_remove(j);
        let cell = Cell::new(stack, a, b).as_noun();
        nodes.push(cell);
    }
    nodes.pop().expect("non-empty tree")
}

fn build_tree_with_shape(stack: &mut NockStack, leaves: Vec<Noun>, shape: TreeShape) -> Noun {
    match shape {
        TreeShape::Balanced => build_tree_balanced(stack, leaves),
        TreeShape::RightAssoc => build_tree_right_assoc(stack, leaves),
        TreeShape::Random(seed) => build_tree_random(stack, leaves, seed),
    }
}

fn apply_offset_mix(stack: &NockStack, pma: &mut Pma, leaves: &mut [Noun], mix: OffsetMix) {
    let offset_target = match mix {
        OffsetMix::Stack100 => 0usize,
        OffsetMix::Stack50 => 50usize,
        OffsetMix::Stack90 => 10usize,
        OffsetMix::Stack10 => 90usize,
    };
    if offset_target == 0 || leaves.is_empty() {
        return;
    }

    let leaf_count = (leaves.len() * offset_target) / 100;
    for leaf in leaves.iter_mut().take(leaf_count) {
        unsafe { leaf.copy_to_pma(stack, pma) };
    }
}

fn build_unique_direct(_stack: &mut NockStack) -> Vec<Noun> {
    (0..LEAF_COUNT).map(|i| D(i as u64 + 1)).collect()
}

fn build_unique_small_indirect(stack: &mut NockStack) -> Vec<Noun> {
    (0..LEAF_COUNT)
        .map(|i| small_indirect(stack, 0x1000_0000 + i as u64))
        .collect()
}

fn build_mixed_direct_small_indirect(stack: &mut NockStack) -> Vec<Noun> {
    (0..LEAF_COUNT)
        .map(|i| {
            if i % 2 == 0 {
                D(i as u64 + 1)
            } else {
                small_indirect(stack, 0x2000_0000 + i as u64)
            }
        })
        .collect()
}

fn build_shared_small_indirect(stack: &mut NockStack) -> Vec<Noun> {
    let shared = small_indirect(stack, 0x5eed_baa5);
    let uniques = LEAF_COUNT / 10;
    let mut leaves = Vec::with_capacity(LEAF_COUNT);
    for i in 0..LEAF_COUNT {
        if i % 10 == 0 && (i / 10) < uniques {
            leaves.push(small_indirect(stack, 0x3000_0000 + (i / 10) as u64));
        } else {
            leaves.push(shared);
        }
    }
    leaves
}

fn build_unique_large_indirect(stack: &mut NockStack) -> Vec<Noun> {
    (0..LEAF_COUNT)
        .map(|i| {
            let size = 5 + (i % 996);
            large_indirect(stack, 0x4000_0000 + i as u64, size)
        })
        .collect()
}

fn build_shared_large_indirect(stack: &mut NockStack) -> Vec<Noun> {
    let shared = large_indirect(stack, 0x5aa5_5aa5, 768);
    let mut leaves = Vec::with_capacity(LEAF_COUNT);
    for i in 0..LEAF_COUNT {
        if i % 10 == 0 {
            let size = 5 + (i % 996);
            leaves.push(large_indirect(stack, 0x5000_0000 + (i / 10) as u64, size));
        } else {
            leaves.push(shared);
        }
    }
    leaves
}

lazy_static! {
    static ref DUMB_JAM_BYTES: Vec<u8> = {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DUMB_JAM_PATH);
        fs::read(&path).unwrap_or_else(|e| panic!("failed to read {:?}: {}", path, e))
    };
}

#[allow(dead_code)]
fn load_kernel_dumb(stack: &mut NockStack) -> Noun {
    Noun::cue_bytes_slice(stack, &DUMB_JAM_BYTES).expect("cue dumb.jam")
}

/// Load the kernel into stack-pointer form (for benchmarking retag_noun_tree)
#[allow(dead_code)]
fn load_kernel_dumb_stack_pointer_form(stack: &mut NockStack) -> Noun {
    let buffer = IndirectAtom::from_bytes(stack, &DUMB_JAM_BYTES);
    cue_into_stack_pointer_form(stack, buffer).expect("cue dumb.jam into stack-pointer form")
}

/// Sanity check: returns true if all allocated nouns in the tree are in stack-pointer form
/// (i.e., is_stack_allocated() returns true for all indirect atoms and cells).
/// Direct atoms are ignored since they have no allocation.
/// Returns (is_valid, stack_pointer_count, offset_count) for debugging
#[allow(dead_code)]
fn check_noun_tagging_state(
    stack: &NockStack,
    pma: Option<&Pma>,
    root: Noun,
) -> (bool, usize, usize) {
    let space = pma.map_or_else(
        || stack.noun_space(),
        |pma| nockvm::noun::NounSpace::new(stack, pma),
    );
    let mut work: Vec<Noun> = Vec::with_capacity(32);
    let mut visited: HashSet<u64> = HashSet::new();
    let mut stack_pointer_count = 0usize;
    let mut offset_count = 0usize;
    work.push(root);

    while let Some(noun) = work.pop() {
        // Direct atoms have no allocation, skip them
        if noun.is_direct() {
            continue;
        }

        // Check for duplicate visits (structural sharing)
        let raw = unsafe { noun.as_raw() };
        if !visited.insert(raw) {
            continue;
        }

        // For allocated nouns (cells and indirect atoms), count their form
        if noun.is_allocated() {
            if matches!(
                noun.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            ) {
                stack_pointer_count += 1;
            } else {
                offset_count += 1;
            }
        }

        // If it's a cell, traverse children
        if let Ok(cell) = noun.in_space(&space).as_cell() {
            let head = cell.head().noun();
            let tail = cell.tail().noun();
            work.push(head);
            work.push(tail);
        }
    }

    let all_stack_pointer = offset_count == 0;
    (all_stack_pointer, stack_pointer_count, offset_count)
}

/// Returns true if all allocated nouns are in stack-pointer form
#[allow(dead_code)]
fn is_entirely_stack_pointer_form(stack: &NockStack, root: Noun) -> bool {
    let (all_stack_pointer, _, _) = check_noun_tagging_state(stack, None, root);
    all_stack_pointer
}

/// Sanity check: returns true if all allocated nouns in the tree are in offset form
/// (i.e., is_stack_allocated() returns false for all indirect atoms and cells).
/// Direct atoms are ignored since they have no allocation.
#[allow(dead_code)]
fn is_entirely_offset_form(stack: &NockStack, root: Noun) -> bool {
    let space = stack.noun_space();
    let mut work: Vec<Noun> = Vec::with_capacity(32);
    let mut visited: HashSet<u64> = HashSet::new();
    work.push(root);

    while let Some(noun) = work.pop() {
        // Direct atoms have no allocation, skip them
        if noun.is_direct() {
            continue;
        }

        // Check for duplicate visits (structural sharing)
        let raw = unsafe { noun.as_raw() };
        if !visited.insert(raw) {
            continue;
        }

        // For allocated nouns (cells and indirect atoms), check they're in offset form
        if noun.is_allocated()
            && matches!(
                noun.in_space(&space).allocated_location(),
                Some(AllocLocation::Stack)
            )
        {
            // This noun is in stack-pointer form, not offset form
            return false;
        }

        // If it's a cell, traverse children
        if let Ok(cell) = noun.in_space(&space).as_cell() {
            let head = cell.head().noun();
            let tail = cell.tail().noun();
            work.push(head);
            work.push(tail);
        }
    }

    true
}

struct BenchCase {
    _pma_dir: TempDir,
    pma: Pma,
    stack: NockStack,
    root: Noun,
}

type BaseCase = (&'static str, fn(&mut NockStack) -> Vec<Noun>);

fn setup_case<F>(mut make_leaves: F, shape: TreeShape, mix: OffsetMix) -> impl FnMut() -> BenchCase
where
    F: FnMut(&mut NockStack) -> Vec<Noun>,
{
    move || {
        let mut stack = make_stack();
        let (pma_dir, mut pma) = make_pma(PMA_WORDS);
        let mut leaves = make_leaves(&mut stack);
        assert!(leaves.len() >= LEAF_COUNT);
        apply_offset_mix(&stack, &mut pma, &mut leaves, mix);
        let root = build_tree_with_shape(&mut stack, leaves, shape);
        BenchCase {
            _pma_dir: pma_dir,
            pma,
            stack,
            root,
        }
    }
}

fn bench_retag_noun_tree(c: &mut Criterion) {
    let mut group = c.benchmark_group("retag_noun_tree");
    let kernel_only = std::env::var_os("NOCKCHAIN_KERNEL_ONLY").is_some();
    let mut cases: Vec<(String, Box<dyn FnMut() -> BenchCase>)> = Vec::new();

    if !kernel_only {
        let base_cases: Vec<BaseCase> = vec![
            ("direct_unique", build_unique_direct),
            ("small_indirect_unique", build_unique_small_indirect),
            (
                "mixed_direct_small_indirect", build_mixed_direct_small_indirect,
            ),
            ("small_indirect_shared", build_shared_small_indirect),
            ("large_indirect_unique", build_unique_large_indirect),
            ("large_indirect_shared", build_shared_large_indirect),
        ];

        let mixes = [
            (OffsetMix::Stack100, "stack100"),
            (OffsetMix::Stack50, "stack50"),
            (OffsetMix::Stack90, "stack90"),
            (OffsetMix::Stack10, "stack10"),
        ];

        for (case_idx, (label, leaves_fn)) in base_cases.into_iter().enumerate() {
            let shapes = [
                (TreeShape::Balanced, "balanced"),
                (TreeShape::RightAssoc, "right_assoc"),
                (
                    TreeShape::Random(RANDOM_SEED_BASE ^ (case_idx as u64)),
                    "random",
                ),
            ];
            for (shape, suffix) in shapes {
                for (mix, mix_name) in mixes {
                    let name = format!("{label}_{suffix}_{mix_name}");
                    cases.push((name, Box::new(setup_case(leaves_fn, shape, mix))));
                }
            }
        }
    }

    for (label, setup) in cases.iter_mut() {
        let name = label.clone();
        group.bench_function(BenchmarkId::new("retag", name), |b| {
            // Only measure retag_noun_tree time; setup/build happens outside the timed section.
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let mut case = setup();
                    let start = Instant::now();
                    unsafe { case.root.copy_to_pma(&case.stack, &mut case.pma) };
                    black_box(&case.root);
                    total += start.elapsed();
                }
                total
            });
        });
    }

    // Kernel benchmark: retag_noun_tree
    // This tests the speed of converting a pointer-form noun to offset form
    // using the retag_noun_tree function.
    group.bench_function(BenchmarkId::new("kernel", "retag_noun_tree"), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                // Setup: cue the kernel into stack-pointer form
                let mut stack = make_kernel_stack();
                let (_pma_dir, mut pma) = make_pma(NOCK_STACK_SIZE_TINY);
                let mut kernel_ptr_form = load_kernel_dumb_stack_pointer_form(&mut stack);

                // Sanity check: kernel should be entirely in stack-pointer form before retagging
                let (all_stack, stack_count, offset_count) =
                    check_noun_tagging_state(&stack, Some(&pma), kernel_ptr_form);
                assert!(
                    all_stack,
                    "Kernel should be in stack-pointer form before retag_noun_tree. \
                     Found {} stack-pointer nouns and {} offset nouns",
                    stack_count, offset_count
                );

                // Timed section: retag_noun_tree
                let start = Instant::now();
                unsafe { kernel_ptr_form.copy_to_pma(&stack, &mut pma) };
                black_box(&kernel_ptr_form);
                total += start.elapsed();

                // Sanity check: kernel should be entirely in offset form after retagging
                let (_, stack_count_after, offset_count_after) =
                    check_noun_tagging_state(&stack, Some(&pma), kernel_ptr_form);
                assert!(
                    stack_count_after == 0,
                    "Kernel should be in offset form after retag_noun_tree. \
                     Found {} stack-pointer nouns and {} offset nouns",
                    stack_count_after,
                    offset_count_after
                );
            }
            total
        });
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .measurement_time(Duration::from_millis(700))
        .warm_up_time(Duration::from_millis(200))
        .sample_size(10);
    targets = bench_retag_noun_tree
}
criterion_main!(benches);
