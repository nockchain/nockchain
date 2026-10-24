# Notes ~2021.9.23

> Historical note. Archived design notes, not current protocol or runtime authority.

## Discussion with ~rovnys-ricfer
* Some discussion of the memory model, in particular using tag bits to identify
  - direct noun
  - indirect noun
  - pointer to cell
  - hash table reference
* Hash table ejection strategy
  - When the copier is returning a noun, it counts how many iterations of copying it has performed
  - Above a tunable threshhold, an entry in the hash table is made and the noun is copied there instead.
  - Existing pointers into the hash table are copied and re-inserted as new entries, thus maintaining an invariant
    that a hash table entry can only reference its own memory by a direct pointer, or another hash table entry,
    by hash reference.
  - nouns that require metadata (jet pointers, cached bytecode) are ejected to the hashtable
  - hashtable can also store non-noun data such as bytecode
  - TBD: a collection strategy for the hash table.

## Infrastructure channel discussion
* Interpreter should handle nock 12 with an extra input of a scry gate stack, so it can be used to jet `+mink`
* Also need to return crash values from the interpreter, and build traces when passing through nock 11.
* ~master-morzod objects to calling the machine code from JIT compilation "automatic jets" and is probably right.
