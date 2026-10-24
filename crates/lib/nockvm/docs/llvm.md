# Compiling Nock to LLVM

> Historical note. This document captures LLVM codegen design notes, not current protocol or runtime authority.

Nock formulae are nouns, which are binary trees whose leaves are natural numbers.
The LLVM IR is a mostly sequential language comprised of operations arranged into basic blocks and functions.

In order to generate LLVM IR which executes a Nock formula, we can conceptually lower Nock to NockIR.
NockIR is a mostly-sequential language which contains a set of instructions optimized as a target for Nock.
It abstracts memory management and is intended to run with the VM's stack allocator.

## The NockIR VM
The machine semantically has three registers.
Two of these registers, `sub` and `res`, contain noun values.
The third, `pc`, contains a function pointer.

- `sub` contains the subject of the current Nock computation.
- `res` stores the result of a completed Nock computation.
- `pc` can semantically be read to saved on the stack, and set to jump.

It is important to note that we do not actually implement `pc` directly in LLVM, rather we create new functions at save points and tail call to jump.

The machine also has a stack with the number of "slots" in a frame specified per-frame when it is pushed.
Each slot can hold a noun, the first slot (slot 0) can optionally hold a function pointer.
We reference slots in the current frame as `frame[n]` where `n` is the 0-based index of the slot.

We denote the NockIR for a nock formula as `ir[x]` where `x` is a formula.

NockIR provides the following instructions:

| Instruction | Operation                                                       |
| ----------- | --------------------------------------------------------------- |
| `axe[x]`    | `res := /[x sub]`                                               |
| `cel[s]`    | `res := [frame[s] res]`                                         |
| `puh[x]`    | push a frame with x slots                                       |
| `pop`       | pop a frame                                                     |
| `put[s]`    | `frame[s] := res`                                               |
| `get[s]`    | `res := frame[s]`                                               |
| `sub`       | `sub := res`                                                    |
| `noc`       | `sub := /[2 res]; res := /[3 res]`                              |
| `sav[s]`    | `frame[s] := sub`                                               |
| `reo[s]`    | `sub := frame[s]`                                               |
| `con[x]`    | `res := x`                                                      |
| `clq`       | if res is a cell, `res := 0` else `res := 1`                    |
| `inc`       | `res := res + 1` (crash if cell)                                |
| `eqq[s]`    | if `frame[s] == res` then `res :=0` else `res := 1`             |
| `edt[x]`    | `res := #[x res sub]`                                           |
| `ext`       | `sub := [res sub]`                                              |
| `lnt`       | `pc := ir[res]`                                                 |
| `lnk`       | `frame[0] := pc; pc := ir[res]`                                 |
| `don`       | `pc = frame[0]`                                                 |
| `br0[x,y]`  | if `res` is 0, `pc = x`, if res is 1, `pc = y`, crash otherwise |
| `spy`       | External jump to a runtime-provided function producing a noun.  |
| `hns[b]`    | Look up a static hint in the hint table, jump if entry          |
| `hnd[b]`    | Look up a dynamic hint from res in the table, jump if entry.    |

The translation of Nock to NockIR takes the Nock formula plus two extra input bits, both of which are 0 if unspecified:

