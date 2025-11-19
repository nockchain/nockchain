::  testing utilities meant to be directly used from files in %/tests
::
|%
::  +expect-eq: compares :expected and :actual and pretty-prints the result
::
++  expect-eq
  |=  [expected=vase actual=vase]
  ^-  tang
  ::
  =|  result=tang
  ::
  =?  result  !=(q.expected q.actual)
    %+  weld  result
    ^-  tang
    :~  [%palm [": " ~ ~ ~] [leaf+"expected" (sell expected) ~]]
        [%palm [": " ~ ~ ~] [leaf+"actual  " (sell actual) ~]]
    ==
  ::
  =?  result  !(~(nest ut p.actual) | p.expected)
    %+  weld  result
    ^-  tang
    :~  :+  %palm  [": " ~ ~ ~]
        :~  [%leaf "failed to nest"]
            (~(dunk ut p.actual) %actual)
            (~(dunk ut p.expected) %expected)
    ==  ==
  result
::
::
++  expect
  |=  actual=vase
  (expect-eq !>(%.y) actual)
::
::  +expect-null: checks if actual is null
++  expect-null
  |=  actual=vase
  (expect-eq !>(%.y) !>(?=(~ q.actual)))
::
::  +expect-some: checks if actual is not null, used to check units
++  expect-some
  |=  actual=vase
  (expect-eq !>(%.n) !>(?=(~ q.actual)))
::
::  +expect-all: checks that all vases in list are %.y
++  expect-all
  |=  vs=(list vase)
  ^-  tang
  ?~  vs  ~
  ?~  t.vs  ~
  =/  h=vase  i.vs
  =/  tl=(list vase)  t.vs
  |-  ^-  tang
  ?~  tl  ~
  (weld (expect-eq !>(%.y) i.tl) $(tl t.tl))
::
::  +expect-fail: kicks a trap, expecting crash. pretty-prints if succeeds
++  expect-fail
  |=  [a=(trap) err=(unit tape)]
  ^-  tang
  =/  b  (mule a)
  ?:  ?=(%& -.b)
    =-  (welp - ~[(sell !>(p.b))])
    ~['expected crash, got: ']
  ?~  err
    %.  ~
    (%*(. slog pri 1) ['caught expected failure: ' p.b])
  =/  found=(unit tank)
    (find-tank p.b u.err)
  ?:  ?=(^ found)
    %.  ~
    (%*(. slog pri 1) ['caught expected failure: ' p.b])
  %+  weld
    ^-  tang
    :~  [%palm [": " ~ ~ ~] [leaf+"expected" leaf+u.err ~]]
        [%palm [": " ~ ~ ~] [leaf+"actual  " ~]]
    ==
  p.b
    ::
::  +expect-runs: kicks a trap, expecting success; returns trace on failure
::
++  expect-success
  |=  a=(trap)
  ^-  tang
  =/  b  (mule a)
  ?-  -.b
    %&  ~
    %|  ['expected success - failed' ((slog p.b) p.b)]
  ==
::  $a-test-chain: a sequence of tests to be run
::
::  NB: arms shouldn't start with `test-` so that `-test % ~` runs
::
+$  a-test-chain
  $_
  |?
  ?:  =(0 0)
    [%& p=*tang]
  [%| p=[tang=*tang next=^?(..$)]]
::  +run-chain: run a sequence of tests, stopping at first failure
::
++  run-chain
  |=  seq=a-test-chain
  ^-  tang
  =/  res  $:seq
  ?-  -.res
    %&  p.res
    %|  ?.  =(~ tang.p.res)
          tang.p.res
        $(seq next.p.res)
  ==
::  +category: prepends a name to an error result; passes successes unchanged
::
++  category
  |=  [a=tape b=tang]  ^-  tang
  ~&  >  "category: {a}"
  ?:  =(~ b)  ~  :: test OK
  :-  leaf+"in: '{a}'"
  (turn b |=(c=tank rose+[~ "  " ~]^~[c]))
::  +give-result: runs a test, pretty-prints the result
::
++  give-result
  |=  [name=tape test=(trap tang)]
  ^-  [ok=? =tang]
  =+  run=(mule test)
  ?-  -.run
    %|  |+(welp p.run leaf+"CRASHED {name}" ~)
    %&  ?:  =(~ p.run)
          &+[leaf+"OK      {name}"]~
        |+(flop `tang`[leaf+"FAILED  {name}" p.run])
  ==
++  find-tank
  |=  [=tang =tape]
  ^-  (unit tank)
  ?~  tang  ~
  ::  %-  (slog i.tang ~)
  ?.  ?=(%leaf -.i.tang)  $(tang t.tang)
  ?:  ?=(^ (find tape i.tang))
    `i.tang
  $(tang t.tang)
::
::
::  Convenience functions for roswell testing modules
::
+$  test-arm  [name=term func=test-func]
+$  test-func  (trap tang)
++  succeed
  |=  res=(list [ok=? =tang])
  ^-  ?
  %+  roll  res
  |=  [[ok=? =tang] pass=?]
  %-  (slog (flop tang))
  &(pass ok)
::
++  run-tests
  |=  test-arms=(list test-arm)
  ^-  (list [ok=? =tang])
  %+  turn  test-arms
  |=  =test-arm
  (run-test test-arm)
::
++  run-test
  |=  =test-arm
  ^-  [ok=? =tang]
  =+  name=(trip name.test-arm)
  ~&  >>  "-------------- RUNNING TEST --------------  ".
          "{name}   ".
          "-----------------------------------------"
  =+  run=(mule func.test-arm)
  ?-  -.run
    %|  [| `tang`(welp p.run leaf+"CRASHED {name}" ~)]
    %&  ?:  =(~ p.run)
          [& `tang`[leaf+"OK      {name}"]~]
        [| (flop `tang`[leaf+"FAILED  {name}" p.run])]
  ==
::
++  get-test-arms
  |=  tests-core=vase
  ^-  (list test-arm)
  (get-prefix-arms 'test-' tests-core)
::
::  +get-prefix-arms: produce arms that begin with .prefix
++  get-prefix-arms
  |=  [prefix=term tests-core=vase]
  ^-  (list test-arm)
  |^
  =/  arms=(list @tas)  (sloe p:tests-core)
  %+  turn  (skim arms has-prefix)
  |=  name=term
  ^-  test-arm
  =/  fire-arm=nock
    ~|  [%failed-to-compile-test-arm name]
    q:(~(mint ut p:tests-core) p:!>(*tang) [%limb name])
  :-  name
  |.(;;(tang ~>(%bout.[1 name] .*(q:tests-core fire-arm))))
::
  ++  has-prefix
    |=  a=term  ^-  ?
    =((end [3 (met 3 prefix)] a) prefix)
  --
--
