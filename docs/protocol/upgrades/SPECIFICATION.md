# Protocol upgrade specifications

The [protocol index](../README.md) records the current release track and
activation conditions. Each upgrade has a versioned file in this directory;
runbooks and implementation summaries do not override its rules.

## Files and metadata

Files use `NNN-slug.md`: the sequence orders entries, while `version` is the
protocol version. Entries `001`–`011` are historical `legacy-*` backfills;
`012` onward use Aeon codenames. Retain historical entries even when an upgrade
never activated.

Specifications begin with TOML delimited by `+++`:

```toml
+++
version = "0.1.0"
consensus_critical = true
activation_height = 0
published = "2026-01-19"
activation_target = ""
supersedes = ""
superseded_by = ""
+++
```

- `activation_height = 0` means no consensus-height trigger is recorded: it may
  be a historical gap or an operationally coordinated rollout. It does not mean
  activation at genesis. Read the specification's Activation section.
- `activation_target` records a scheduled date or target; it does not replace the
  actual activation condition. Scheduled activations should be published at least
  one month ahead.
- `supersedes` and `superseded_by` record release-track lineage, not proof that
  either version activated. A replacement must state when its predecessor never
  activated.
- Existing `authors` and `reviewers` fields record attribution and review history.

The frontmatter supports upgrade timelines and compatibility tooling. Preserve
its meaning when updating historical specifications.

## What an upgrade needs to explain

Describe the motivation and exact consensus, data-structure, encoding, and API
changes. Include the activation condition, required software/artifacts,
configuration or data migration, rollback limits, and what happens to nodes and
transactions that do not upgrade. State security assumptions, concrete failure
modes, operator actions before activation, and relevant validation commands or
results. Link the reference implementation when available.

Consensus-affecting changes must be version-gated. Coordinate activation before
rollout and keep the protocol index consistent with the specification, including
replacements and never-activated plans. Documentation alone does not activate a
rule.

## Versioning

- **Major** (`X.0.0`): breaking consensus changes that may fork non-upgraded nodes.
- **Minor** (`0.X.0`): consensus changes that are backward-compatible or have graceful degradation.
- **Patch** (`0.0.X`): non-consensus changes that still require node updates, such as P2P or RPC changes.

Non-consensus changes that require coordinated node updates also belong in the
upgrade history.