| Code generation        | Generated code                                                                |
| ---------------------- | ----------------------------------------------------------------------------- |
| `ir[0 0 [[b c] d]]`    | `puh[1]; ir[1 1 [b c]]; put[0]; ir[0 1 d]; cel[0]; pop; don`                  |
| `ir[s 1 [[b c] d]]`    | `puh[1]; ir[1 1 [b c]]; put[0]; ir[s 1 d]; cel[0]; pop`                       |
| `ir[0 0 [0 b]]`        | `axe[b]; don`                                                                 |
| `ir[s 1 [0 b]]`        | `axe[b]`                                                                      |
| `ir[0 0 [1 x]]`        | `con[x]; don`                                                                 |
| `ir[s 1 [1 x]]`        | `con[x]; don`                                                                 |
| `ir[0 0 [2 b c]]`      | `puh[1]; ir[1 1 c]; put[0]; ir[0 1 b]; cel[0]; pop; noc; lnt`                 |
| `ir[0 1 [2 b c]]`      | `puh[2]; ir[1 1 c]; put[1]; ir[0 1 b]; cel[1]; noc; lnk; pop`                 |
| `ir[1 1 [2 b c]]`      | `puh[2]; ir[1 1 c]; put[1]; ir[1 1 b]; cel[1]; sav[1]; noc; lnk; reo[1]; pop` |
| `ir[0 0 [3 b]]`        | `ir[0 1 b]; clq; don`                                                         |
| `ir[s 1 [3 b]]`        | `ir[s 1 b]; clq`                                                              |
| `ir[0 0 [4 b]]`        | `ir[0 1 b]; inc; don`                                                         |
| `ir[s 1 [4 b]]`        | `ir[s 1 b]; inc;`                                                             |
| `ir[0 0 [5 b c]]`      | `puh[1]; ir[1 1 b]; put[0]; ir[0 1 c]; eqq[0]; pop; don`                      |
| `ir[s 1 [5 b c]]`      | `puh[1]; ir[1 1 b]; put[0]; ir[s 1 c]; eqq[0]; pop`                           |
| `ir[s t [6 b c d]]`    | `ir[1 1 b]; br0[ir[s t c] ir[s t d]]`                                         |
| `ir[0 t [7 b c]]`      | `ir[0 1 b]; sub; ir[0 t c]`                                                   |
| `ir[1 1 [7 b c]]`      | `puh[1]; ir[0 1 b]; sav[0]; sub; ir[0 1 c]; reo[0]; pop`                      |
| `ir[0 t [8 b c]]`      | `ir[1 1 b]; ext; ir[0 t c]`                                                   |
| `ir[1 1 [8 b c]]`      | `puh[1]; ir[0 1 b]; sav[0]; ext; ir[0 1 c]; reo[0]; pop`                      |
| `ir[0 0 [9 b c]]`      | `ir[0 1 c]; sub; axe[b]; lnt`                                                 |
| `ir[0 1 [9 b c]]`      | `puh[1]; ir[0 1 c]; sub; axe[b]; lnk; pop`                                    |
| `ir[1 1 [9 b c]]`      | `puh[2]; sav[1]; ir[0 1 c]; sub; axe[b]; lnk; reo[1]; pop`                    |
| `ir[0 0 [10 [b c] d]]` | `puh[1]; ir[1 1 d]; put[0]; ir[0 1 c]; reo[0]; edt[b]; pop; don`              |
| `ir[0 1 [10 [b c] d]]` | `puh[1]; ir[1 1 d]; put[0]; ir[0 1 c]; reo[0]; edt[b]; pop;`                  |
| `ir[1 1 [10 [b c] d]]` | `puh[2]; sav[1]; ir[0 1 d]; put[0]; reo[1]; ir[0 1 c]; reo[0]; edt[b]; pop`   |
| `ir[s t [11 [b c] d]]` | `ir[1 1 b]; hnd[b]; ir[s t d]`                                                |
| `ir[s t [11 b c]]`     | `hns[b]; ir[s t c]`                                                           |
| `ir[0 0 [12 b c]]`     | `puh[1]; ir[1 1 b]; put[0]; ir[0 1 c]; cel[0]; spy; pop; don`                 |
| `ir[s 1 [12 b c]]      | `puh[1]; ir[1 1 b]; put[0]; ir[s 1 c]; cel[0]; spy; pop`                      |

## From NockIR to LLVM IR

NockIR is not intended to be generated separately. Rather, each NockIR instruction is implemented as a builder for some sequence of LLVM IR.

The registers for the memory allocator (the stack and frame pointers) and the VM (the subject and result) are implemented in the LLVM IR by making each basic block an LLVM function. Within a function, each mutation to a register results in a new SSA register, and the previous registers are not used after the new assignment.
Branching is accomplished by means of a conditional tail cail to basic blocks which contain static, unconditional jumps to the common suffix of the branch, or `don` instructions otherwise.

## Calling convention for basic blocks

We employ the `cc 10` convention, supplied for use by the Glasgow Haskell Compiler, as it matches our own needs.
This ensures no registers are spilled to the (LLVM) stack, that parameters are passed in registers, and that tail calls are always tail-call-optimized, i.e. compiled to jumps.

Each function representing a basic block, takes the current stack pointer, current frame pointer, current subject, and current value of the result register as arguments.

Instructions which need to be implemented as functions, such as `axe` or `pop`, take these 4 arguments, plus a function pointer to tail-call to continue the basic block.

## Calls and returns

The NockIR provides the `lnt` and `lnk` instructions for tail and non-tail calls, respectively.
The `lnt` instruction is relatively the simpler: it invokes the runtime system to generate or fetch cached code for the noun in the `res` register, then executes a dynamic jump to this code.
The `lnk` instruction will be followed by another basic block, thus, it first saves the function pointer for that basic block in `frame[0]`; and then jumps to the code in question.
The `don` instruction simply looks up the function pointer in `frame[0]` and jumps to it.
Thus the outer frame for a computation installs a handler, matching the calling convention for basic blocks, which returns the result noun to the runtime system.

## Instructions

### `axe`

The `axe` instruction looks up an axis in the subject and places it in the result register.
It will nock crash if the axis is not valid for the noun in the subject register.

### `cel`

Creates a cell by allocating on the stack, with head from the given stack frame slot, and tail from the result register.
Cell is placed in the result register.

### `puh`

Pushes a new frame with a given number of slots onto the stack

### `pop`
Pops a frame from the stack. This will cause a copy of stack-allocated nouns from within the current frame,
traced from the result register as a root, so that the result register remains valid.

The noun in the result register will remain the same, but since it must be copied up the stack, the memory
representation may change

### `put`
Save the noun in the result register to a frame slot.

### `get`
Load the noun in a frame slot to the result register.

### `sub`
Set the subject register to the noun in the result register.

### `noc`
Set the subject register to the head of the cell in the result register

**This operation assumes that the result register is a cell, and its behavior is undefined (e.g. not a nock crash)
if the result register is not a cell.**

### `sav`
Save the noun in the subject register to a frame slot.

### `reo`
Load a noun from a frame slot to the subject register

### `con`
Load a noun immediate into the result register

### `clq`
If the result register is a cell, set the result register to 0, else set it to 1

### `inc`
If the result register is an atom, increment it by one. If it is a cell, nock crash

### `eqq`
Unifying equality between the noun in the given slot and the noun in the result register.
Result register set to `0` if equal and `1` if not equal

### `edt`
Edit the noun in the subject register by replacing the subnoun at the given axis with the noun in the result register.
Place the resulting noun in the result register (subject register is unmodified).

Nock crash if axis is not valid for the noun.

### `ext`
Allocate a cell with the noun in the result register as a head, and the noun in the subject register as a tail.
Place the cell in the subject register.

### `lnt`
Codegen and evaluate the noun in the result register with the current subject, in tail position.

### `lnk`
Codegen and evaluate the noun in the result register, returning to the instruction following `lnk`

### `don`
Return to just past the calling `lnk` instruction or the outer virtualization context.

### `br1`
Continue if the result register is 0, branch to the given label if it is 1. Nock crash otherwise
