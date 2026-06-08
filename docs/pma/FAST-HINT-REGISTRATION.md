# `%fast` Hint Registration

This document explains how `%fast` hint registration works in the current runtime, with special attention to how the cold state is constructed, what data and code participate in that construction, and what constraints the implementation relies on.

This is a description of the code as it exists today. It is not a design proposal.

## Scope

The relevant code lives primarily in:

- `crates/nockvm/rust/nockvm/src/interpreter.rs`
- `crates/nockvm/rust/nockvm/src/jets/cold.rs`
- `crates/nockvm/rust/nockvm/src/jets/warm.rs`
- `crates/nockvm/rust/nockvm/src/jets/hot.rs`
- `crates/nockvm/rust/nockvm/src/trace/mod.rs`
- `crates/nockvm/rust/nockvm/src/trace/tracing_backend.rs`
- `crates/nockvm/rust/nockvm/src/site.rs`
- `crates/nockvm/rust/nockvm/src/hamt.rs`
- `crates/nockvm/rust/nockvm/src/unifying_equality.rs`

Persistence and serialization code is also relevant because the cold state is stored and moved around as a runtime object:

- `crates/nockapp/src/kernel/form.rs`
- `crates/nockapp/src/nockapp/export.rs`

## Executive Summary

`%fast` is a dynamic Nock 11 hint that records structural information about cores as they are produced.

At a high level:

1. The interpreter evaluates a dynamic hint formula and obtains a clue noun.
2. After the hinted body finishes evaluating, `%fast` extracts a `chum` and a parent formula from that clue.
3. The result core and its parent relationship are registered into `context.cold`.
4. If registration inserted a new entry, the runtime rebuilds `context.warm` from scratch.
5. Later jet lookup uses `warm` to find candidate jets and uses ancestry information stored in `cold` to prove that the candidate really matches the live core.
6. The same cold-state path information is also used for tracing.

The cold state is therefore a derived structural index over runtime-discovered cores. It is not the kernel state itself. It exists to make jet lookup and tracing possible.

## Where `%fast` Sits in the Interpreter

### Nock 11 dispatch

Opcode 11 is handled in `interpret()` in `interpreter.rs`.

- A static hint becomes `NockWork::Work11S`.
- A dynamic hint becomes `NockWork::Work11D`.

The parser distinguishes them by looking at the head of the hint payload:

- If the head is an atom, it is a static hint.
- If the head is a cell whose head is an atom, it is a dynamic hint.

For dynamic hints, the interpreter stores:

- `tag`
- `hint` as a formula to be evaluated
- `body` as the actual formula whose result may be affected or observed

### Dynamic hint evaluation order

`Work11D` runs in three stages:

1. `ComputeHint`
2. `ComputeResult`
3. `Done`

Those stages call:

- `hint::match_pre_hint(...)`
- `hint::match_pre_nock(...)`
- `hint::match_post_nock(...)`

`%fast` only does work in `match_post_nock()`. It does not short-circuit the body. It does not change the result noun. It is a side-effecting registration step that runs after the hinted body has already been evaluated.

One explicit control-flow constraint is in `hint::is_tail()`: `%fast` is treated as non-tail. That prevents the interpreter from discarding the current frame before post-hint work runs.

## What `%fast` Reads

Inside `hint::match_post_nock()`:

- `res` is the result of evaluating the hinted body.
- `hint` is the already-evaluated clue noun for the dynamic hint.

The `%fast` logic expects the clue to provide:

- `chum = clue.slot(2)`
- `parent = clue.slot(6)`

The code then peels nested hint wrappers off `parent`. While `parent` looks like a Nock 11 formula, it follows `slot(7)` until it reaches the underlying formula.

After unwrapping, it reads:

- `parent_formula_op = parent.slot(2)`
- `parent_formula_ax = parent.slot(3)`

Those values are then used to interpret the parent relationship for the result core:

- If `parent_formula_op == 1` and `parent_formula_ax == 0`, the result is treated as a root registration.
- Otherwise `parent_formula_ax` is treated as the axis from the result core to its parent core.

This means `%fast` does not directly consume a parent core from the clue. It consumes a parent formula description and then applies the resulting axis to the actual runtime result `res`.

## Static Data Versus Dynamic Data

The runtime uses three related structures:

### Hot state

`Hot` is a static table built from `URBIT_HOT_STATE` plus any constant hot entries.

Each `HotEntry` is:

```text
(path, axis-in-battery, jet-function)
```

`Hot::init()` constructs noun paths from those static path segments and stores them as a linked list of `HotMem` nodes.

Hot state answers:

- Which path names are associated with jets?
- At what axis inside the battery does the jetted formula live?

