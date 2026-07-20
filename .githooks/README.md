# Git hooks

`make install-hooks` sets `core.hooksPath -> .githooks`.

The pre-commit hook refuses staged changes under `crates/plonky3-recursion`
unless the commit explicitly opts in:

```sh
ALLOW_VENDORED_RECURSION_CHANGES=1 git commit ...
```

That tree is vendored for upstream comparison. Formatting-only churn belongs in
neither routine commits nor broad formatter runs.

## Run these manually before committing

```sh
make fmt         # formats root-owned workspace packages only
make clippy-fix  # apply clippy autofixes (or `make clippy` to only check)
```

Then stage and review the changes yourself so each commit stays scoped:

```sh
git add -p       # stage intentionally, not `git add -u`
```
