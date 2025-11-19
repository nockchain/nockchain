use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use nockvm::hamt::Hamt;
use nockvm::interpreter::{interpret, Context, NockCancelToken, Slogger};
use nockvm::jets;
use nockvm::jets::cold::Cold;
use nockvm::jets::hot::{Hot, URBIT_HOT_STATE};
use nockvm::jets::warm::Warm;
use nockvm::mem::NockStack;
use nockvm::noun::{self, Noun, D, T};
use nockvm::serialization::{cue, jam};
use nockvm::unifying_equality::unifying_equality;

struct BenchSlogger;

impl Slogger for BenchSlogger {
    fn flog(&mut self, _stack: &mut NockStack, _cord: Noun) {}

    fn slog(&mut self, _stack: &mut NockStack, _pri: u64, _noun: Noun) {}
}

fn bench_context() -> Context {
    let mut stack = NockStack::new(8 << 20, 0);
    let cold = Cold::new(&mut stack);
    let warm = Warm::new(&mut stack);
    let hot = Hot::init(&mut stack, URBIT_HOT_STATE);
    let cache = Hamt::<Noun>::new(&mut stack);
    let slogger = std::boxed::Box::pin(BenchSlogger);
    let cancel = Arc::new(AtomicIsize::new(NockCancelToken::RUNNING_IDLE));
    let test_jets = Hamt::<()>::new(&mut stack);

    Context {
        stack,
        slogger,
        cold,
        warm,
        hot,
        cache,
        scry_stack: D(0),
        trace_info: None,
        running_status: cancel,
        test_jets,
    }
}

fn build_symbol_table(stack: &mut NockStack, count: usize) -> (Hamt<Noun>, Vec<Noun>) {
    let mut table = Hamt::new(stack);
    let binding = noun::tape(stack, "binding");
    let mut keys = Vec::with_capacity(count);
    for idx in 0..count {
        let name = format!("sym-{idx:04x}");
        let mut key = noun::tape(stack, &name);
        let payload = T(
            stack,
            &[binding, D((idx % 32) as u64), D(((idx * 65_537) % 1_000_003) as u64)],
        );
        table = table.insert(stack, &mut key, payload);
        keys.push(key);
    }
    (table, keys)
}

fn bench_hamt_symbol_table(c: &mut Criterion) {
    c.bench_function("hamt_symbol_table_hot_path", |b| {
        b.iter_batched(
            || {
                let mut stack = NockStack::new(16 << 20, 0);
                let (table, keys) = build_symbol_table(&mut stack, 256);
                (stack, table, keys)
            },
            |(mut stack, mut table, keys)| {
                let reload = noun::tape(&mut stack, "reload");
                for (round, key) in keys.iter().enumerate().take(96) {
                    let mut lookup_key = *key;
                    let hit = table.lookup(&mut stack, &mut lookup_key);
                    black_box(hit);

                    if round % 8 == 0 {
                        let mut reinsertion_key = *key;
                        let payload = T(&mut stack, &[D(0xfaceu64), reload, D(round as u64)]);
                        table = table.insert(&mut stack, &mut reinsertion_key, payload);
                    }
                }
                unsafe {
                    stack.preserve(&mut table);
                }
            },
            BatchSize::LargeInput,
        );
    });
}

fn build_deep_ast(stack: &mut NockStack, depth: usize, breadth: usize) -> Noun {
    let mut acc = D(0);
    for level in 0..depth {
        let label = noun::tape(stack, &format!("arm-{level:03}"));
        let mut branch = D(0);
        for offset in 0..breadth {
            let leaf_tag = noun::tape(stack, "leaf");
            let leaf = T(stack, &[D((level * breadth + offset) as u64), leaf_tag]);
            branch = T(stack, &[leaf, branch]);
        }
        acc = T(stack, &[label, branch, acc]);
    }
    acc
}

fn bench_noun_preserve(c: &mut Criterion) {
    c.bench_function("noun_preserve_deep_core", |b| {
        b.iter_batched(
            || {
                let mut stack = NockStack::new(24 << 20, 0);
                let ast = build_deep_ast(&mut stack, 160, 4);
                (stack, ast)
            },
            |(mut stack, mut ast)| {
                unsafe {
                    stack.preserve(&mut ast);
                }
                black_box(ast);
            },
            BatchSize::LargeInput,
        );
    });
}

