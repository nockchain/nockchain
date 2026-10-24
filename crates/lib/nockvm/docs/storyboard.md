# New Mars storyboard

Status: Historical

*Trust posture: this document captures planning context, not current roadmap or canonical architecture policy.*

## Current Reality Check (2026-02-19)

- The two-stack allocator discussed here exists in current code (`rust/nockvm/src/mem.rs`).
- The file-backed PMA path discussed here is not present in the current `rust/nockvm` crate.
- Time estimates and implementation status below are historical snapshots from the original planning period.

## Elevator pitch
An Urbit runtime with near-machine execution speed and NAS-equivalent addressable storage enables most classes of personal applications to be built directly on Urbit.

## Wins
 
### Near-machine execution speed
At the time of writing, standard library functions which implement highly-iterated loops, such as `turn`, were jetted to remove arm invocation overhead arising from indirect jumps, dynamic jet matching, and allocation for parameters. The majority of overhead in hot codepaths (such as Ames) was in arm invocations themselves, not in the code for arms.

The proposed replacement was static jet matching, which removes runtime overhead for jet matching whether a jet is found or not; mostly-direct arm invocations (always direct for idiomatic loops in Hoon) which removes prediction overhead, and bump-allocated and/or register-mapped subjects, which either eliminate memory allocation overhead or make it equivalent to parameter-passing on the stack.
Taken together, these approaches were expected to eliminate nearly all overhead in Nock execution and make Urbit performance comparable to native implementations of equivalent programs (modulo algorithmic mismatches for which jets are still necessary).

**Components: Codegen and 2stackz allocator**

### NAS-equivalent addressable storage
The current vere is a 32-bit program which addresses a 2, 4, or possibly 8GB "loom" which represents a unified single-level store for both intermediate computational results and persistent state between events. This is implemented by anonymously remapping pages upon writing to them, and then re-writing dirty pages back to the snapshot at regular intervals for durability. The smaller pointer size and the anonymous mapping of "dirty" pages (though primarily the former) limit the size of the loom, necessitating off-Urbit ("off-loom") storage for large objects, especially images and multimedia.

The proposed replacement was a 64-bit (practically, 47-bit due to limitations of underlying CPU and operating system architectures) arena for persistent data, using a copy-on-write strategy to ensure durability while ensuring all in-use pages can remain mapped to backing disk storage at all times. This would retain the single-level store while permitting the underlying OS's virtual memory system to evict pages at will, uncoupling the size of Urbit's data storage from pointer size limitations and available system RAM and swap space.

**Components: persistent memory arena**

## Naming
"New Mars" is an excellent skunkworks name but a terrible product name. Perhaps "Ares" is better for a product. Consultation welcome. BIKESHEDDING ENDS HERE

## Historical status at time of writing: R&D

Note: *Technical risk* denotes a low-resolution estimate of the probability that a show-stopping technical problem which returns the project to R&D or makes it demonstrably infeasible emerges from a particular component. *Implementation effort* is a low-resolution estimation of the developers X effort required to complete and integrate the component into a releasable New Mars/Ares product.

### Codegen

Most active work is on code generation, driven by ~ritpub-sipsyl. Subject knowledge analysis, destination-driven linearization, and registerization combine to turn Nock into a nearly zero-cost abstraction. Active implementation effort is focused on the yak-shaving always involved in generating low-level code, and on the problem of propagating subject-knowledge analysis across code-as-state boundaries (arvo-to-vanes, gall-to-agents). Codegen code is currently in Hoon, which requires some bootstrapping story for how codegen code is run and the results supplied to the runtime for execution.

A potential technical risk mitigation is to initially target a bytecode rather than machine code for the lowering of linearized Nock. This eliminates concerns about detecting and managing platform portability and decreases implementation effort and long-tail bugfixes and optimizations significantly.

**Technical risk:** medium to low

**Implementation effort:** high

### 2stackz allocator

At the time of this storyboard, the allocator was considered fully implemented and pending hardening/benchmarking.

**Technical risk:** low

**Implementation effort:** low

### Permanent memory arena

At the time of this storyboard, this was under active development, primarily driven by ~finmep-lanteb. The primary innovation was the application to the Urbit runtime of well-understood techniques in database persistence, with pointers instead of semantically higher-level keys as indexes. The primary open question was virtual memory system pressure: can the underlying OS handle the number of virtual memory mappings contained in the table, or is an on-demand mapping strategy necessary? The latter option was considered feasible and supported by implementation decisions (in particular the use of a B+ tree for the page directory and an always-file-mapped copy-on-write strategy for memory mapping), but with extra implementation effort.

**Technical risk**: low

**Implementation effort**: medium

## Non-R&D technical requirements

### Jets
One of the hairier yaks to shave to turn New Mars into a product is to rewrite a sufficient set of jets. New Mars' code generation system for Nock should eliminate the need for jets to ameliorate function call and/or looping overheads. Jets will still be needed for algorithmic or numeric hardness, e.g. arithmetic, bit-twiddling, encryption. There are over 350 jets in the current vere, ranging from the absolutely necessary (`+add`) to the absolutely obsoleted (`+turn`).

### Event logging

The only well-tested approach is logging events to LMDB as the current `vere` does, so we will initially re-use this approach.

### Frontend

We will need to re-implement bootstrapping from a boot sequence (contained in a pill), IPC to communicate with Urth, an event loop which receives events via IPC and dispatches them for logging and processing, and some form of pier import from `vere`.

## Hypothetical storyboard

| Task                                                            | Time | P   |
| --------------------------------------------------------------- | ---- | --- |
| **Jets**                                                        |      |     |
| Tabulate the jets in current vere                               | 4w   | Y   |
| Implement jets known absolutely to be necessary                 | ???  | Y   |
| Test-and-implement to discover remaining necessary jets         | 8w   | N   |
| **Codegen**                                                     |      |     |
| Bytecode generator and interpreter for linearized Nock          | 4w   | N   |
| Memory representation of code table keyed by subject knowledge  | 3w   | N   |
| SKA propagation through dynamic dispatch (vanes/agents/threads) | 12w  | N   |
| Bootstrapping: compile codegen code to bytecode                 | 8w   | N   |
| Virtualization and error handling                               | 4w   | N   |
| **2stackz**                                                     |      |     |
| Harden                                                          | 3w   | Y   |
| Integrate with PMA                                              | 2w   | N   |
| **PMA**                                                         |      |     |
| Implement B+ tree page index                                    | 5w   | N   |
| Garbage collection from stack root                              | 4w   | N   |
| Harden                                                          | 3w   | Y   |
| **Frontend**                                                    |      |     |
| Urth IPC protocol                                               | 3w   | N   |
| Bootstrapping from a pill                                       | 5w   | N   |
| Implement an event loop (receive IPC events, log, evaluate)     | 3w   | N   |
| **Event Log**                                                   |      |     |
| Re-implement Vere LMDB Event log                                | 4w   | N   |
| **Compatibility**                                               |      |     |
| Migration tooling (cue snapshot + metadata) vere->New Mars      | 2w   | N   |