Hot state does not know which live runtime cores correspond to those paths.

### Cold state

`Cold` is the dynamic structural index built by `%fast` registration.

It answers:

- Which runtime-discovered cores correspond to which paths?
- For a given path, what ancestry chain of batteries proves that match?
- For a given outer battery or root, which paths might be candidates?

### Warm state

`Warm` is derived from `Hot` plus `Cold`.

It answers:

- Given a formula, which jet candidates should we try?

Warm state is the actual formula-indexed jet lookup table used during Nock 9 execution.

## The Cold State Representation

`Cold` is a wrapper around `ColdMem`, which holds three HAMTs:

```text
battery_to_paths   : Hamt<NounList>
root_to_paths      : Hamt<NounList>
path_to_batteries  : Hamt<BatteriesList>
```

### `battery_to_paths`

Key:

- The outermost battery of a core, meaning `core.slot(2)`

Value:

- A linked list of candidate registered paths for cores with that battery

This exists because identical Nock can appear at multiple places. A battery is not unique enough to identify a path by itself.

### `root_to_paths`

Key:

- A root noun

Value:

- A linked list of root paths

This is the root-only analogue of `battery_to_paths`.

### `path_to_batteries`

Key:

- A registered path noun

Value:

- A linked list of possible battery ancestry chains for that path

This is the proof object used to validate that a live core really corresponds to a registered path.

## Paths and Chums

Paths are represented as nested noun lists ending in `0`.

Examples:

```text
root path  = [chum 0]
child path = [chum parent_path]
```

This representation is used consistently across:

- `Cold::register()`
- `Hot::init()`
- `Warm::init()`
- trace path rendering in `path_to_cord()`

There are two important implications:

1. Static hot paths and dynamic `%fast` paths must use the same encoding or warm rebuild cannot join them.
2. `chum` shape matters. The code currently does not validate `chum` beyond a TODO comment in `Cold::register()`.

In practice, the surrounding code assumes `chum` is path-friendly:

- an atom, usually printable text
- or a `[name version]` cell used by path formatting logic

Malformed `chum` values may still register structurally, but they can break or degrade path rendering and logging.

## Batteries and Batteries Lists

The most important cold-state type is `Batteries`.

Each `BatteriesMem` node stores:

- `battery: Noun`
- `parent_axis: Atom`
- `parent_batteries: Batteries`

Conceptually, a `Batteries` chain is a proof that says:

```text
this core's battery is X
its parent is reached by axis A
that parent has battery Y
its parent is reached by axis B
...
eventually we reach a root
```

The root is marked by `parent_axis == 0`.

`BatteriesList` is just a linked list of alternative ancestry chains for one path. Multiple chains may exist because the same path may correspond to multiple structurally distinct but valid candidates.

## How `Batteries::matches()` Works

`Batteries::matches(stack, core)` validates a live core against a stored ancestry chain.

For each stored `(battery, parent_axis)`:

1. If `parent_axis == 0`, the live `core` must unify with the stored root battery noun.
2. Otherwise:
   - the live `core.slot(2)` must unify with the stored battery
   - `core = core.slot(parent_axis)` becomes the next parent core

If the walk reaches a root and all checks succeeded, the chain matches.

This is the key correctness check in the whole design. Registration and jet lookup both depend on it.

## How `Cold::register()` Works

`Cold::register(stack, core, parent_axis, chum) -> Result<bool, Error>`

The return value means:

- `Ok(true)`: new registration inserted
- `Ok(false)`: registration already existed or was intentionally ignored
- `Err(NoParent)`: a parent path could not be resolved
- `Err(BadNock)`: structural operations like `slot()` failed

### Root registration

If `parent_axis == 0`:

1. Construct `root_path = [chum 0]`.
2. Check `root_to_paths[core]` to see whether that exact root/path pair already exists.
3. If not present:
   - create a one-node `Batteries` chain with:
     - `battery = core`
     - `parent_axis = 0`
     - `parent_batteries = NO_BATTERIES`
   - prepend that chain to `path_to_batteries[root_path]`
   - prepend `root_path` to `root_to_paths[core]`
4. Build a new `ColdMem` containing the updated HAMT roots.

Notably, the root case does not update `battery_to_paths`.

### Non-root registration

If `parent_axis != 0`:

1. Extract:
   - `battery = core.slot(2)`
   - `parent = core.slot(parent_axis)`
   - `parent_battery = parent.slot(2)`
2. Check for an existing registration:
   - look up `battery_to_paths[battery]`
   - for each candidate path:
     - compare the path head against `chum`
     - if that matches, look up `path_to_batteries[path]`
     - if any stored ancestry chain matches the full live `core`, the registration already exists