fn build_hint_formula(stack: &mut NockStack) -> (Noun, Noun) {
    let subj = D(0);

    let hint_spot = D(1_953_460_339);
    let hint_path = T(stack, &[D(1_953_719_668), D(0)]);
    let hint_dyn = D(0);
    let hint_row = D(1);

    let make_hint = |stack: &mut NockStack, col_start: u64, col_end: u64| {
        let start = T(stack, &[hint_row, D(col_start)]);
        let end = T(stack, &[hint_row, D(col_end)]);
        T(stack, &[hint_spot, hint_dyn, hint_path, start, end])
    };

    let sss3s1 = T(stack, &[D(0), D(3)]);
    let sss3s2s1 = make_hint(stack, 20, 22);
    let sss3s2s2 = T(stack, &[D(1), D(53)]);
    let sss3s2 = T(stack, &[D(11), sss3s2s1, sss3s2s2]);
    let sss3 = T(stack, &[D(7), sss3s1, sss3s2]);

    let sss2s1 = sss3s1;
    let sss2s2s1 = make_hint(stack, 16, 18);
    let sss2s2s2 = T(stack, &[D(0), D(0)]);
    let sss2s2 = T(stack, &[D(11), sss2s2s1, sss2s2s2]);
    let sss2 = T(stack, &[D(7), sss2s1, sss2s2]);

    let sss1s1 = T(stack, &[D(1), D(0)]);
    let sss1s2 = T(stack, &[D(0), D(2)]);
    let sss1 = T(stack, &[D(5), sss1s1, sss1s2]);

    let ss2 = T(stack, &[D(6), sss1, sss2, sss3]);

    let ss1s1 = make_hint(stack, 13, 14);
    let ss1s2 = sss1s1;
    let ss1 = T(stack, &[D(11), ss1s1, ss1s2]);

    let s2 = T(stack, &[D(8), ss1, ss2]);
    let s1 = make_hint(stack, 9, 22);
    let form = T(stack, &[D(11), s1, s2]);

    (subj, form)
}

fn bench_interpret_hint_case(c: &mut Criterion) {
    c.bench_function("interpret_hint_stack", |b| {
        b.iter_batched(
            || {
                let mut ctx = bench_context();
                let (subj, form) = build_hint_formula(&mut ctx.stack);
                (ctx, subj, form)
            },
            |(mut ctx, subj, form)| {
                ctx.running_status
                    .store(NockCancelToken::RUNNING_IDLE, Ordering::SeqCst);
                let outcome = interpret(&mut ctx, subj, form);
                black_box(&outcome);
            },
            BatchSize::SmallInput,
        );
    });
}

fn build_balanced_tree(stack: &mut NockStack, depth: u8, seed: u64) -> Noun {
    if depth == 0 {
        D(seed & 0xffff_ffff)
    } else {
        let left_seed = seed
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .wrapping_add(0x85eb_ca6b);
        let right_seed = seed.wrapping_mul(0xc2b2_ae35).wrapping_add(0x27d4_eb2f);
        let head = build_balanced_tree(stack, depth - 1, left_seed);
        let tail = build_balanced_tree(stack, depth - 1, right_seed);
        T(stack, &[head, tail])
    }
}

fn bench_unifying_equality(c: &mut Criterion) {
    c.bench_function("unifying_equality_canopy", |b| {
        b.iter_batched(
            || {
                let mut stack = NockStack::new(24 << 20, 0);
                let mut pairs = Vec::with_capacity(48);
                for i in 0..48 {
                    let left = build_balanced_tree(&mut stack, 6, i as u64 + 1);
                    let delta = if i % 2 == 0 { 0 } else { 1 };
                    let right = build_balanced_tree(&mut stack, 6, i as u64 + 1 + delta);
                    pairs.push((left, right));
                }
                (stack, pairs)
            },
            |(mut stack, pairs)| {
                for &(left, right) in &pairs {
                    let mut lhs = left;
                    let mut rhs = right;
                    let first = unsafe { unifying_equality(&mut stack, &mut lhs, &mut rhs) };
                    let mut lhs_again = left;
                    let mut rhs_again = right;
                    let second =
                        unsafe { unifying_equality(&mut stack, &mut lhs_again, &mut rhs_again) };
                    black_box((first, second));
                }
            },
            BatchSize::SmallInput,
        );
    });
}

