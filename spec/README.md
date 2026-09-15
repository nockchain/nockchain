# Bridge withdrawal model

`bridge-withdrawal.qnt` models protocol facts for one complete
Base-to-Nockchain withdrawal plus a bounded second withdrawal competing for the
same input set. `bridge-withdrawal-tests.qnt` contains deterministic happy,
restart, pre/post-authorization reorg, replacement, two-withdrawal ordering,
stale-kernel, simultaneous-fork, and compensation runs.
`mutations/bridge-withdrawal-mutations.qnt` contains deliberately unsafe
transitions used only as negative controls.

## Local check

Run from the repository root:

```sh
scripts/check-bridge-withdrawal-model.sh
```

The command requires `node`, `npx`, `jq`, and `timeout`. It resolves exactly
Quint 0.32.0, rejects version drift, typechecks both model files, executes the
named simulations, runs a seeded safety simulation, checks the complete bounded
state graph with TLC, checks both explicit-fairness liveness properties, and
requires all seven mutations to produce machine-readable counterexamples.
Timeout is failure, never a pass.

Each invocation creates a new `target/bridge-model/<UTC-time>-<pid>/` directory.
It never replaces an earlier run. `summary.json` records the tool version, seed,
bound, timeout, positive checks, and expected negative controls. Simulator and
counterexample traces use Informal Trace Format (ITF) JSON; command logs remain
beside them.

Optional bounds:

- `BRIDGE_MODEL_MAX_STEPS` (default `20`)
- `BRIDGE_MODEL_SEED` (default `424242`)
- `BRIDGE_MODEL_TIMEOUT_SECONDS` per command (default `180`)

## Negative controls

| Mutation module | Deliberate defect | Required failing invariant |
|---|---|---|
| `reservation_owner_mutation` | assign one selected input to another withdrawal | `oneReservationOwnerInv` |
| `authorized_identity_mutation` | replace raw transaction identity after authorization | `retryIdentityInv` |
| `authorized_base_fork_mutation` | discard authorized raw identity and reservations on a Base fork | `authorizedRawIdentityInv` |
| `premature_second_withdrawal_mutation` | reserve inputs for withdrawal two before withdrawal one confirms | `secondReservationOrderInv` |
| `compensation_payout_mutation` | pay a compensated burn | `compensationExcludesPayoutInv` |
| `premature_terminal_mutation` | publish terminal before kernel settlement | `terminalProofInv` |
| `skipped_deep_hold_mutation` | record a deep fork without entering hold first | `unsafeForkHoldsInv` |

A negative control passes only when Quint exits nonzero with a parseable ITF
trace containing at least two states. A missing trace, malformed trace, timeout,
or mutation that no longer reaches its bad state fails the command.

## Importing a safety counterexample

Translate a machine-readable ITF safety trace into the versioned E2E action
envelope:

```sh
bridge-dev e2e import-formal target/bridge-model/<run>/<trace>.itf.json \
  --property oneReservationOwnerInv \
  --counterexample-id reservation-owner
```

The importer infers serialized model transitions from adjacent states, maps
symmetry-renamed integer inputs to stable `formal-input-N` note names, inserts
explicit observation barriers, and validates the resulting action trace before
writing it. Each create-new output directory keeps
`formal-counterexample.itf.json`, `scenario.json`, and `import-report.json`
together. `scenario.json` carries source path/hash, property, counterexample ID,
and model schema version in its action-scenario envelope.

Supported safety mutations include reservation ownership, authorized raw
transaction identity, authorized Base-fork preservation, premature second
withdrawal admission, compensation/payout exclusion, and premature terminal
publication. A proper deep-hold transition imports; an abstract transition that
skips the hold has no safe runtime action and is rejected explicitly.
Fairness/liveness lassos are also rejected rather than fabricated as shell
steps. A translated scenario can be passed directly to
`bridge-dev e2e replay <scenario.json>`; only an actually observed invariant
failure counts as SUT reproduction.

## Scope and limits

The state space is deliberately finite: three kernel nodes, two selected
inputs, one complete withdrawal, one bounded competing withdrawal, two proposal
epochs, and two journal generations. One bounded shallow Nockchain reinclusion
is modeled. Fixed abstract amounts preserve both bridge and Nockchain
conservation equations. TLC explores the complete reachable graph within the
documented 20-step bound, but that does not prove cryptography, SQLite
transactions, RPC authentication, process supervision, real chain timing, or
correctness beyond these bounds. The Quint model remains an independent oracle;
real E2E and runtime trace conformance are separate gates.

This check is local-only. No workflow invokes it yet.
