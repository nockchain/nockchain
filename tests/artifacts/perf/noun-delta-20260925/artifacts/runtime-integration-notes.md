# What the timings do and do not mean

The saved PMA root is kernel state (arvo axis 6), not a complete live NockVM subject with battery. Inputs are two rotating snapshots and a stopped operative state, read from independent copied files. No node is booted.

The first semantic comparison hashes every reachable atom and cell in both inputs. A content lookup then recognizes unchanged subtrees despite relocation and different sharing. The delta stores new cells and full changed atoms, plus references into the base index. Original noun bytes and cached mugs are untouched.

A decoded delta creates a virtual Graph view. It validates new records using cached ancestor fingerprints; a separate full walk compares actual values. This is not the cost of writing a complete new PMA or making the existing NockVM boot directly from an overlay.

The chain variant retains the epoch fingerprint and lookup allocations with shared ownership, appending fingerprints for the new records only. It may retain ancestor nodes that are no longer reachable. Epoch rebasing eventually must discard that retention.

Existing production snapshots have no such per-noun content-hash sidecar. Reading an arbitrary fresh snapshot still requires a full scan. Achieving warm timings during routine runtime operation would require externally maintained fingerprints for new finalized nouns, preserving associations through GC/relocation, and a durable representation for references. These integration costs are not implemented or measured.

The optional sidecar sizes are exact serialized-format sizes, checked against actual bytes on synthetic fixtures. Large sidecars are not written or read on the server. Reports must not equate a virtual mount with a cold process restore; building the epoch index, loading any persisted cache, and full source verification are separate costs.

The128-bit variant trades fingerprint width/collision margin for memory and speed; it is an exploratory choice, not a production format recommendation. Independent full value verification is still required in this experiment.
