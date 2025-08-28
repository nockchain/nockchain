/=  ztd-six  /common/ztd/six
=>  ztd-six
~%  %table-lib  ..fri-door  ~
::    table-lib
|%
::
+|  %jute-types
+$  jute-data  [name=@tas sam=tree-data prod=tree-data]
+$  jute-map  (map @tas @)
+$  atom-data  [register=@tas axis=@ interim-mem-set=(unit @tas)]
+$  register-map  (map @tas felt)
+$  encoder  [a=mp-graph c=mp-graph]
::
++  one-poly  ^~((mp-c:mp-to-graph 1))
++  zero-poly  ^~((mp-c:mp-to-graph 0))
::
+$  jute-func-map  (map @tas jute-funcs)
::
::  +jute-funcs: jute interface
++  jute-funcs
  $+  jute-funcs
  $_  ^|
  |%
  ::
  ++  atoms
    ^-  (list atom-data)
    ~
  ::  compute the actual jet corresponding to this jute
  ++  compute
    |~  sam=*
    ^-  *
    0
  ::
  ++  u8s
    |~  jute-info=[name=@tas sam=* prod=*]
    ^-  (list belt)
    ~
  ::
  ::  write base row of the jute table for this jute
  ++  build
    |~  jute-info=[name=@tas sam=* prod=*]
    ^-  (map @tas felt)
    ~
  ::
  ++  u8-msets
    |~  jute-info=[name=@tas sam=* prod=*]
    ^-  (list @tas)
    ~
  ::
  ::  write extension columns for this jute
  ++  extend
    |~  [=jute-data challenges=(list felt)]
    ^-  (map @tas felt)
    ~
  ::
  ::  transition constraints for this jute
  ++  transition-constraints
    |~  vars=(map term mp-graph)
    *(map term mp-graph)
  --