fn build_serialization_fixture(stack: &mut NockStack) -> Noun {
    let mut acc = D(0);
    for idx in 0..128 {
        let key = noun::tape(stack, &format!("binding-{idx:03}"));
        let payload_tag = noun::tape(stack, "payload");
        let val = T(
            stack,
            &[D((idx * 7 + 3) as u64), payload_tag, D(idx as u64)],
        );
        acc = T(stack, &[key, val, acc]);
    }
    acc
}

fn bench_cue_jam_roundtrip(c: &mut Criterion) {
    c.bench_function("cue_jam_roundtrip", |b| {
        b.iter_batched(
            || {
                let mut stack = NockStack::new(32 << 20, 0);
                let noun = build_serialization_fixture(&mut stack);
                let jammed = jam(&mut stack, noun);
                (stack, jammed)
            },
            |(mut stack, jammed)| {
                let decoded = cue(&mut stack, jammed).expect("cue succeeds");
                let rejam = jam(&mut stack, decoded);
                black_box(rejam);
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_warm_lookup(c: &mut Criterion) {
    c.bench_function("warm_jet_lookup", |b| {
        b.iter_batched(
            || {
                let mut ctx = bench_context();
                ctx.warm = Warm::init(&mut ctx.stack, &mut ctx.cold, &ctx.hot, &ctx.test_jets);
                let mut subject = D(0);
                let leaf = noun::tape(&mut ctx.stack, "leaf");
                subject = T(&mut ctx.stack, &[leaf, subject]);
                let formula = jets::nock::util::slam_gate_fol(&mut ctx.stack);
                (ctx, subject, formula)
            },
            |(mut ctx, mut subject, mut formula)| {
                let mut warm = ctx.warm;
                let hit = warm.find_jet(&mut ctx.stack, &mut subject, &mut formula);
                let mut bogus_formula = D(0);
                let miss = warm.find_jet(&mut ctx.stack, &mut subject, &mut bogus_formula);
                black_box((hit, miss));
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_cache_churn(c: &mut Criterion) {
    c.bench_function("context_cache_churn", |b| {
        b.iter_batched(
            || {
                let mut ctx = bench_context();
                let mut keys = Vec::with_capacity(96);
                for i in 0..96 {
                    let key = build_balanced_tree(&mut ctx.stack, 5, 0xfeed_face_u64 ^ (i as u64));
                    keys.push(key);
                }
                (ctx, keys)
            },
            |(mut ctx, keys)| {
                let mut cache = ctx.cache;
                for (idx, &key) in keys.iter().enumerate() {
                    let mut insert_key = key;
                    let value = T(&mut ctx.stack, &[key, D(idx as u64)]);
                    cache = cache.insert(&mut ctx.stack, &mut insert_key, value);
                    if idx % 4 == 0 {
                        let mut probe = key;
                        let _ = cache.lookup(&mut ctx.stack, &mut probe);
                    }
                }
                for &key in keys.iter().rev().take(32) {
                    let mut probe = key;
                    let _ = cache.lookup(&mut ctx.stack, &mut probe);
                }
                ctx.cache = cache;
                black_box(ctx.cache.is_null());
            },
            BatchSize::SmallInput,
        );
    });
}

fn criterion_benchmark(c: &mut Criterion) {
    bench_hamt_symbol_table(c);
    bench_noun_preserve(c);
    bench_unifying_equality(c);
    bench_interpret_hint_case(c);
    bench_warm_lookup(c);
    bench_cache_churn(c);
    bench_cue_jam_roundtrip(c);
}

criterion_group!(hoonc_hotspots, criterion_benchmark);
criterion_main!(hoonc_hotspots);
