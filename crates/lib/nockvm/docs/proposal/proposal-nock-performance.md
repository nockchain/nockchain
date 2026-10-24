# Introduction

> Historical note. This is an archived performance proposal, not current protocol or runtime authority.

The current Nock implementation is a limitation on the performance of Urbit. When performance of code is not limited by
algorithmic concerns, generally the only approach to increasing performance is to jet the code in question. For code such
as arithmetic, encryption, or bitwise operations this is the correct approach. For code with more complex control flow or
memory management behavior, writing a jet is a difficult and error-prone process.

It is possible for interpreted languages to be made very fast. Evaluation of a Nock-9 currently takes many tens or even a
few hundreds of microseconds, which in a deep and heterogenous call trace such as that of Ames quickly adds up to multiple
milliseconds to process one event. Memory management overhead after nock evaluation completes adds >1 millisecond, and
memory management overhead during evaluation is harder to measure but likely to be significant.

Functional programming language implementations mostly do not mutate, they allocate. This means that many allocations are
are discarded quickly. Nock is extreme about this: there is no possible way to mutate in Nock (with the only exception being
a case of optimization where there is no sharing). Therefore allocation should be fast, and garbage should not incur
management overhead.

Further, while a computed-goto bytecode interpreter is far faster than naive structural recursion over the noun tree or a
switch-cased interpreter, it still requires a computed jump in between every instruction, and does not admit well-known
low-level optimizations.

Urbit is a personal server. Nock is the language in which that personal server's software is provided. Browsers are personal
clients, and Javascript is the language in which browser software is provided. Javascript at one time had a reputation for
slowness due to its interpreted nature. But modern Javascript is quite fast, and this has had a qualitative, not simply
quantitative, effect on the types of software written for the browser platform.

Making Nock much faster than it is currently would plausibly have the same effect for Urbit.
It would provide immediate benefits in the form of Ames throughput and JSON handling for client interaction.
Further, applications not presently feasible on Urbit would rapidly become feasible.

This proposal also includes changes which would allow for incremental snapshotting and large looms, thus removing other
limitations to implementing applications for Urbit on Urbit.

# Ideas

## Lexical memory management
The current worker uses (explicit/manual) reference counting to manage allocated objects, and adds objects
scavenged on reclamation to a free list. This means that evaluation is subject to the overhead of reference counting all
allocated objects and of maintaining free lists when dead objects are scavenged. For a language which allocates at the
rate Nock (or really any functional language) does, this is not optimal.

However, the reference-counting scheme has the desirable property of having predictable, lexically-mappable behavior for
memory management. This behavior means that two runs of the same nock program produce the same code traces, even within
memory management code.

This could be achieved similarly by the following system.

Two 'call' stacks are maintained. Perhaps they share a memory arena and grow towards each other, analogous to roads without
heaps. Logically, they are one, interleaved stack, that is, a push to the top of the (logical) stack pushes onto the opposite
stack from the current top of the (logical) stack.

Noun allocation is performed by extending the stack frame and writing the noun on the stack. There is no heap*.

When it is time to pop a stack frame and return a value to the control represented by the previous stack frame,
a limited form of copying collection is performed. The return value is copied to return-target stack frame, which
because of the interleaved stack, also has free space adjacent. Descendant nouns referenced by the current noun are
copied in their turn, and pointers updated.

Note that only references to the returning frame need to initiate copies, and there can be no references to data in
the returning frame from outside the current frame, because there is no mutation and no cyclical references in Nock.
So the copied nouns can reference nouns "further up" the stack, but nouns further up the stack cannot reference nouns
in the current stack frame.

### Optimization: hash-indexed heap for large nouns
While for most computation this scheme should be an improvement, it can result in repeated copies up-the-stack
of large nouns. Nouns over a certain size can be ejected to an external heap indexed by a hashed table, thus providing
de-duplication and eliminating the need to copy.

### Advantages
* Allocation in this model is very fast as it involves only a pointer increment.
* Allocations are compact (not interleaved with free space) and tend toward adjacency of relative structures,
  leading to generally better cache locality.
* Pause times for 'collection' are lexically limited *by the size of the noun returned, less parts of the noun
  originating above the returning frame in lexical scope.*
* The predictable and repeatable memory managment behavior is retained.
* Memory management overhead is proportional to the size of nouns returned, *not* the size of
  discarded memory as is presently the case.
* Memory management does not require any data structures to persist between collections.
  (Ephemeral memory for the collector can be allocated above the frame being scavenged.)
* Big loom/snapshots: the implementation will use 64 bit pointers and thus remove the 2GB limit on loom/snapshot size.
* Incremental snapshots: By ejecting the result of an arvo event computation to the hash table,
  incremental snapshotting can be done by storing only new hashes in the table.
* Control structures for the interpreter itself are stored off the loom, simplifying it drastically.

### Disadvantages
* Copying and pointer adjustment could be expensive for large nouns (but see 'Optimization', above)
* Slightly less eager scavenging than reference counting, allocations persist for a lexical scope.
* Snapshots would not be compatible with current loom

## Just-in-time compilation

Nock code for execution is currently compiled at runtime to bytecode, which is interpreted by a looping interpreter using
'computed gotos', that is, program addresses for labels are computed and stored in an array, which is indexed by the
opcodes of the bytecode representation.

This approach is much faster than a naive looping or recursive structural interpreter. It can be made faster, especially
for code which is "hot" i.e. routinely called in rapid iteration.

The approach is to keep a counter on a segment of bytecode which is incremented each time it is run. This counter would
persist in between invocations of Arvo, so as to notice code which is 'hot' across the event loop. When a counter hits
a threshhold, the bytecode is translated into an LLVM graph, which can be fed to LLVM and result in a function pointer.
This function pointer is then stored as an "automatic jet" of the code.

Of course, the JIT compiler should also respect jet hints and link in existing jets, as LLVM is not likely to e.g.
optimize the naive O((x, y)^2) `%+  add  x  y` invocation into code using the ALU.

This approach of JIT compilation of hotspot code is used to great effect by Javascript in a context where
code loading is ephemeral and the performance benefits from a particular invocation of the JIT compiler last only
for the duration that a page is loaded. In a context where an Urbit persistently loops through much the same code for
every event (until Arvo or an application are updated) the overhead could be amortized across an even greater number of
invocations, over a longer period of time.

An even simpler approach is to simply JIT every formula to machine code, with the assumption that most code
will not be ephemeral. 

# Tasks

## A new Mars

***Time (core): 2 months***

***Time (jet porting): ?***

A new Mars implementation is written in Racket-derived C, containing a bytecode interpreter for Nock as well as snapshotting
and event logging. The implementation initially uses Boehm GC or similar off-the-shelf memory management. Jets are supported by porting them to use an allocator supplied by the interpreter.

## Lexical memory

***Time: 3 months***

The new mars implementation is converted to use lexical memory management as described above.
Jets may allocate memory using an allocation function provided by the interpreter, and may use this
memory as they wish, but *must not* mutate memory that they did not allocate.

Question: is Beohm modular or flexible enough that we can use all or part of it to implement this strategy?

## Jets-In-Time compilation of Nock bytecode

***Time: 4 months***

The new mars creates jets on-the-fly by using LLVM to compile Nock bytecode to machine code, whenever some metric of heat is reached (this metric is probably just a counter, as Urbit code will tend to be highly persistent rather than ephemeral).
