# Notes ~2021.9.24

> Historical note. Archived design notes, not current protocol or runtime authority.

## Exploration of `+mink`
The [`+mink`](https://github.com/urbit/urbit/blob/fa894b9690deae9e2334ccec5492ba90cb0b38f9/pkg/arvo/sys/hoon.hoon#L5978-L6106)
arm in [`hoon.hoon`](https://github.com/urbit/urbit/blob/master/pkg/arvo/sys/hoon.hoon) is a metacircular Nock interpreter
intended to be jetted by invoking the host Nock interpreter. In addition to the subject and formula to evaluate, `+mink`
takes a scry gate which is used to evaluate nock 12 (scry) formulas.

The jet uses `u3r_mean` to take apart the sample of the `+mink` gate and feeds the resulting nouns to 
`u3n_nock_et`, which runs the interpreter and produces a 'toon', 

Scry gates are kept in a stack because a scry gate may itself scry, and should not re-enter itself.

## Exceptions in new mars
### Suboptimal but simple way
The simple thing to do would be to make every continuation expect a toon and on an error or block result, immediately call
the next continuation up the stack with it, until a continuation installed e.g. by the +mink jet branched on the result.
This is effectively writing the interpreter as a CPS/trampoline translation of +mink.

### Likely more optimal way
It should be possible to store exception contexts in the hash table: comprising stack pointers for unwinding,
code pointer to jump to (possibly we could use setjmp/longjmp instead), and a hash reference to the next outermost
handler. The hash reference to the current exception context would be held in a register. This structure could also hold
the current scry gate.
