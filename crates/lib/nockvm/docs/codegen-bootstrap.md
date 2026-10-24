# New Mars: Codegen Bootstrapping

> Historical note. This document captures bootstrap design ideas, not current protocol or runtime authority.

New Mars provides an implementation of generating low-level, linear bytecode or machine code from Nock. This strategy is specified in Hoon, which is compiled to Nock. Thus, in order to generate low level code for Nock, we must execute Nock.

Further, we should have a way of upgrading the Nock code for code generation without baking a machine code, bytecode, or Nock blob into the New Mars runtime.

The proposed solution is to have the runtime provide a tree-walking Nock interpreter which can be used to run the Nock codegen code against itself, receiving a linearized version from which bytecode or machine code can be generated. This bytecode or machine code is then used to generate low level code for all other executed Nock, with the possible exception of interactively-proposed Nock (i.e. from the Dojo), which may still be executed by the tree-walking interpreter.

The runtime will ship with Nock code for code generation. Upgrades to this Nock code can be distributed two ways: along with upgrades to the runtime, and by a scry path from Arvo. If Arvo binds the next version of this designated scry path, the result is interpreted as an upgraded Nock formula for code generation.

## Bytecode

The quickest path to making use of linearization is a bytecode. The current vere already uses a bytecode, but instruction execution is not the current performance bottleneck.

The easiest approach to a bytecode is to make a compact and rapidly dispatchable encoding of the Nock linearizer output, which must encode the following instructions:

| Instruction | Description                          | Op  | Hex |
| ----------- | ------------------------------------ | --- | --- |
| `imm`       | Write an immediate noun to an SSA    | 1   | 1   |
| `mov`       | Copy one SSA to another              | 2   | 2   |
| `inc`       | Checked increment                    | 3   | 3   |
| `unc`       | Unchecked increment                  | 4   | 4   |
| `con`       | Create a cell                        | 5   | 5   |
| `hed`       | Checked head of a cell               | 6   | 6   |
| `tal`       | Checked tail of a cell               | 7   | 7   |
| `hud`       | Unchecked head of a cell             | 8   | 8   |
| `tul`       | Unchecked tail of a cell             | 9   | 9   |
| `clq`       | Branch on whether a noun is a cell   | 10  | A   |
| `eqq`       | Branch on whether nouns are equal    | 11  | B   |
| `brn`       | Branch on a loobean                  | 12  | C   |
| `hop`       | Direct jump internally in an arm     | 13  | D   |
| `cal`       | Call an arm in non-tail position     | 14  | E   |
| `lnk`       | Evaluate an arm in non-tail position | 15  | F   |
| `jmp`       | Call an arm in tail position         | 16  | 10  |
| `lnt`       | Evaluate an arm in tail position     | 17  | 11  |
| `spy`       | Execute a scry                       | 18  | 12  |
| `hnt`       | Provide a hint                       | 19  | 13  |
| `bom`       | Crash                                | 20  | 14  |

Thus we need 5 bits to encode instructions. The linearized nock IR is registerized in SSA form, meaning we need to either maintain a dynamic mapping of SSA registers to values or perform register allocation. To avoid having to encode arbitrary-length integers for registers, we choose the latter option.

Our bytecode VM will have 8 general-purpose registers, `r0` through `r7`.
When a value must be in a single, conventional register (i.e. for returns or for the subject for dynamic evaluation), that register is `r0`. Registers are encoded with 3 bits in an instruction. A register's value holds a noun. Reading from an uninitialized register results in undefined behavior. It is therefore permissible for debugging purposes, but not mandatory, to crash on a read from an uninitialized register. 

Register allocation is done using a Reverse Linear Scan algorithm. Register allocation begins at exit points of the functions. SSAs used by `don` and `lnt` are assigned to `r0` (and `r1` for the formula of LNT). A map of SSAs to register assignments is maintained, and entries are copied to implement and eliminate `mov` instructions. A stack of live registers, annotated with their SSAs, is maintained as well. When an SSA is defined, the corresponding register is removed from the stack. A stack of dead (unused) registers is also maintained, so that the most recently unused register is the one first selected for reallocation. In this way, terminal operations on an SSA will immediately re-use the VM register for their result.

When there are no available dead registers, then we must contract our live register set. This is done by "spilling" a currently live register to the stack.
We extend the SSA->register map to allow SSAs to be mapped to stack slots, and add stacks of live and dead stack slots. When there are no dead registers which may be allocated to an SSA at its use point, we select the register furthest down the live register stack, and either the stack slot at the top of the dead stack slot stack, or a fresh stack slot if all stack slots are live (or if we have not yet allocated any). We introduce a load from the selected slot to the selected register following the instruction which we are allocating for. We then update the mapping to point the corresponding SSA, no longer at the register, but at the stack slot.

If we encounter a use of an SSA mapped to a stack slot, we must allocate a register at that point for that SSA and introduce a load instruction. This may entail spilling a later-used register.

When we discover the definition of an SSA mapped to a stack slot, we allocate a register (which, again, may entail spilling) and introduce a `sto` operation of the SSA to that stack slot. 

Internal labels are encoded as flag bits, and then 13 or 48 bit integers representing byte offsets from the end of the instruction invoking the label. If the first bit of an offset is 0, then the offset represents a fall-through to the next instruction, useful for compact encoding of conditionals. If the first bit is 1, then the next bit selects either 13 or 48 bits for the label size.

External labels for arms are encoded as 45 bit integers, which should be shifted by 3 to the left and become pointers to bytecode buffers. **This requires the code table to guarantee pointer stability of bytecode buffers for arms.** Such pointers are looked up and compressed to 45 bits at bytecode generation time. **TODO** recursive arms.

Calls require a list of one to seven registers. (Calls which require 0 should be replaced by an immediate instruction.) To simplify the encoding, registers from `r0` to `r6` are used in order, and `mov` instructions are inserted to match parameters to the register allocator's assignments for the initial SSAs. If a call takes more than 8 parameters, the eighth and later parameters are consed into a list and passed in `r7`.   

Encoding of the begins with the 5 bit opcode, followed by the instruction's parameters encoded as described above and concatenated. The new 

## Codegen todos:

### Optimization: Shift some defines adjacent to uses

Register pressure will be reduced WLOP if we can shift memory load instructions to as near the uses of the loaded values as possible. In particular, it does no good if we load the head or tail of a cell, only to spill it to the stack later. 
A single use of a loaded value should always be immediately preceded by the load instruction. Multiple uses (perhaps unified by `mov`), if a spill does intervene, should replace the spill by repeated loads from the original memory location, to avoid an unnecessary memory write to the stack. See "Definition copying"

This also applies to immediates. Immediates should be shifted to immediately proceed their first use. An immediate instruction of an arbitrary noun carries a reference to the noun's fixed location in (virtual) memory, and so it is no extra cost to duplicate the immediate instruction instead of spilling it.

The `mov` instruction should also be shifted immediately prior to its first use. This will also ensure that it gets out of the way of the shift above.

### Optimization: Definition copying

If a spilled variable is a memory load or immediate, then a load instruction should not be written for the spill. Instead, the defining instruction should be copied to the locations where the spill would have been loaded from the stack slot. This would most easily be implemented by notating such SSAs with their defining instruction. For memory load instructions, this is only practical if we are still within the liverange of the register for the cell targeted by the load. Otherwise, we must allocate a register at precisely the point where we need to spill because of register pressure. Other encountered uses of the SSA, preceding the spill, may be able to extend the liverange of the cell if there are available dead registers at that point.
