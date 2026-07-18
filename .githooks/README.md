# Git hooks (disabled)

The auto-format / clippy-fix **pre-commit hook was removed**. It ran
`make fmt` + `make clippy-fix` across the *whole workspace* on every commit and
`git add -u`'d everything they changed, which pulled unrelated churn into
otherwise-focused commits (and made each commit slow).

## Run these manually before committing

```sh
make fmt         # cargo fmt across the workspace
make clippy-fix  # apply clippy autofixes (or `make clippy` to only check)
```

Then stage and review the changes yourself so each commit stays scoped:

```sh
git add -p       # stage intentionally, not `git add -u`
```

`make install-hooks` only sets `core.hooksPath -> .githooks`; with no
`pre-commit` file here it is now a no-op. To re-enable an automatic hook, add a
`pre-commit` script here and run `make install-hooks`.
