use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use nockvm::mem::NockStack;
use nockvm::noun::{Cell, Noun, D};
use nockvm::pma::{Pma, PmaCopy};
use tempfile::TempDir;

const CELL_WORDS: usize = 3;
const LIST_CELLS: usize = 4096;
const EXPECTED_WORDS: usize = LIST_CELLS * CELL_WORDS;
const STACK_WORDS: usize = EXPECTED_WORDS * 2 + 1024;

fn build_direct_atom_list(stack: &mut NockStack, len: usize) -> Noun {
    let mut noun = D(0);
    for value in (0..len).rev() {
        noun = Cell::new(stack, D(value as u64), noun).as_noun();
    }
    noun
}

fn copy_fixture(pma_words: usize, reserved_words: usize) -> (TempDir, NockStack, Pma, Noun) {
    let dir = TempDir::new().expect("create PMA benchmark dir");
    let path = dir.path().join("bench.pma");
    let mut stack = NockStack::new(STACK_WORDS, 0);
    let noun = build_direct_atom_list(&mut stack, LIST_CELLS);
    let pma = Pma::new_with_reserved(pma_words, reserved_words, path).expect("create PMA");
    (dir, stack, pma, noun)
}

fn populated_pma_fixture() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().expect("create PMA open benchmark dir");
    let path = dir.path().join("open.pma");
    {
        let mut stack = NockStack::new(STACK_WORDS, 0);
        let mut noun = build_direct_atom_list(&mut stack, LIST_CELLS);
        let mut pma = Pma::new(EXPECTED_WORDS * 2, path.clone()).expect("create PMA");
        unsafe {
            noun.copy_to_pma(&stack, &mut pma);
        }
        pma.sync_all().expect("sync PMA mapping");
        pma.sync_file().expect("sync PMA file");
    }
    (dir, path)
}

fn bench_pma_copy_no_growth(c: &mut Criterion) {
    c.bench_function("pma_copy_direct_list_no_growth", |b| {
        b.iter_batched(
            || copy_fixture(EXPECTED_WORDS * 2, EXPECTED_WORDS * 4),
            |(_dir, stack, mut pma, mut noun)| unsafe {
                noun.copy_to_pma(&stack, &mut pma);
                black_box(pma.alloc_offset());
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_pma_copy_with_growth(c: &mut Criterion) {
    c.bench_function("pma_copy_direct_list_with_growth", |b| {
        b.iter_batched(
            || copy_fixture(1024, EXPECTED_WORDS * 4),
            |(_dir, stack, mut pma, mut noun)| unsafe {
                noun.copy_to_pma(&stack, &mut pma);
                black_box((pma.size_words(), pma.alloc_offset()));
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_pma_open(c: &mut Criterion) {
    let (_dir, path) = populated_pma_fixture();
    c.bench_function("pma_open_populated", |b| {
        b.iter(|| {
            let pma = Pma::open(black_box(path.clone())).expect("open PMA");
            black_box((pma.size_words(), pma.alloc_offset()));
        });
    });
}

fn criterion_benchmark(c: &mut Criterion) {
    bench_pma_copy_no_growth(c);
    bench_pma_copy_with_growth(c);
    bench_pma_open(c);
}

criterion_group!(pma_growth, criterion_benchmark);
criterion_main!(pma_growth);
