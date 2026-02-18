# Protocol Upgrade Specification Format

This document defines the format and process for documenting protocol upgrades.

## Overview

Protocol upgrades are changes that require nodes to update. Each upgrade is documented in a single file within this directory, following semver versioning starting from `0.1.0`.

## File Format

Each upgrade specification uses TOML frontmatter followed by Markdown content.

### File Naming

Files are named with a sequential number and a slug:

```
NNN-slug.md
```

Naming eras:

- **Pre-Bythos historical backfill** (`001`-`011`): use `legacy-*` slugs that describe the rule change.
- **Bythos and later** (`012+`): use **Gnostic Aeons** codenames (Bythos, Nous, Aletheia, Logos, Zoe, etc.).

Examples:
- `001-legacy-checkpoint-144.md`
- `012-bythos.md`

The sequential number provides ordering; the semver version in the frontmatter is authoritative.

### TOML Frontmatter

```toml
+++
version = "0.1.0"
status = "draft"  # draft | final | activated | superseded
consensus_critical = true

# Activation (filled in after coordination)
activation_height = 0  # 0 = not yet determined

# Dates
published = "2026-01-19"
activation_target = ""  # ISO date, must be >= 1 month after published

# People
authors = ["@nockchain-core"]  # handles preferred; "Name <email>" allowed
reviewers = ["@nockchain-core"]  # Required before status can be "final"

# If this upgrade replaces a previous one
supersedes = ""  # e.g., "0.0.9"
superseded_by = ""  # filled in if this spec is later superseded
+++
```

### Required Sections

Every upgrade specification MUST include the following sections:

#### 1. Summary

A brief (2-3 sentence) description of what the upgrade does. Written for all audiences.

#### 2. Motivation

Why is this change needed? What problem does it solve? Include context for node operators and integrators who may not be familiar with the codebase.

#### 3. Technical Specification

Detailed description of the changes. Include:

- Data structure changes (with before/after examples)
- Encoding/decoding changes
- Consensus rule changes
- API changes

Use code blocks and diagrams where helpful.

#### 4. Activation

- **Height**: The block height at which the upgrade activates (filled in after coordination)
- **Coordination**: Any special coordination required during rollout

#### 5. Migration

Instructions for node operators to upgrade:

- Required software version
- Configuration changes (if any)
- Data migration steps (if any)
- Rollback procedure (if applicable)

#### 6. Backward Compatibility

- Is this a breaking change?
- What happens to nodes that don't upgrade?
- What happens to transactions created with old software?

#### 7. Security Considerations

Describe any security-sensitive changes, new assumptions, or threat-model shifts.

#### 8. Operational Impact

Explain operator-facing impacts: resource usage, fee behavior, monitoring signals, and any rollout risks.

#### 9. Testing and Validation

List required or recommended tests and validation steps (unit/integration/manual).

#### 10. Reference Implementation

Link to the implementation (PR/commit/branch) and any related design docs.

## Status Lifecycle

```
draft → final → activated → superseded (optional)
```

- **draft**: Specification is being developed, subject to change
- **final**: Specification is complete, activation height set, awaiting activation
- **activated**: Upgrade is live on mainnet
- **superseded**: A newer upgrade replaces this one (set `superseded_by` field)

## Process

1. **Draft**: Author creates spec file with `status = "draft"`
2. **Review**: Reviewers listed in frontmatter review the spec
3. **Finalize**: After review, set `status = "final"` and determine activation height
4. **Announce**: Publish spec at least 1 month before activation
5. **Activate**: After activation height is reached, set `status = "activated"`

## Versioning Rules

- **Major** (X.0.0): Breaking consensus changes that may fork non-upgraded nodes
- **Minor** (0.X.0): Consensus changes that are backward-compatible or have graceful degradation
- **Patch** (0.0.X): Non-consensus changes that still require node updates (e.g., P2P protocol, RPC changes)

Since all changes in this repository are tracked together, even non-consensus changes that require node updates should be documented here.

## Machine-Readable Metadata

The TOML frontmatter can be parsed programmatically. The frontmatter is delimited by `+++` markers:

```
+++
[toml content]
+++

[markdown content]
```

Tools can extract metadata for:
- Generating upgrade timelines
- Checking activation status
- Building compatibility matrices
- Automating release notes

## Template

A minimal template for new upgrade specifications:

```markdown
+++
version = "X.Y.Z"
status = "draft"
consensus_critical = true

activation_height = 0
published = "YYYY-MM-DD"
activation_target = ""

authors = ["@nockchain-core"]  # handles preferred; "Name <email>" allowed
reviewers = ["@nockchain-core"]

supersedes = ""
superseded_by = ""
+++

# [Codename]

## Summary

[2-3 sentence overview]

## Motivation

[Why is this needed?]

## Technical Specification

[Detailed changes]

## Activation

- **Height**: TBD
- **Coordination**: [Any special requirements]

## Migration

### Requirements

- Software version: X.Y.Z+

### Configuration

[Config changes or "None"]

### Data Migration

[Data migration steps or "None"]

### Steps

1. [Step 1]
2. [Step 2]

### Rollback

[If applicable]

## Backward Compatibility

[Breaking change analysis]

## Security Considerations

[Security-sensitive changes or "None"]

## Operational Impact

[Operator-facing impact and monitoring notes]

## Testing and Validation

[Tests run or recommended validation steps]

## Reference Implementation

[Link to PR/commit/branch]
```
