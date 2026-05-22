# ADR 0003: Merge Master Into Long-Running Feature Branches

Status: accepted
Date: 2026-05-03
Owners: @sobchek
Supersedes: none
Superseded by: none
Related:
- [START_HERE](../START_HERE.md)
- [WORKFLOWS](../WORKFLOWS.md)
- [DECISIONS index](./README.md)

## Context

Long-running feature branches (the first concrete case is `feat/package-patches`, PR #117) accumulate commits while master keeps moving. Master's dependency churn — workspace deps, new crates, version pins — frequently produces conflicts in `Cargo.lock`, which cannot auto-merge because both sides modify the same lines from the merge base.

When the conflict surfaces, contributors face two choices:

- **Rebase** the feature branch onto master: linear history, but rewrites commit hashes and breaks in-flight review continuity (existing PR comments lose anchor commits, force-push churn).
- **Merge** master into the feature branch: preserves commit hashes and review continuity at the cost of a merge commit in history.

## Decision

For long-running feature branches that have accumulated review activity, **merge master in** rather than rebase.

Resolve `Cargo.lock` by:

1. Take master's lockfile: `git checkout --theirs -- Cargo.lock && git add Cargo.lock`.
2. Re-run `cargo update --workspace` so the merged manifest's new entries (workspace deps added by the feature branch) are written to the lock without disturbing master's pins.
3. Verify with `cargo check -p <feature-crate> --locked` before committing the merge.

For short-lived branches with no review activity, rebase is still preferred.

## Alternatives Considered

1. **Always rebase.** Rejected for long-running PRs — disrupts review continuity and forces reviewers to re-orient against new commit hashes after every master sync.
2. **Pre-emptively mirror master's `Cargo.lock` on the branch.** Rejected — fragile (requires re-syncing on every master lockfile change) and doesn't actually integrate master's manifest changes that the lock references.

## Consequences

- Positive: Review continuity preserved across master syncs.
- Tradeoff: Merge commits appear in branch history.
- Follow-on: Merge commit messages should record conflicted paths and resolution strategy so future readers can audit.

## Rollout

1. PR #117 (`feat/package-patches`) is the first branch to follow this convention; see merge commit `Merge master into feat/package-patches` (`ae71443`).
2. New long-running branches: apply this policy on first conflict.
3. Backout: revert the merge commit and rebase if review state allows.
