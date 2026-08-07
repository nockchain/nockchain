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
