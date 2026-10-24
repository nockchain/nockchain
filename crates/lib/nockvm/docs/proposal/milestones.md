# New Mars project milestones

> Historical note. This roadmap is archived project planning context, not current protocol or runtime authority.

The two major research (rather than implementation) goals of the New Mars project are:

1. The effectiveness of an on-stack bump allocator and delimited copying collector as a memory manager for Nock computational memory.
2. An effective linearization strategy for Nock code generation.

## Near Term: Memory management

The goal of this set of milestones is to validate the 2stackz memory model and its implementation

- run nock
  * 2stackz allocator (implemented)
  * tree-walking interpreter (in-progress)
    + unifying equality
    + tree edits
- run non-trivial amounts of nock
  * cue
  * load a small pill (probably of a custom core)
- run some compiled hoon
  * mug caching
  * noun hashtable
  * jam
  * jet dashboard
    + optimize for simplicity / speed of implementation

## Next: code generation

The goal of this stage is to generate highly-linearized IR from Nock

Note that a substantial part of this milestone is already accomplished, if unvalidated.
The New Mars project began with linearization of Nock across Nock 2 and Nock 9 being a problem without theoretical or design solution.
Subject-knowledge analysis (SKA) is the (currently theoretical) artifact which permits such linearization.

- Firm up reference Hoon implementation of SKA
- Use SKA to bytecode-compile Nock
  - This validates SKA as a code-generation and jet-matching strategy for Nock without bogging down in the details of LLVM
- Switch out bytecode for LLVM IR

## Later: heaps and paging

The goal of this stage is to permit New Mars to function fully as a Mars, saving snapshots and event logs, and paging infrequently used and/or large data out to disk.

- Threshhold based eviction to a heap
- Heap collection strategies
- Demand paging of heap objects
- Event logging and snapshotting using demand-paged heap objects
- Tie disk reclamation to virtual memory reclamation

## Historical progress:

Project time to this point has been spent:

1. Implementing the memory management strategy
2. Attacking the code linearization problem for Nock in order to come up with a means of producing direct, monomorphized calls to generated code for procedures and to jets.

The memory management implementation is complete but untested. The implementation of the tree-walking interpreter will permit us to test and harden this implementation.

The subject knowledge analysis is a still theoretical result, but a result nonetheless, which convincingly and constructively establishes that linearization and monomorphization of Nock for code generation is possible in a reasonably efficient manner.
