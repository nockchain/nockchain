# `honk` OSS next steps

## Requirements

- Skip any items with `SKIP:` or similar in the title.
- Parity check honk artifacts against the kernels in assets/*.jam, but be aware dir-hash won't match unless it's a Bazel sandboxed build.
- Do not delete or re-build the hoonc-generated artifacts in assets/*.jam, they take far too long to rebuild. Especially right now while everything is slower.
- `honk` must not take more than 1 minute to compile the roswell kernel
- The final state of this branch must eliminate the overhead inflicted on `nockvm` and cleanly conform `honk` to the post-PMA architecture of `nockvm`

## Work

- Fix the items in `docs/TODOS.md`
- Fix the items in `docs/TODOS-PERF.md`

## Docs provided for context

- `docs/native-compiler/BATTERIES-MATCHES-STRUCTURAL-EQUALITY.md`
- `docs/native-compiler/DOR-DEEP-EQUALITY.md`
- `docs/native-compiler/FIND-JET-NORMALIZATION.md`
- `docs/native-compiler/LOG-HINT-EVENT.md`
- `docs/native-compiler/SLAB-PMA-COMINGLING.md`
