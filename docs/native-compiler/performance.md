# Native compiler performance notes

Performance work should not be separated from parity work. A faster compiler that
changes artifact bytes is a regression.

## Workflow

1. Make one class of performance change at a time.
2. Run the relevant parser and compiler parity tests before collecting new timing evidence.
3. Use compiler timing totals to distinguish parsing, compiling self time, compiling-with-children time, and interpreting time.
4. Keep diagnostic logging completion-oriented and environment-gated so normal builds stay quiet and deterministic.

## `bran_canonical_semi` memoization

Large compiler workloads can spend disproportionate time repeatedly projecting seminouns from large subject types. Memoizing `bran_canonical_semi` is valid only when the memo key distinguishes all semantic context that can affect the result.

In particular:

- Do not ignore active `%hold` expansion context.
- Store enough hold-state signature data to reject a candidate memo entry when the current recursion guard context differs.
- Treat memo hits as an optimization only. Disabling the memo must preserve artifact bytes.
- After changing this memoization, run byte-for-byte compiler artifact parity before trusting timing improvements.

## Useful workload

`crates/hoonc/hoon/hoon-138.hoon` arbitrary compilation is a useful stress case because it exercises parser source spots, compiler type operations, and large emitted artifacts in a single parity target.

## Kernel serialization benchmark

`just honk-nockasm-serialization-bench` compiles the complete Dumbnet kernel
once, then compares three serialization paths over the prepared noun:

- NockVM noun to canonical JAM bytes;
- the equivalent Nockasm noun to its sharing-preserving `NasmDag` AST;
- the equivalent Nockasm noun through that AST to versioned DAG text.

Compilation and cueing are setup and are not included in sample latency. Each
case runs in a separate child process so peak RSS and allocator state do not
leak between cases. Every case defaults to 20 samples and three warmups; tune
them independently with `HONK_BENCH_JAM_SAMPLES`, `HONK_BENCH_AST_SAMPLES`,
and `HONK_BENCH_TEXT_SAMPLES` (and their `_WARMUPS` counterparts). Select a
subset with `HONK_BENCH_CASES=jam,ast,text` and replace the fixture with
`HONK_KERNEL_JAM=/path/to/kernel.jam`.

Before timing AST or text conversion, a separate validation process proves
that the DAG AST lowers to the original kernel noun, DAG text parses back to
the same nodes and root, and that parsed graph also lowers to the original.
This validation process is isolated so its allocator high-water mark does not
pollute the benchmark samples.

The runner writes its host fingerprint, p50/p95/p99/max latencies, logical
throughput, output size, peak RSS, and child wall time to
`target/honk-nockasm-serialization/results.txt`. A failed child is part of the
result rather than a harness failure; this makes an isolated regression in one
representation visible without losing the other samples. A correctness
failure in the preflight DAG round trip aborts the benchmark.

### Reference result (2026-08-06)

Apple M5 Max, 128 GiB RAM, 20 samples after three warmups, complete Dumbnet
kernel (19.08 MiB canonical JAM):

| Path | p50 | p95 | p99 | Result | Repeated-process peak RSS |
|---|---:|---:|---:|---:|---:|
| NockVM JAM | 1.891 s | 2.106 s | 2.152 s | 19.08 MiB | 1.09 GiB |
| Nockasm DAG AST | 813 ms | 958 ms | 962 ms | 3,448,885 nodes | 3.20 GiB |
| Nockasm DAG text | 1.049 s | 1.122 s | 1.149 s | 111.91 MiB | 2.75 GiB |

The AST and text processes retain allocator arenas across 23 conversions; an
isolated one-shot conversion peaked at approximately 923 MiB for each. Before
DAG preservation, the boxed-tree lift failed to complete: the benchmark child
was killed after roughly 175 seconds, and a standalone probe reached about
52 GiB RSS. These figures are host-specific; use the checked-in runner for
comparisons on another machine or after allocator/toolchain changes.