3. Resolve the parent path:
   - first through `battery_to_paths[parent_battery]`
   - then through `root_to_paths[parent]`
4. For each candidate parent path:
   - fetch `path_to_batteries[parent_path]`
   - use `BatteriesList::matches(parent)` to validate the actual parent core
5. For each successful parent match:
   - construct `my_path = [chum parent_path]`
   - create a new `Batteries` node containing:
     - `battery`
     - `parent_axis`
     - `parent_batteries = matched parent chain`
   - prepend that chain to `path_to_batteries[my_path]`
   - prepend `my_path` to `battery_to_paths[battery]`
6. Build a new `ColdMem` containing the updated HAMT roots.

If no parent candidate matches, registration returns `Err(NoParent)`.

## Why the Cold State Uses Three Indices

The three-way indexing exists because no single key is sufficient:

- Path alone is useful for warm rebuild and tracing, but registration often starts from a core, not a path.
- Battery alone is useful for narrowing search, but the same battery can appear at multiple paths.
- Full core structural comparison is too expensive to use as the primary hash key.

The current scheme is:

1. Use outer battery or root as a coarse filter.
2. Enumerate candidate paths.
3. Use stored ancestry chains to prove the exact match.

That design is visible directly in `Cold::register()`, `Cold::matches()`, and `Warm::find_jet()`.

## HAMT Update and Equality Semantics

The cold-state HAMTs are not used like a conventional immutable map keyed by pure equality.

Important details:

- `Hamt::lookup()` takes `&mut Noun` for the key because key comparison uses `unifying_equality()`.
- `unifying_equality()` is not just a read-only comparison. It can rewrite references so equal nouns become unified.
- `Hamt::insert()` returns a new `Hamt` root rather than mutating the old one in place.
- `Cold::register()` follows that style by assembling updated HAMT roots and then allocating a new `ColdMem` to hold them.

That means the cold-state update pattern is functionally persistent at the top level, but equality checks during lookup and insertion are still allowed to mutate noun references for unification.

This is a real implementation constraint, not an incidental detail. Code that uses cold-state HAMTs must tolerate mutating equality.

## How Warm State Is Rebuilt

If `%fast` registration returns `Ok(true)`, the interpreter immediately runs:

```rust
context.warm = Warm::init(stack, cold, hot, &context.test_jets)
```

This rebuilds the entire warm table.

`Warm::init()` does:

1. Iterate every entry in `hot`.
2. For each hot path, call `cold.find(path)` to get the `BatteriesList` for that path.
3. For each batteries chain at that path:
   - take the outer battery
   - slot into that battery at the hot entry's axis
   - use that resulting formula as the warm lookup key
4. Insert a `WarmEntry` containing:
   - `batteries`
   - `jet`
   - `path`
   - `test` flag

Warm is therefore a formula-indexed cache derived from:

- static jet declarations in `hot`
- runtime structure discovered in `cold`

## How Warm Lookup Uses Cold Data

During Nock 9, the interpreter:

1. Computes the formula from the current core and axis.
2. Looks up candidate entries in `warm` by that formula.
3. For each candidate, uses `batteries.matches(subject_core)` to validate the live core.
4. Runs the jet if one matches.
5. Falls back to raw Nock if no candidate matches.

This means:

- `warm` is the fast lookup table
- `cold` provides the structural proof that keeps lookup sound

Without the ancestry match, formula equality alone would not be sufficient to identify the right core.

## How Cold Paths Are Used Outside Jet Lookup

Cold also feeds tracing.

The interpreter calls `cold.matches(stack, &mut res)` in trace-related paths after some Nock 9 activity. If it finds a path, that path is appended to the trace stack.

`path_to_cord()` converts the nested noun-list path representation into a slash-separated cord. The tracing backend also extracts the leftmost `chum` for span naming.

So `%fast` registration affects:

- jet lookup
- trace labeling
- trace path rendering

## Additional Consumer Constraint in `Site::new()`

`Site::new()` does a warm lookup too, but it adds one more constraint: it inspects the first `parent_axis` in each batteries chain and requires that axis 7 be a prefix.

The comment explains the intent: this avoids considering matches where the sample axis 6 would be part of the jet match.

This is not part of `Cold::register()` itself, but it is part of the wider `%fast` contract because cached call-site jetting depends on the shape of the stored ancestry data.

## Structural and Semantic Constraints

### Explicit constraints in the code

