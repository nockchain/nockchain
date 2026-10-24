# Subject Knowledge Analysis

> Historical note. This document captures SKA design exploration, not current protocol or runtime authority.

Nock is inherently a very dynamic language.
Nock has no concept corresponding directly to a procedure call, only to an "eval" of a dynamically-computed noun as a Nock formula.
Actual procedure calls are implemented by looking up a noun corresponding to a formula from a core (a pair of a "battery" of Nock formulas and a pair of "sample" and context), and evaluating that formula with the core as the subject.
This is the semantics of Nock 9.

This presents a very difficult challenge for jetting and code generation of Nock, as in general the semantics of any formula can depend arbitrarily on its subject.
However, it is not sufficient to also fix the subject, as the subject must vary to store state and allow a core to receive input (e.g. a "gate" or one-armed core is "slammed" by altering the sample prior to invoking the arm of the core).
In general, we need to know which parts of the subject are fixed and which can vary while retaining the correspondence of a formula to a jet or to generated code.

## Partial nouns
The subject of any Nock computation is a noun.
In order to do any analysis which partially fixes the subject, we must have a partial representation of nouns.
We can represent this as a Hoon type, which we denote `sock`:

```
|%
+$  sock
  $%  [%know k=*]
      [%bets p=sock q=sock]
      [%dice ~]
      [%gues ~]
  ==
--
```

The `%know` case represents full knowledge of a noun, and simply records the noun.
The `%bets` case records partial knowledge of a cell, recursively described by the `p` (head) and `q` (tail) faces.
The `%dice` case records knowledge that a noun is an atom, but without knowledge of its value.
The `%gues` case records the absence of knowledge of a noun, it could be any noun whatsoever.

A `sock` can be recursively normalized: a `%bets` cell consisting of two `%know` nouns normalizes to a `%know` of the cell.
Other cases can be optimized but this throws away structural information.

The object of our analysis is to discover such a partial noun representing what can be known at the subject during a particular Nock computation.

## Subject Knowledge Analysis
**TODO: safety analysis**

Subject Knowledge Analysis (SKA) is meant to run alongside code generation and produce a representation of what is known of the subject at each point as code generation traverses the Nock formula.
Thus, intermediate results should not be stored, and the analysis does not add an extra iteration of the tree. Rather, it describes how knowledge propagates through the Nock formula tree.

The *input* to each analysis is the knowledge of the subject, and the *output* is knowledge of the result, given the input subject knowledge.
This may seem counterintuitive, but the algorithm describes how this knowledge is propagated such that it may be used by code generation, so it describes how the input to successive traversal steps is generated.

The Nock primitives `/[a b]` and `#[[a b] c]` extend to socks in the obvious way, giving a crash when it can be *known* that the computation would crash, e.g. when attempting to access a non-1 axis of an atom, or
when an axis is not an atom.

Note that while Subject Knowledge Analysis can often determine that a computation *will* crash, it cannot presently determine that a computation *will* not crash.
Thus, the knowledge of the result of a computation is knowledge of the result assuming the computation does not crash.
Thus, for instance, knowledge of the result of `4` where the subformula result is `[%gues ~]` is `[%dice ~]`, despite the fact that the subformula could produce a cell and cause the computation to crash.
Put another way, so long as a `4` does not crash, it will always produce an atom, and thus the knowledge produced is `[%dice ~]`.

### Cell
SKA of a cell formula applies the analysis to both formulae with the same subject knowledge input, then combines their inputs into a `%bets` cell and normalizes the result.

### 0
SKA of 0 applies the `/` primitive as far as it can be applied.

### 1
SKA of 1 returns `%know` of the supplied constant, ignoring the input.

### 2
SKA of 2 analyzes the second subformula first. If it is `%know` of some noun, then the first subformula is analyzed and its result used as the subject to continue the analysis into the known formula.
If the formula isn't known, then the result is `[%gues ~]`, though the first subformula may still be analyzed for code-generation purposes.

### 3
SKA of 3 recursively analyzes the subformula, then examines what is known of the structure of the noun. Specifically, for the %know, %bets, or %dice cases, it is known whether the result is a cell or an atom, and thus
the analysis can produce `[%know 0]` for a known cell or `[%know 1]` for a known atom. For the `[%gues ~]` result, however, we must produce `[%dice ~]`

### 4
SKA of 4 *intentionally* does not produce the obvious result, in order to short-circuit full analysis of functions which should be jetted.
If the result of the subformula is known to be a cell, we can produce knowledge of a crash.
Otherwise the result is `[%dice ~]` whether or not the atom result of the subformula is known.
This permits memoization to prevent analysis of recursive Nock 4 from degenerating into full evaluation of a function.

### 5
SKA of 5 analyzes both subformulas.
If the results of both are fully known, then `[%know 0]` or `[%know 1]` can be returned for equality or inequality.
If either result is not fully known, then the result is `[%dice ~]`

### 6
SKA of 6 first analyzes the first (test) subformula.
If the result is known to be 0, it must only analyze the second (true-branch) subformula.
If the result is known to be 1, it must only analyze the second (false-branch) subformula.
If the result is known to be some other atom, or a cell, it must produce knowledge of a crash.
Otherwise, the result is the _intersection_ of the knowledge produced by analyzing both branches.

The intersection of two nouns produces knowledge where they agree, and ignorance where they disagree.
This is true for both structure and content. See _Sock intersection_ below for the full semantics.

### 7
SKA of 7 analyzes the first subformula, then uses its result as the subject input to the analysis of the second formula.

### 8
SKA of 8 is similar to SKA of 7 but the result of the subject formula must then be consed with the input subject, in the manner described for cell formulas.

### 9
SKA of 9 analyzes the subformula, then applies the `/` primitive as in 0.
If the result is known, we can continue to analyze the resulting noun as a formula, using the computed core as a subject.
Otherwise we must simply return `[%gues ~]`.

### 10
SKA of 10 analyzes both the tree and patch subformulae, and returns a result representing the partial knowledge of the patch inserted into the partial knowledge of the tree.

### 11
SKA of 11 may analyze dynamic hints, but does not strictly need to as SKA is not required to inform of crashes.

### 12
SKA treets all Nock 12 computations as returning `[%gues ~]`

## Operations on socks

### Cell construction
When analyzing a Nock computation, it may be necessary to combine partial knowledge of two nouns into partial knowledge of a cell constructed from those two nouns.
This is done by creating a `%bets` sock combining the two and then normalizing.

### Axis lookup
Looking up an axis into a sock should return whatever can be known about what looking up an axis into the full noun would return.
The case for `%know` is trivial, and `%bets` likewise has an obvious interpretation.
Looking up a non-1 axis into a known atom or into a `[%dice ~]` sock should produce knowledge of a crash.
Looking up an axis into `[%gues ~]` should produce `[%gues ~]`

### Edit
Editing a sock at an axis involves constructing the path to the patch (described by the edit axis) from `%bets` cells, the last of which has as its head or tail (depending on the axis) the patch sock.
Heads or tails at other levels are straightforwardly constructed from the sock being edited.
Descending into a `[%dice ~]` case produces knowledge of a crash.
Descending into a `[%gues ~]` case causes each branch off the path to the path to  also be `[%gues ~]`.

### Sock intersection
Intersecting two socks produces a sock with knowledge where the intersected socks agree, and ignorance where one represents ignorance or the socks disagree.
Intersection descends into `%know` cases in order to produce the maximum degree of agreed-upon knowledge.
