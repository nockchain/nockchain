# Noun Representation

> Historical note. This document describes an exploratory representation model and is not current protocol or runtime authority.

A noun is represented as a word, which may be a pointer or reference to other memory.

Semantically, nouns are either atoms or cells, where a cell is an ordered pair of nouns.

A noun-word is a word sized to the machine architecture (e.g. 64 bits on x86_64, 32 bits on ARMv)
which, possibly together with other memory to which it points, represents a noun.

A noun-word with a 0 as the most significant bit is a direct atom.
The machine word corresponds exactly to the value of the atom.

A noun-word with 10 as the most significant bits is an indirect atom. By masking the 2 least significant bits,
this noun can be converted to a (4 byte aligned) pointer, which points to an length-tagged array.
That is, the machine word pointed to by the pointer specifies the length of the array in bytes,
and is immediately followed in memory by a byte array containing the full indirect atom.

A noun-word with 11 as the most significant bits is a pointer to a cell. By masking the 2 least significant bits,
this noun can be converted to a (8 byte aligned) pointer, which points to a cell in memory.

```
MSB                                                          LSB                  
|--------------------------------------------------------------|                  
0XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX  Direct atom     
10XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX  Indirect atom   
11XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX  Pointer to cell 
```

## Indirect atoms

An indirect atom is a pointer to a machine word, which word is directly adjacent to an array of bytes.
The machine pointer is obtained by left-shifting by 2 bits.
The machine word specifies the size of the atom in bytes, and the byte array stores the atom in little-endian order.

## Cells

A cell is a pointer to a noun-word, which is adjacent to another noun-word.
The machine pointer is obtained by left-shifting by 2 bits.
The pointed-to noun-word is the head of the cell, and the immediately adjacent noun-word is the tail.

# Noun allocation
The primary allocation of memory is done in the _stack arena_.
This arena contains two stacks growing in opposite directions from opposite ends of the arena.
The stack growing from lower memory addresses towards higher addresses is called the "north" stack, while the stack growing from higher to lower addresses is the "south" stack.

Computation begins with a stack frame on the north stack. The current stack frame is delimited by two registers: the stack pointer and the frame pointer.
A stack frame maintains the stack pointer and frame pointer for the previous stack frame as the first two words within the frame, except for the top frame
where these words are null (`0x0`).

Testing whether we are currently in the north or south stack is done by comparing the frame pointer to the stack pointer.
If the frame pointer is greater than the stack pointer, we are in the north stack, otherwise we are in the south stack.

Pushing of a new stack frame is illustrated by the following C-like pseudocode:

```c
void push() {
  if(fp > sp)
  {
    /* we are on the north stack, push on the south stack by reading the end of the last
     * south stack frame (the saved frame pointer). If it's null push at the end of the memory arena */
    void * new_sp = *(sp + sizeof(void*));
    if(new_sp == 0x0) {
      new_sp = arena + arena_size - 1;
    };
    void * new_fp = new_sp - 2 * sizeof(void*);
    // save old SP at new SP
    // save old FP one word below
    *new_sp = sp;
    *(new_sp - sizeof(void*)) = fp;
    sp = new_sp;
    fp = new_fp;
  } else {
    /* we are on the south stack, push onto the north stack by reading the end of the last
     * north stack frame (the saved frame pointer). */
    void* new_sp = *(sp - sizeof(void*));
    void* new_fp = new_sp + 2 * sizeof(void*);
    // Save old SP at new SP
    // Save old FP one word above
    *new_sp = sp;
    *(new_fp + sizeof(void*)) = fp;
    sp = new_sp;
    fp = new_fp;
  }
}
```

Nouns can be pushed onto the stack frame as follows:

```c
// TODO: check that stacks haven't grown into each other

/* Allocate memory on the stack frame for some noun words.
 * Note that the parameter is denominated in noun-words, not bytes.
 * 
 * This enables machine-portable code.
void* push_noun(size_t nouns) {
  if(fp > sp) { // north stack
    // fp is always pointing to free space
    base = fp;
    fp = fp + nouns * sizeof(void*);
    return base;
  } else
    fp = fp - nouns * sizeof(void*);
    // fp is always pointing to free space
    base = fp + 1;
    return base;
  }
}
```

Allocating a cell and receiving it as a noun-word is as follows:

```c
void* push_cell(void* hed, void* tal) {
  void* cel = push(2);
  *(cel) = hed
  *(cel + sizeof(void*)) = tal;
  cel = cel >> 2;
  cel = cel | (0b11 << (sizeof(void*) - 2));
  return cel;
}
```

Pushing an indirect atom is done by providing a pointer to the atom which be copied ot the stack:

```c
void* push_indirect_atom(void* tom) {
  tom = tom << 2;
  size_t words = *tom;
  void* noun = push_noun(words + 1);
  memcpy(noun, tom, (words + 1) * sizeof(void*));
  tom = tom >> 2;
  tom = tom | (0b10 << (sizeof(void*) - 2));
  return tom;
}
```

## Hash reference

XXX

A noun which the copier determines requires more copying than some tunable threshhold can instead be copied into the _hash arena_.

The noun is hashed with a secure hash function. (TODO: which one and how is the noun serialized for hashing?).
The hash arena consists of a bump-allocated memory arena which contains a HAMT (1) mapping hashes of nouns to pointers to those nouns,
and a 1-D (2) tree mapping memory allocations for cells to the hashes of cells.

The noun arena is collected whenever it fills by copying to a new arena.
References are never stored as hashes. Instead, they are stored as indirect atom or cell references as above, with the following invariants:

- A pointer from the stack allocation arena may point to any noun in the hash arena, including a subnoun whose hash is not explicitly stored.
- A pointer from the hash arena may point to a subnoun in its own allocation range.
- A pointer from the hash arena to another allocation range must point to the head of this allocation range. (3)
- A pointer from the hash arena may not point to the stack arena.

### Noun ejection

### Paging and snapshotting