- `%fast` only does anything in the post-hint path.
- `%fast` is treated as non-tail.
- If the build uses `sham_hints`, `%fast` registration is disabled.
- Dynamic `%fast` requires a clue. A static `%fast` hint has no clue and will only log an error.
- The clue is expected to have meaningful values at slots 2 and 6.
- Nested parent hint wrappers are only unwrapped by following Nock 11 slot 7.
- Root registration is only accepted when the unwrapped parent formula has opcode 1 and axis 0.
- `Cold::register()` expects the result noun to support `slot(2)` and parent-axis navigation as a core.
- `chum` validation is explicitly missing. There is a TODO comment for this.
- `Warm::init()` rebuilds from scratch only when registration reports `Ok(true)`.

### Implicit constraints in the code

- Static hot paths and dynamic `%fast` paths must share the same noun encoding.
- `chum` values must be path-compatible in practice, even though the code does not enforce that.
- Parent registrations must happen before child registrations, or child registration fails with `NoParent`.
- The same outer battery can occur at multiple paths, so callers must tolerate candidate fan-out.
- Matching depends on `unifying_equality()`, which can mutate nouns to unify them. The code assumes all nouns passed into these comparisons are safe to unify.
- Matching relies on stable core structure. If the parent axis or core layout convention changes, registration and lookup both break.
- Cold grows monotonically. There is no pruning or delete path in this logic.
- Warm rebuild cost grows with:
  - the size of the hot table
  - the number of registered paths
  - the fan-out of `path_to_batteries`
- The order of linked-list insertion affects lookup order. The implementation assumes that trying candidates sequentially is acceptable.
- Roots are keyed in `root_to_paths`, but ordinary trace lookup through `cold.matches()` starts from outer battery and path-to-batteries. Roots matter mainly for registration of descendants.

## Representation Constraints

The cold state is not just nouns. It is a graph of custom runtime structures:

- `ColdMem`
- `Hamt`
- `NounList`
- `Batteries`
- `BatteriesList`

Those structures contain raw pointers and embedded nouns.

That matters because:

- `Cold` is preserved across stack frame flips using `Preserve`.
- `Cold` is copied into PMA using `PmaCopy`.
- `Cold` can be serialized into a noun with `Cold::into_noun()` and rebuilt with `Cold::from_noun()` plus `Cold::from_vecs()`.

The representation therefore carries strong locality assumptions. It is cheap to use in-memory, but it is not a naturally relocatable data structure.

## Persistence and Serialization Constraints

Cold construction interacts with persistence in several places:

- `Cold` is copied into PMA as part of the persistent runtime state.
- PMA metadata stores a `cold_offset`.
- Snapshots validate and carry a `cold_offset`.
- Checkpoints convert cold to a noun and then copy that noun into a slab.

Two constraints are worth calling out explicitly:

1. The PMA representation of cold relies on pointer-bearing runtime structs being copied into PMA and later reopened.
2. The checkpoint path already contains a warning:

   > Cold state has nouns in it which are *not* copied in `into_noun`

That warning is in `SaveableCheckpoint::new()` and documents an existing footgun in how cold is exported.

These persistence details are not part of `%fast` matching logic itself, but they are part of the full cold-state contract because `%fast` registration is what builds the structure being persisted.

## Failure Modes and Diagnostics

The current code reports several failures through logging rather than hard failure:

- no clue for `%fast`
- invalid root parent axis
- bad clue formula
- `NoParent` when parent lookup cannot be resolved
- non-atom or non-UTF-8 `chum` while formatting an error

Operationally, the main outcomes are:

- registration succeeds and warm is rebuilt
- registration is a duplicate and nothing changes
- registration fails and the runtime continues without that cold entry

The steady-state effect of missing registrations is typically reduced jet coverage and reduced path information for tracing, not direct corruption of the kernel state.

## Performance Characteristics

The hot path cost of `%fast` registration is not just the insert itself.

A successful registration can traverse:

- the clue noun
- nested parent hint wrappers
- the result core
- the parent core
- candidate path lists from `battery_to_paths`
- candidate ancestry chains from `path_to_batteries`
- `Batteries::matches()` walks over the live core hierarchy

Then, if the registration is new, it triggers a full `Warm::init()` rebuild over the entire hot table.

So the dominant cost of `%fast` is often:

- candidate-path fan-out during registration
- full warm rebuild after insertion

not the single HAMT insert alone.

## Mental Model

The cleanest way to think about the system is:

- `Hot` says which names have jets.
- `%fast` tells the runtime which live cores correspond to those names.
- `Cold` stores that discovered correspondence and the ancestry proof for it.
- `Warm` turns the combination of `Hot` and `Cold` into fast formula-indexed jet lookup.

The core invariant is:

> A jet is only sound to run when the runtime can prove that the live core belongs to the registered path associated with that jet.

In this implementation, that proof is the ancestry chain stored in `Batteries` and validated by `Batteries::matches()`.