::
+|  %table-types
+$  row       bpoly
+$  belt-row  (list belt)
+$  matrix    (list row)
::
+$  col-name  term  ::  name of a column
+$  header    $:  name=term     ::  name of the table
                  field=@       ::  cardinality of the field that the table is defined over
                  base-width=@  ::  base number of columns
                  ext-width=@  ::  number of extension columns (doesn't include base)
                  mega-ext-width=@  ::  number of mega-extension columns
                  full-width=@
                  num-randomizers=@
              ==
::  $table: a type that aspirationally contains all the data needed for a table utilized by the prover.
+$  table   [header p=matrix]
+$  table-mary   [header p=mary]
+$  table-dat  (trel table-mary table-funcs verifier-funcs)
::
::
::  the following $matrix type validates that the length of each row is the same. this hurts
::  performance, and eventually we will move towards a more efficient memory allocation anyways,
::  so it is commented out. but you can still use it for debugging by uncommenting it and commenting
::  the $matrix entry above.
::  +$  matrix  $|  (list row)
::              |=  a=(list row)
::              |-
::              ?~  a  %.y
::              ?~  t.a  %.y
::              ?:  =(len.i.a len.i.t.a)
::                $(a t.a)
::              %.n
::
::
::    interfaces implemented by each table
+|  %table-interfaces
::
::  +static-table-common: static table data shared by everything that cares about tables
::    TODO either the static parts of the jute table should implement this, or
::    dynamic tables need their own interface entirely
++  static-table-common
  $+  static-table-common
  $_  ^|
  |%
  ::  +name: name of the table
  ::  +basic-column-names: names for base columns as terms
  ::  +ext-column-names: names for extension columns as terms
  ::  +column-names: names for all columns as terms
  ++  name  *term
  ++  column-names
    *(list col-name)
  ++  basic-column-names
    *(list col-name)
  ++  ext-column-names
    *(list col-name)
  ++  mega-ext-column-names
    *(list col-name)
  ++  variables
    *(map col-name mp-mega)
  ++  terminal-names
    *(list col-name)
  --
::
++  table-funcs
  $+  table-funcs
  $_  ^|
  |%
  ::
  ::  +build: Build the table (Algebraic Execution Trace) for a given nock computation.
  ::
  ::    The returned table is a mary of its rows. The step of the mary
  ::    is the number of columns while its length is the number of rows.
  ::
  ++  build
    |~  fock-meta=fock-return
    *table-mary
  ::
  ::  +extend: Returns extension columns for first pass of challenges
  ::
  ::    The columns are commitments to the validity of the ION fingerprints
  ::
  ++  extend
    |~  [table=table-mary challenges=(list belt) fock-meta=fock-return]
    *table-mary
  ::
  ::  +mega-extend: Returns extension columns for the second pass of challenges
  ::
  ++  mega-extend
    |~  [table=table-mary challenges=(list belt) fock-meta=fock-return]
    *table-mary
  ::
  ::  +pad: include extra rows until their number equals the next power of two
  ::
  ::    If the height of the table is 6, then it will be padded to 8.
  ::    If the height of the table is already a power of 2, it will remain
  ::    the same, e.g pad(8) = 8.
  ::
  ++  pad
    |~  table=table-mary
    *table-mary
  ::
  ::  +terminal: produce a map of felts sourced from the final row of the built and extended table
  ::
  ::    The output of this is made available to other tables in the terminal-constraints
  ::    arm, where tables can specify constraints to interrelate columns by
  ::    e.g. forcing terminal values of a multiset to be equal.
  ::
  ++  terminal
    |~  =table-mary
    *bpoly
  ::
  ++  boundary-constraints    boundary-constraints:*verifier-funcs
  ++  row-constraints         row-constraints:*verifier-funcs
  ++  transition-constraints  transition-constraints:*verifier-funcs
  ++  terminal-constraints    terminal-constraints:*verifier-funcs
  ++  extra-constraints       extra-constraints:*verifier-funcs
  --
::
++  verifier-funcs
  ::  the verifier interface is a strict subset of the prover interface because verify:nock-verifier only uses constraints
  $+  verifier-funcs
  $_  ^|
  |%
  ::
  ::  apply to the first row only
  ++  boundary-constraints
    *(map term mp-ultra)
  ::
  ::  apply to a single row
  ++  row-constraints
    *(map term mp-ultra)
  ::
  ::  apply to adjacent row-pairs
  ++  transition-constraints
    *(map term mp-ultra)
  ::
  ::  apply to the final row only
  ++  terminal-constraints
    *(map term mp-ultra)
  ::
  ::  apply to extra composition poly only
  ++  extra-constraints
    *(map term mp-ultra)
  --
::
+|  %cores
::
++  dyn
  ~/  %dyn
  |%
  ++  c
    |=  [nam=term tail=(list term)]
    ^-  (list term)
    :^    (crip (weld (trip nam) "-a"))
        (crip (weld (trip nam) "-b"))
      (crip (weld (trip nam) "-c"))
    tail
  ::
  ++  terminal-names
    ^-  (list term)
    %+  c  %compute-terminal-one
    %+  c  %memory-terminal-one
    ~
  ::
  ++  num-terms     (lent terminal-names)
  ::
  ++  make-dyn-mps
    ~/  %make-dyn-mps
    |=  [dyn-names=(list term)]
    ^-  (map term mp-mega)
    ~+
    %-  ~(gas by *(map term mp-mega))
    %+  iturn  dyn-names
    |=  [i=@ nam=term]
    [nam (mp-dyn:mp-to-mega i)]
  ::
  ++  dyn
    |_  terms=(map term mp-mega)
    ++  d
      |=  nam=term
      ^-  mp-pelt
      =/  nam-a  (crip (weld (trip nam) "-a"))
      =/  nam-b  (crip (weld (trip nam) "-b"))
      =/  nam-c  (crip (weld (trip nam) "-c"))
      ?>  (~(has by terms) nam-a)
      ?>  (~(has by terms) nam-b)
      ?>  (~(has by terms) nam-c)
      :+  (~(got by terms) nam-a)
        (~(got by terms) nam-b)
      (~(got by terms) nam-c)
    --
  ::
  ++  pdyn
    |_  terms=(map term belt)
    ++  r
      |=  nam=term
      ^-  belt
      ~|  missing-terminal+nam
      (~(got by terms) nam)
    --
  ::
  ++  make-dynamic-map
    ~/  %make-dynamic-map
    |=  raw-terms=(list belt)
    ^-  (map @ belt)
    ~+
    ~|  "make sure that you are passing in the right number of raw terminals, especially in your test arm"
    ?>  (gte (lent raw-terms) num-terms)  ::  ensure we have enough terminals
    =/  raw-terms  (scag num-terms raw-terms)
    %-  ~(gas by *(map @ belt))
    %+  iturn  raw-terms
    |=  [i=@ term=belt]
    [i term]
  --  ::  dyn
::
++  chal  ::  /lib/challenges
  ~/  %chal
  |%
  ++  c
    |=  [nam=term tail=(list term)]
    ^-  (list term)
    :^    (crip (weld (trip nam) "-a"))
        (crip (weld (trip nam) "-b"))
      (crip (weld (trip nam) "-c"))
    tail
  ::
  ++  chal-names-rd1
    ^-  (list term)
    %+  c  %a    :: tuple packing for ions
    %+  c  %b
    %+  c  %c
    %+  c  %d
    %+  c  %e
    %+  c  %f
    %+  c  %g
    %+  c  %p    :: tuple packing for other tuples
    %+  c  %q
    %+  c  %r
    %+  c  %s
    %+  c  %t
    %+  c  %u
    %+  c  %alf  :: stack
    ~
  ::
  +$  ext-chals
    $:  a=pelt-chal:constraint-util
        b=pelt-chal:constraint-util
        c=pelt-chal:constraint-util
        d=pelt-chal:constraint-util
        e=pelt-chal:constraint-util
        f=pelt-chal:constraint-util
        g=pelt-chal:constraint-util
        p=pelt-chal:constraint-util
        q=pelt-chal:constraint-util
        r=pelt-chal:constraint-util
        s=pelt-chal:constraint-util
        t=pelt-chal:constraint-util
        u=pelt-chal:constraint-util
        alf=pelt-chal:constraint-util
    ==
  ::
  ++  init-ext-chals
    |=  challenges=(list belt)
    ^-  ext-chals
    =/  r
      %-  make-pelt-chal:constraint-util
      ~(r prnd:chal (bp-zip-chals-list:chal chal-names-rd1:chal challenges))
    :: alf for stacks, bet for multisets, z for kv store, a-g for tuples
    :*  (r %a)
        (r %b)
        (r %c)
        (r %d)
        (r %e)
        (r %f)
        (r %g)
        (r %p)
        (r %q)
        (r %r)
        (r %s)
        (r %t)
        (r %u)
        (r %alf)
    ==
  ::
  ++  chal-names-rd2
    ^-  (list term)
    %+  c  %j
    %+  c  %k
    %+  c  %l
    %+  c  %m
    %+  c  %n
    %+  c  %o
    %+  c  %w
    %+  c  %x
    %+  c  %y
    %+  c  %z    :: key-value store
    %+  c  %bet  :: op0 multiset
    %+  c  %gam  :: decoder multiset
    ~
  ::
  ++  chal-names-basic
    ^-  (list term)
    (weld chal-names-rd1 chal-names-rd2)
  ::
  +$  mega-ext-chals
    $:  a=pelt-chal:constraint-util
        b=pelt-chal:constraint-util
        c=pelt-chal:constraint-util
        d=pelt-chal:constraint-util
        e=pelt-chal:constraint-util
        f=pelt-chal:constraint-util
        g=pelt-chal:constraint-util
        p=pelt-chal:constraint-util
        q=pelt-chal:constraint-util
        r=pelt-chal:constraint-util
        s=pelt-chal:constraint-util
        t=pelt-chal:constraint-util
        u=pelt-chal:constraint-util
        alf=pelt-chal:constraint-util
        j=pelt-chal:constraint-util
        k=pelt-chal:constraint-util
        l=pelt-chal:constraint-util
        m=pelt-chal:constraint-util
        n=pelt-chal:constraint-util
        o=pelt-chal:constraint-util
        w=pelt-chal:constraint-util
        x=pelt-chal:constraint-util
        y=pelt-chal:constraint-util
        z=pelt-chal:constraint-util
        bet=pelt-chal:constraint-util
        gam=pelt-chal:constraint-util
    ==
  ::
  ++  init-mega-ext-chals
    |=  challenges=(list belt)
    ^-  mega-ext-chals
    =/  r
      %-  make-pelt-chal:constraint-util
      ~(r prnd:chal (bp-zip-chals-list:chal chal-names-basic:chal challenges))
    :: alf for stacks, bet for multisets, z for kv store, a-g for tuples
    :*  (r %a)
        (r %b)
        (r %c)
        (r %d)
        (r %e)
        (r %f)
        (r %g)
        (r %p)
        (r %q)
        (r %r)
        (r %s)
        (r %t)
        (r %u)
        (r %alf)
        (r %j)
        (r %k)
        (r %l)
        (r %m)
        (r %n)
        (r %o)
        (r %w)
        (r %x)
        (r %y)
        (r %z)
        (r %bet)
        (r %gam)
    ==
  ::
  ++  chals-derived
    ^-  (list term)
    ::  for now we just have inverses but other could be added if needed
    %+  c  %inv-alf
    %+  c  %input
    ~
  ::
  ++  chal-names-all
    ^-  (list term)
    (weld chal-names-basic chals-derived)
  ::
  ++  num-chals     (lent chal-names-basic)
  ::
  ++  num-chals-rd1  (lent chal-names-rd1)
  ::
  ++  num-chals-rd2  (lent chal-names-rd2)
  ::
  ::
  ::  +rnd: randomness symbols for use in constraint graphs
  ::
  ++  make-chal-mps
    ~/  %make-chal-mps
    |=  [chal-names=(list term)]
    ^-  (map term mp-mega)
    ~+
    %-  ~(gas by *(map term mp-mega))
    %+  iturn  chal-names
    |=  [i=@ nam=term]
    [nam (mp-chal:mp-to-mega i)]
  ::
  ++  rnd
    |_  chals=(map term mp-mega)
    ++  r
      |=  nam=term
      ^-  mp-pelt
      =/  nam-a  (crip (weld (trip nam) "-a"))
      =/  nam-b  (crip (weld (trip nam) "-b"))
      =/  nam-c  (crip (weld (trip nam) "-c"))
      ?>  (~(has by chals) nam-a)
      ?>  (~(has by chals) nam-b)
      ?>  (~(has by chals) nam-c)
      :+  (~(got by chals) nam-a)
        (~(got by chals) nam-b)
      (~(got by chals) nam-c)
    --
  ::
  ::  +prnd: populated randomness felt values for use in extending a table
  ++  prnd
    |_  chals=(map term belt)
    ++  r
      |=  nam=term
      ^-  belt
      ~|  missing-challenge+nam
      (~(got by chals) nam)
    --
  ::
  ++  make-challenge-map
    ~/  %make-challenge-map
    |=  [raw-chals=(list belt) [s=* f=*]]
    ^-  (map @ belt)
    ~+
    ~|  "make sure that you are passing in the right number of raw challenges, especially in your test arm"
    ?>  (gte (lent raw-chals) num-chals)  ::  ensure we have enough random values
    =/  raw-chals  (scag num-chals raw-chals)
    =/  named-chals
      %-  ~(gas by *(map term belt))
      (zip-up chal-names-basic raw-chals)
    =/  a    (got-pelt named-chals %a)
    =/  b    (got-pelt named-chals %b)
    =/  c    (got-pelt named-chals %c)
    =/  alf  (got-pelt named-chals %alf)
    =/  inv-alf
      (pinv (got-pelt named-chals %alf))
    =/  input-ifp
      =-  (compress-pelt ~[a b c] ~[size dyck leaf]:-)
      (build-tree-data:constraint-util s alf)
    =.  raw-chals
      %+  weld  raw-chals
      :~  (~(snag bop [3 inv-alf]) 0)
          (~(snag bop [3 inv-alf]) 1)
          (~(snag bop [3 inv-alf]) 2)
          (~(snag bop [3 input-ifp]) 0)
          (~(snag bop [3 input-ifp]) 1)
          (~(snag bop [3 input-ifp]) 2)
      ==
        ::~[a.inv-alf b.inv-alf c.inv-alf]
        ::~[a.input-ifp b.input-ifp c.input-ifp]
    %-  ~(gas by *(map @ belt))
    %+  iturn  raw-chals
    |=  [i=@ chal=belt]
    [i chal]
  ::
  ::  Compute derived challenges
  ++  augment-challenges
    ~/  %augment-challenges
    |=  [raw-chals=(list belt) [s=* f=*]]
    ^-  bpoly
    ~+
    ~|  "make sure that you are passing in the right number of raw challenges, especially in your test arm"
    ?>  (gte (lent raw-chals) num-chals)  ::  ensure we have enough random values
    =/  raw-chals  (scag num-chals raw-chals)
    =/  named-chals
      %-  ~(gas by *(map term belt))
      (zip-up chal-names-basic raw-chals)
    =/  a    (got-pelt named-chals %a)
    =/  b    (got-pelt named-chals %b)
    =/  c    (got-pelt named-chals %c)
    =/  alf  (got-pelt named-chals %alf)
    =/  inv-alf
      (pinv (got-pelt named-chals %alf))
    =/  input-ifp
      =-  (compress-pelt ~[a b c] ~[size dyck leaf]:-)
      (build-tree-data:constraint-util s alf)
    %-  init-bpoly
    %+  weld  raw-chals
    :~  (~(snag bop [3 inv-alf]) 0)
        (~(snag bop [3 inv-alf]) 1)
        (~(snag bop [3 inv-alf]) 2)
        (~(snag bop [3 input-ifp]) 0)
        (~(snag bop [3 input-ifp]) 1)
        (~(snag bop [3 input-ifp]) 2)
    ==
  ::
  ++  zip-chals
    ~/  %zip-chals
    |=  [names=(set term) raw-chals=(list felt)]
    (~(gas by *(map term felt)) (zip ~(tap in names) raw-chals same))
  ::
  ++  bp-zip-chals
    ~/  %bp-zip-chals
    |=  [names=(set term) raw-chals=(list belt)]
    (~(gas by *(map term belt)) (zip ~(tap in names) raw-chals same))
  ::
  ++  bp-zip-chals-list
    ~/  %bp-zip-chals
    |=  [names=(list term) raw-chals=(list belt)]
    (~(gas by *(map term belt)) (zip names raw-chals same))
  --  ::challenges
::
++  terminal  ::  /lib/terminal  ::TODO this is a core with one arm - move elsewhere?
  =,  mp-to-graph
  ~%  %terminal  ..terminal  ~
  |%
  ::  +gen-consistency-checks: assist with validating the terminals map
  ::
  ::    the purpose of this arm is to assist with validating the terminals map in sam of terminal-constraints
  ::
  ::    the terminals map is out-of-band data, i.e., it is not constrained and is to be considered untrusted
  ::    thus, we need to manually ensure, per table, that all values in the inner map are correct.
  ::
  ::    we also need to assert that the list of column names (inner map's keys) are a (non-strict) subset
  ::    of the full column names of the table
  ::    we will generate equality constraints for each
  ::
  ::    extraneous-columns := terminal-columns - all-columns
  ++  gen-consistency-checks
    ~/  %gen-consistency-checks
    |=  [our-terminals=(map term felt) our-columns=(list term) v=$-(@tas mp-graph)]
    ^-  (list mp-graph)
    =/  terminal-columns    ~(key by our-terminals)
    =/  all-columns         (~(gas in *(set col-name)) our-columns)
    =/  extraneous-columns  (~(dif in terminal-columns) all-columns)
    ?>  =(~ extraneous-columns)
    %+  turn  ~(tap in terminal-columns)
    |=  col=@tas
    ^-  mp-graph
    =/  term-val  (~(got by our-terminals) col)
    ::  terminal constraint:
    ::
    ::    <col> = <terminal-val>,
    ::    <col> - <terminal-val> = 0
    ::    for all col in table columns names
    ::    for all terminal-val in claimed terminal values received OOB for this table
    ::
    (mpsub (v col) (mp-c term-val))
  --  ::terminal
::
++  tlib  ::  /lib/table
  ~/  %tlib
  =/  util  constraint-util  ::TODO =/ vs =*?
  |%
  ::
  ::  +height: target padding height for given table is the next smallest power of 2
  ::
  ::    e.g. if (lent p.table) == 5, (height table) == 8
  ++  height
    ~/  %height
    |=  =table
    ^-  @
    ~+
    =/  len  (lent p.table)
    ?:  =(len 0)  0
    (bex (xeb (dec len)))
  ::
  ++  height-mary
    ~/  %height-mary
    |=  p=mary
    ^-  @
    ~+
    =/  len  len.array.p
    ?:  =(len 0)  0
    (bex (xeb (dec len)))
  ::
  ++  weld-exts
    ~/  %weld-exts
    |=  [l=table-mary r=table-mary]
    ^-  table-mary
    ?>  =(-.l -.r)
    :-  -.l
    (~(weld-step ave p.l) p.r)
  ::
  ::  TODO make some of these functions a door
  ++  table-to-verifier-funcs
    ~/  %table-to-verifier-funcs
    ::  this arm is theoretically a temporary shim to assist the separation of the codebase
    ::  ideally this disappears after sufficient surgery but may be here to stay
    |=  fs=table-funcs
    ^-  verifier-funcs
    |%
    ++  boundary-constraints    boundary-constraints:fs
    ++  row-constraints         row-constraints:fs
    ++  transition-constraints  transition-constraints:fs
    ++  terminal-constraints    terminal-constraints:fs
    ++  extra-constraints       extra-constraints:fs
    --
  ::
  ::  (representing the state of the computation at each step), but rather
  ::  a list of those base elements *lifted* into extension field elements.
  ::
  ::  Thus, fpoly is being used to represent a generic list of felts
  ::  not any specific polynomial with felt coefficients.
  ::
  ::  TODO the following 2 arms could go into the tab door and be renamed shorter
  ++  belt2d-to-matrix
    ~/  %belt2d-to-matrix
    |=  btable=(list (list belt))
    ^-  matrix
    (turn btable lift-to-fpoly)
  ::
  ++  matrix-to-belt2d
    ~/  %matrix-to-belt2d
    |=  mat=matrix
    ^-  (list (list))
    (turn mat row-to-belts)
  ::
  ++  row-to-belts
    ~/  %row-to-belts
    |=  row=row
    ^-  (list)
    (turn ~(to-poly fop row) drop)
  ::
  ::  +var: helper door. allows for terse `(v %idx)` style variable accesses
  ::
  ::    after initializing with a variables map
  ::
  ++  var
    |_  variables=(map term mp-mega)
    ++  v
      |=  nam=term
      ^-  mp-mega
      ~+
      ~|  var-not-found+nam
      (~(got by variables) nam)
    --
  ++  var-pelt
    |_  variables=(map term mp-mega)
    ++  v
      |=  nam=term
      ^-  mp-pelt
      ~+
      ~|  var-not-found+nam
      :+  (~(got by variables) (crip (weld (trip nam) "-a")))
        (~(got by variables) (crip (weld (trip nam) "-b")))
      (~(got by variables) (crip (weld (trip nam) "-c")))
    ::
    ++  v-n
      |=  nam=term
      ^-  mp-pelt
      ~+
      ~|  var-not-found+nam
      :+  (~(got by variables) (crip (weld (trip nam) "-a-n")))
        (~(got by variables) (crip (weld (trip nam) "-b-n")))
      (~(got by variables) (crip (weld (trip nam) "-c-n")))
    ::
    ++  c
      |=  nam=term
      ^-  mp-pelt
      ~+
      ~|  var-not-found+nam
      =/  mp  (~(got by variables) nam)
      [mp mp mp]
    --
  ::
  ::  +make-vars:
  ::
  ::    given a list of variable names (i.e., column names),
  ::    produce a map from variable names to corresponding mp-graph
  ++  make-vars
    ~/  %make-vars
    |=  [var-names=(list col-name)]
    |^  ^-  (map col-name mp-mega)
    ~+
    ::
    ::  Equivalent to:
    ::
    ::  %-  ~(gas by (map term mp-graph:f))
    ::  :~  [%idx (make-variable 0)]
    ::      [%a (make-variable 1)]
    ::      ...
    ::      [%a-n (make-variable 2*n)]
    ::  ==
    ::
    =/  num-succ  1
    ::  is the number of successors to generate
    ::  hardcoded to 1 because that is all the current stark impl supports
    ::  i.e., cannot have constraints of the form idx'' = idx' + 1
    ::
    =/  successor-names=(list col-name)
      ::  flat list of succesors ~[%idx-n %a-n %idx-n-n %a-n-n]
      %-  zing
      ^-  (list (list col-name))
      ::  list of ith successors idents for all i
      ::  e.g. ~[[%idx-n %a-n] ~[%idx-n-n %a-n-n]]
      %+  turn  (gulf 1 num-succ)
      |=  succ-num=@
      :: a list of all ith successor idents e.g. ~[%idx-n %a-n]
      (turn var-names |=(nam=col-name `@tas`(successor-name nam succ-num)))
    ::
    =/  vars-all  (weld var-names successor-names)
    ::  produce the final map of all var-names to mp-graphs
    %-  ~(gas by *(map col-name mp-mega))
    %+  iturn  vars-all
    |=  [i=@ var=col-name]
    [var (mp-var:mp-to-mega i)]
    ::
    ++  successor-name
      |=  [nam=col-name n=@]
      ^-  col-name
      ::  example: when n=2, idx -> xdi -> -n-nxdi -> idx-n-n
      ::  idk why it works but it does lol
      (crip (flop (runt [n '-n'] (flop (trip nam)))))
    --
  ::
  ++  weighted-sum
    ~/  %weighted-sum
    |=  [=row weights=fpoly]
    ^-  felt
    ?>  =(len.row len.weights)
    %+  roll
      ~(to-poly fop (~(zip fop row) weights fmul))
    fadd
  ::
  ++  static-unit-distance
    ~/  %static-unit-distance
    |=  [domain-len=@ height=@]
    ^-  @
    ?:  =(height 0)
      0
    (div domain-len height)
  --  ::tlib
::
::  +jlib: parts of lib/jute that arent related to particular jutes
++  jlib
  |%
  ::  +compute-jute-map
  ::
  ::    map jute name -> index. used to pick which selector goes with which jute.
  ::    we sort the list so the order is deterministic.
  ++  compute-jute-map
    |=  jutes=(list [@tas sam=* prod=*])
    ^-  jute-map
    ?~  jutes  ~
    =/  jute-list=(list term)
      %-  sort
      :_  gth
      %~  tap  in
      %-  ~(gas in *(set @tas))
      (turn jutes |=([name=@tas *] name))
    %-  ~(gas by *(map term @))
    (zip-up jute-list (range (lent jute-list)))
  --
::
++  zkvm-debug
  |%
  +|  %tables
  ::TODO see if this can be made into a door once table structs are more stable
  ++  print-row
    |=  [row=belt-row names=(list term)]
    ^-  @
    ~&  >  "["
    =+
      %+  turn  (zip-up row names)
      |=  [c=belt name=term]
      ~&  >  "{<name>}:{<c>}"
      0
    ~&  >  "]"
    0
  ::
  ++  print-table
    |=  col-names=(list col-name)
    |=  tab=table-mary
    ^-  table-mary
    =-  tab
    %+  turn  (range len.array.p.tab)
    |=  i=@
    =/  row=bpoly  (~(snag-as-bpoly ave p.tab) i)
    ~&  >  "row={<i>}"
    (print-row (bpoly-to-list row) col-names)
  ::
  ++  test
    |=  [table=table-mary [s=* f=*]]
    |=  $:  challenges=(list belt)
            dynamics=bpoly
            funcs=verifier-funcs
        ==
    ::  TODO maybe add a should-fail flag that silences failed constraints or modifies printf
    ::       or change signature to surface error unit?
    ::  augment challenges with derived challenges
    =/  augmented-chals=bpoly
      (augment-challenges:chal challenges s f)
    |^  ^-  ?
    =/  bound-fail  (run-bounds boundary-constraints:funcs dynamics)
    ?.  ?=(~ bound-fail)
      ~&((need bound-fail) %.n)
    =/  term-fail  (run-terms terminal-constraints:funcs dynamics)
    ?.  ?=(~ term-fail)
      ~&((need term-fail) %.n)
    ?:  =(len.array.p.table 0)
      %.n
    ?:  =(len.array.p.table 1)
      %.y  ::  1 row table automatically passes transition constraints
    =/  row-fail  (run-rows row-constraints:funcs dynamics)
    ?.  ?=(~ row-fail)
      ~&((need row-fail) %.n)
    =/  trans-fail  (run-trans transition-constraints:funcs dynamics)
    ?.  ?=(~ trans-fail)
      ~&((need trans-fail) %.n)
    %.y
    ::
    ++  run-bounds
      |=  [boundary-constraints-labeled=(map @tas mp-ultra) dynamics=bpoly]
      %+  mevy  ~(tap by boundary-constraints-labeled)
      |=  [name=@tas constraint=mp-ultra]
      =/  point  (~(snag-as-bpoly ave p.table) 0)
      =/  eval  (mpeval-ultra %base constraint point augmented-chals dynamics)
      ?:  (levy eval |=(b=belt =(b 0)))  ~
      %-  some
      :*  %constraint-failed-bounds
          table=name.table
          name=name
          row=~(to-poly bop point)
          result=eval
      ==
    ::
    ++  run-terms
      |=  [terminal-constraints-labeled=(map @tas mp-ultra) dynamics=bpoly]
      %+  mevy  ~(tap by terminal-constraints-labeled)
      |=  [name=@tas constraint=mp-ultra]
      =/  last  (dec len.array.p.table)
      =/  point  (~(snag-as-bpoly ave p.table) last)
      =/  eval  (mpeval-ultra %base constraint point augmented-chals dynamics)
      ?:  (levy eval |=(b=belt =(b 0)))  ~
      %-  some
      :*  %constraint-failed-terms
          table=name.table
          name=name
          row=~(to-poly bop point)
          result=eval
      ==
    ::
    ++  run-rows
      |=  [row-constraints-labeled=(map @tas mp-ultra) dynamics=bpoly]
      ::  produces ~ if all constraints pass on all points
      ::  and [~ err] on first error
      %+  mevy  ~(tap by row-constraints-labeled)
      ::  following gate produces ~ if given constraint passes on all points
      ::  and [~ err] on first error
      |=  [name=@tas constraint=mp-ultra]
      %+  mevy  (range len.array.p.table)
      |=  i=@
      =/  point       (~(snag-as-bpoly ave p.table) i)
      =/  eval  (mpeval-ultra %base constraint point augmented-chals dynamics)
      ?:  (levy eval |=(b=belt =(b 0)))  ~
      %-  some
      :*  %constraint-failed-row
          table=name.table
          name=name
          row-num=i
          row=~(to-poly bop point)
          result=eval
      ==
    ::
    ++  run-trans
      |=  [transition-constraints-labeled=(map @tas mp-ultra) dynamics=bpoly]
      ::  produces ~ if all constraints pass on all points
      ::  and [~ err] on first error
      %+  mevy  ~(tap by transition-constraints-labeled)
      ::  following gate produces ~ if given constraint passes on all points
      ::  and [~ err] on first error
      |=  [name=@tas constraint=mp-ultra]
      %+  mevy  (range (dec len.array.p.table))
      |=  i=@
      =/  point       (~(snag-as-bpoly ave p.table) i)
      =/  next-point   (~(snag-as-bpoly ave p.table) +(i))
      =/  combo-point  (~(weld bop point) next-point)
      =/  eval  (mpeval-ultra %base constraint combo-point augmented-chals dynamics)
      ?:  (levy eval |=(b=belt =(b 0)))  ~
      %-  some
      :*  %constraint-failed-trans
          table=name.table
          name=name
          row-num=i
          row=~(to-poly bop point)
          next-row=~(to-poly bop next-point)
          result=eval
      ==
    ::
    ++  labeled-constraints
      |=  [constraints=(list mp-ultra) prefix=tape]
      =/  len  (lent constraints)
      %-  ~(gas by *(map @tas mp-ultra))
      (zip (make-labels prefix len) constraints same)
    ::
    ++  make-labels
      |=  [prefix=tape n=@]
      ^-  (list @t)
      ?:  =(n 0)  ~
      %+  turn  (range 1 (add 1 n))
      |=  i=@
      ^-  term
      (crip (welp prefix (scot %ud i)^~))
    --
  --
--
