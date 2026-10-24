# Hypotheses tested by New Mars

> Historical note. This proposal captures research hypotheses, not current protocol or runtime authority.

## Stack-only allocation for computation
**Hypotheses:**
*Stack allocations, with nouns for return copied up the stack ("lexical memory management")
is a feasible way to implement Nock. Specifically*
- *Lexical memory management is simple to implement.*
- *Lexical memory management provides performant allocation and collection of Nock nouns.*

## Just-in-time compilation
Intuitively, compiling Nock bytecode to machine code should provide performance wins with a highly-amortized cost.

Urbit code is highly persistent. We don't dynamically load code for a short interactive session and then discard it,
but instead load *and already compile* hoon code to Nock, then continue using that code in an event loop for a long period
until the next OTA update of Arvo or an application.

Especially since we already take the time to compile hoon to nock, it likely makes sense to compile Nock to machine code
that can be directly invoked without interpretation overhead.

**Hypothesis:**
*Compiling Nock bytecode to machine code and caching the compilation results in much faster Nock execution,
and the expense of compilation is acceptable given the amortization across a very high number of invocations.*

## Large-noun hash table
The major downside of the lexical memory management scheme is that large nouns allocated deep in the stack and returned from
Arvo will be repeatedly copied to return them through multiple continuation frames. This can be ameliorated by using
a separate arena for large nouns. The step of copying the noun up the stack tracks how much memory has been copied, and,
at a certain threshhold, resets the stack pointer to undo the copy and instead copies the noun into the separate
arena and returns a reference.

By making this arena a hash table, we can create a store which can be copy-collected without adjusting references.
This can also serve to deduplicate nouns in the table.

The hashtable serves as a place to store non-noun objects, such as bytecode or jet dashboards, and a place to store noun
metadata. Rather than suffering the overhead of possible metadata annotations on every cell, we can instead only
allow metadata as the head of a hashtable entry.

This hashtable also permits differential snapshotting, by storing only the hashes which are new in the table since the last
snapshot. It also permits paging of large nouns to disk, as a hashtable entry could be marked with a path to a page file
and paged out.

**Hypotheses**:
- *A hash referenced memory arena for large nouns resolves the major downside of lexical memory management by preventing repeated copying of large nouns.*
- *A hash referenced memory arena provides a store for non-noun objects in the nock interpreter.*
- *A hash referenced memory arena provides transparent incremental snapshotting and disk paging of large nouns.*
