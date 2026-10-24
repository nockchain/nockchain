=<
::  a memoization for formula analysis, updated/inspected at eval
::  boundaries (2/9)
=|  memo=(map * (list [sock (unit sock) (unit @tas)]))
::  A jet dashboard
::
::  The @tas tag is used only for debugging
=|  jute=(map * (list [sock [$-(* *) @tas]]))
|%
::  Partial knowledge of a noun
+$  sock  ^sock
++  sk  .
++  lash
  |=  [[s=sock f=*] j=$-(* *) n=@tas]
  =.  jute
    ?.  (~(has by jute) f)
      (~(put by jute) f (limo ~[[s [j n]]]))
    (~(jab by jute) f |=(js=(list [sock [$-(* *) @tas]]) (weld ~[[s [j n]]] js)))
  sk
++  entr
  |=  [s=sock f=* nam=(unit @tas)]
  ^-  (each _memo [r=(unit sock) n=(unit @tas)])
  =/  entry
    =/  e  (~(get by memo) f)  ?~  e  ~  u.e
  |-
  ^-  (each _memo [(unit sock) (unit @tas)])
  ?~  entry
    :-  %&
    ?:  (~(has by memo) f)
      (~(jab by memo) f |=(e=(list [sock (unit sock) (unit @tas)]) (weld (limo ~[[s ~ nam]]) e)))
    (~(put by memo) f (limo ~[[s ~ nam]]))
  ?:  (mous -<.entry s)
    [%| ->.entry]
  $(entry +.entry)
++  exit
  |=  [s=sock f=* r=sock]
  ^-  _memo
  ~|  "Should not exit where there is no entry"
  %:
    ~(jab by memo)
    f
    |=  e=(list [sock (unit sock) (unit @tas)])
    |-
    ^-  (list [sock (unit sock) (unit @tas)])
    ?<  ?=  %~  e
    ?:  =(-<.e s)
      [[s `r ->+.e] +.e]
    [-.e $(e +.e)]
  ==
++  bord  *(map * (list [sock [$-(* *) @tas]]))
::  test whether a sock nests in another sock
::  a=sock nests in b=sock if b has the same information as a, or
::  strictly more information
::
++  mous
  |=  [a=sock b=sock]
  ^-  ?
  ?:  ?=  [%gues ~]  a
    %.y
  ?-  a
      ::
      [%hint *]
    ?&
      ?=  [%hint *]  b
      .=  s.a  s.b
      ?~  d.a
        ?~  d.b
          %.y
        %.n
      ?~  d.b
        %.n
      $(a u.d.a, b u.d.b)
      $(a r.a, b r.b)
    ==
      ::
      [%know *]
    ?|
      ?&
        ?=  [%know *]  b
        =(k.a k.b)
      ==
      ?&
        ?=  [%bets * *]  b
        ?=  [* *]  k.a
        $(a [%know -.k.a], b p.b)
        $(a [%know +.k.a], b q.b)
      ==
    ==
      ::
      [%bets * *]
    ?|
      ?&
        ?=  [%know * *]  b
        $(a p.a, b [%know -.k.b])
        $(a q.a, b [%know +.k.b])
      ==
      ?&
        ?=  [%bets * *]  b
        $(a p.a, b p.b)
        $(a q.a, b q.b)
      ==
    ==
  ==
::  compute what we know of the result of a formula
::
::  We may reimplement this more efficiently later, but this way we
::  don't have to keep updating our experiments in 2 implementations
++  wash
  |=  [s=sock f=*]
  ^-  [sock _memo]
  =+  (pull s f)
  [(cort -<) ->]
::  Check for a jet
++  juke
  |=  [s=sock f=*]
  ^-  (unit [$-(* *) @tas])
  =/  jets
    =/  j  (~(get by jute) f)
    ^-  (list [sock [$-(* *) @tas]])
    ?~  j  ~  u.j
  |-
  ^-  (unit [$-(* *) @tas])
  ?~  jets
    ~
  ?:  (mous -<.jets s)
    `->.jets
  $(jets +.jets)
::  learn a noun at an address
::
::  darn discards hints if it must descend through them
::
:: TODO: we use +knit everywhere, this is just +knit with [%know *], can
:: we get rid of it?
++  darn
  |=  [s=sock b=@ n=*]
  ^-  sock
  ?:  ?=  [%hint *]  s
    $(s r.s)
  ?:  =(b 1)
    [%know n]
  ?:  ?=  [%know *]  s
    :-  %know
    .*  [k.s n]  [%10 [b [0 3]] [0 2]] 
  ?-  (cap b)
      ::
      %2
    ?-  s
        ::
        [%gues ~]
      [%bets p=$(b (mas b), s [%gues ~]) q=[%gues ~]]
        ::
        [%bets p=* q=*]
      [%bets p=$(b (mas b), s p.s) q=q.s]
    ==
      ::
      %3
    ?-  s
        ::
        [%gues ~]
      [%bets p=[%gues ~] q=$(b (mas b), s [%gues ~])]
        ::
        [%bets p=* q=*]
      [%bets p=p.s q=$(b (mas b), s q.s)]
    ==
  ==
::  axis into a sock
::
::  yarn will return a hint record if it is at an axis
::  but will discard them as it descends through them
++  yarn
  |=  [s=sock b=@]
  ^-  sock
  ?<  =(b 0)
  |-
  ?:  =(b 1)
    s
  ?:  ?=  [%hint *]  s
    $(s r.s)  ::  descend through hint records
  ?-  s
      ::
      [%know *]
    :-  %know
    .*  k.s  [0 b]
      ::
      [%gues ~]
    [%gues ~]
      ::
      [%bets p=* q=*]
     ?-  (cap b)
         ::
         %2
       $(b (mas b), s p.s)
         ::
         %3
       $(b (mas b), s q.s)
       ::
     ==
  ==
::  axis into a sock, retrieving a name from a fast hint
++  narn
  |=  [s=sock b=@]
  ^-  [sock (unit @tas)]
  ?<  =(b 0)
  =|  nam=(unit @tas)
  |-
  ^-  [sock (unit @tas)]
  ?:  =(b 1)
    ?.  ?=  [%hint *]  s  [s nam]
    ?.  =(s.s %fast)  [s nam]
    ?~  d.s  [s nam]
    =/  namsock  (yarn u.d.s 2)
    ?.  ?=  [%know @]  namsock  [s nam]
    [s `k.namsock]
  ?:  ?=  [%hint *]  s
     ?.  =(s.s %fast)  $(s r.s)
     ?~  d.s  $(s r.s)
     =/  namsock  (yarn u.d.s 2)
     ?.  ?=  [%know @]  namsock  $(s r.s)
     =.  nam  `k.namsock
     $(s r.s)
  ?-  s
      ::
      [%know *]
    :-
      :-  %know
      .*  k.s  [%0 b]
    nam
      ::
      [%gues ~]
    [[%gues ~] nam]
      ::
      [%bets * *]
    ?-  (cap b)
        ::
        %2
      $(b (mas b), s p.s)
        ::
        %3
      $(b (mas b), s q.s)
    ==
  ==
::  make a new sock from 2 socks (and an axis)
++  knit
  |=  [s=sock b=@ t=sock]
  ^-  sock
  ?:  =(b 1)
    t
  ?:  ?=  [%hint *]  s
    [%hint s.s d.s $(s r.s)]
  ?-  s
      ::
      [%know @]
    ~|  %know-atom  !!
      ::
      [%know * *]
    ?-  (cap b)
        ::
        %2
      =/  r  $(s [%know +<.s], b (mas b))
      ?:  ?=  [%know k=*]  r
        [%know +.r +>.s]
      [%bets r [%know +.k.s]]
        ::
        %3
      =/  r  $(s [%know +>.s], b (mas b))
      ?:  ?=  [%know k=*]  r
        [%know +<.s +.r]
      [%bets [%know +<.s] r]
    ==
      ::
      [%bets p=* q=*]
    ?-  (cap b)
        ::
        %2
      =/  r  $(s p.s, b (mas b))
      ?:  ?=  [%know k=*]  r
        ?:  ?=  [%know k=*]  q.s
          [%know +.r +.q.s]
        [%bets r q.s]
      [%bets r q.s]
        ::
        %3
      =/  r  $(s q.s, b (mas b))
      ?:  ?=  [%know k=*]  r
        ?:  ?=  [%know k=*]  p.s
          [%know +.p.s +.r]
        [%bets p.s r]
      [%bets p.s r]
    ==
      ::
      [%gues ~]
    =/  r  $(b (mas b))
    ?:  ?=  [%gues ~]  r
      [%gues ~]
    ?-  (cap b)
        ::
        %2
      [%bets r [%gues ~]]
        ::
        %3
      [%bets [%gues ~] r]
    ==
  ==
++  pear
  |=  [a=sock b=sock]
  ^-  sock
  ?:  ?=  [%know *]  a
    ?:  ?=  [%know *]  b
      [%know =(a b)]
    [%gues ~]
  [%gues ~]
::  Description of knowledge at a call site
::
::  Codegen behavior
::  %dyn means we will generate a full eval: check cache or codegen,
::  guard on stored sock
::
::  %mis means we will do fresh inline codegen of a formula with a new
::  label
::
::  %rec and %hit both mean we will generate to jumps to labels
::  They differ because for %rec the analysis must treat the call as
::  unknowable, while for %hit we do not re-analyze the call but return
::  the memoized sock
::
::  %jet corresponds to a jump to a jet
+$  coot
  $%
    [%dyn =sock]     :: we don't know the formula
    [%mis =foot]     :: we know the formula, it's not memoized
    [%rec =sock f=*] :: a recursive call, the memo table has a blackhole for this sock/formula pair
    [%hit res=sock]  :: a memoized call, the memo table has an entry for this sock/formula pair
    [%jet jet=@tas]  :: call would be jetted 
  ==
::  Annotated Nock tree with subject knowledge
+$  foot
  $:
    $%
      [%1 b=*]
      [%2 b=foot c=foot =coot]
      [%3 b=foot]
      [%4 b=foot]
      [%5 b=foot c=foot]
      [%6 b=foot c=(unit foot) d=(unit foot)]
      [%7 b=foot c=foot]
      [%8 b=foot c=foot]
      [%9 b=@ c=foot =coot]
      [%10 [b=@ c=foot] d=foot]
      [%11 b=@ c=foot]
      [%11 [b=@ c=foot] d=foot]
      [%12 ref=foot path=foot]
      [%cell p=foot q=foot]
      [%0 b=@]
    ==
    $=  s  sock
    $=  r  sock
  ==
++  cort
  |=  =coot
  ^-  sock
  ?-  coot
      ::
      [%dyn *]
    [%gues ~]
      ::
      [%mis *]
    r.foot.coot
      ::
      [%rec *]
    [%gues ~]
      ::
      [%hit *]
    res.coot
      ::
      [%jet *]
    [%gues ~]
  ==
++  pull
  |=  [s=sock f=*]
  (pull-eval s [%know f] ~)
++  pull-eval
  |=  [s=sock f=sock nam=(unit @tas)]
  ^-  [coot _memo]
  ?:  ?=  [%hint *]  f  $(f r.f)
  ?.  ?=  [%know *]  f
    [[%dyn s] memo]
  =/  jet  (juke s k.f)
  ?.  ?=  ~  jet
    ::  found a jet
    ~&  "Jet: {<+.u.jet>}"
    [[%jet +.u.jet] memo]
  =/  mem=(each _memo [r=(unit sock) n=(unit @tas)])  (entr s k.f nam)
  ?-  mem
      ::
      [%& *]
    =.  memo  +.mem
    =^  res  memo  (pull-inner s k.f)
    =.  memo  (exit s k.f r.res)
    [[%mis res] memo]
      ::
      [%| *]
    ?~  r.p.mem
      :: memo blackhole
      ~&  "Recur: sock {<[s]>} formula {<k.f>}"
      [[%rec s k.f] memo]
    :: memo hit
    ~&  "Hit: sock {<[s]>} formula {<k.f>} result {<u.r.p.mem>} named {<n.p.mem>}"
    [[%hit u.r.p.mem] memo] 
  ==
++  pull-inner
  |=  [s=sock f=*]
  ^-  [foot _memo]
  ?+  f  ~|  "Unrecognized nock {<f>}"  ~|  %pull-bonk  !!
      ::
      [[* *] *]
    =^  pfoot  memo  $(f -.f)
    =^  qfoot  memo  $(f +.f)
    [[[%cell pfoot qfoot] s=s r=(knit [%bets r.pfoot [%gues ~]] 3 r.qfoot)] memo]
      ::
      [%0 b=@]
    [[[%0 b.f] s=s r=(yarn s b.f)] memo]
      ::
      [%1 b=*]
    [[[%1 b.f] s=s r=[%know b.f]] memo]
      ::
      [%2 b=* c=*]
    =^  bfoot  memo  $(f b.f)
    =^  cfoot  memo  $(f c.f)
    =/  [r=sock nam=(unit @tas)]  (narn r.cfoot 1)
    =^  coot  memo  (pull-eval r.bfoot r nam)
    [[[%2 bfoot cfoot coot] s (cort coot)] memo]
      ::
      [%3 b=*]
    =^  bfoot  memo  $(f b.f)
    =/  br  r.bfoot
    =/  r
      |-
      ?-  br
          ::
          [%hint *]
        $(br r.br)
          ::
          [%know @]
        [%know 1]
          ::
          [%know * *]
        [%know 0]
          ::
          [%bets * *]
        [%know 0]
          ::
          [%gues ~]
        [%gues ~]
      ==
    [[[%3 bfoot] s r] memo]
      ::
      [%4 b=*]
    =^  bfoot  memo  $(f b.f)
    [[[%4 bfoot] s [%gues ~]] memo]
      ::
      [%5 b=* c=*]
    =^  bfoot  memo  $(f b.f)
    =^  cfoot  memo  $(f c.f)
    =/  br  r.bfoot
    =/  cr  r.cfoot
    =/  r
      |-
      ?:  ?=  [%hint *]  br
        $(br r.br)
      ?:  ?=  [%hint *]  cr
        $(cr r.cr)
      ?:  ?=  [%know *]  r.bfoot
        ?:  ?=  [%know *]  r.cfoot
          [%know =(k.r.bfoot k.r.cfoot)]
        [%gues ~]
      [%gues ~]
    [[[%5 bfoot cfoot] s r] memo]
      ::
      [%6 b=* c=* d=*]
    =^  bfoot  memo  $(f b.f)
    =/  br  r.bfoot
    |-
    ?+  br  ~|  %pull-nest  !!
        ::
        [%hint *]
      $(br r.br)
        ::
        [%know %0]
      =^  cfoot  memo  ^$(f c.f)
      [[[%6 bfoot `cfoot ~] s r.cfoot] memo]
        ::
        [%know %1]
      =^  dfoot  memo  ^$(f d.f)
      [[[%6 bfoot ~ `dfoot] s r.dfoot] memo]
        ::
        [%gues ~]
      =^  cfoot  memo  ^$(f c.f)
      =^  dfoot  memo  ^$(f d.f)
      [[[%6 bfoot `cfoot `dfoot] s [%gues ~]] memo]
    ==
      ::
      [%7 b=* c=*]
    =^  bfoot  memo  $(f b.f)
    =^  cfoot  memo  $(s r.bfoot, f c.f)
    [[[%7 bfoot cfoot] s r.cfoot] memo]
      ::
      [%8 b=* c=*]
    =^  bfoot  memo  $(f b.f)
    =^  cfoot  memo  $(s (knit [%bets [%gues ~] s] 2 r.bfoot), f c.f)
    [[[%8 bfoot cfoot] s=s r=r.cfoot] memo]
      ::
      [%9 b=@ c=*]
    =^  cfoot  memo  $(f c.f)
    =/  [r=sock nam=(unit @tas)]  (narn r.cfoot b.f)
    =^  coot  memo  (pull-eval r.cfoot r nam)
    [[[%9 b.f cfoot coot] s (cort coot)] memo]
      ::
      [%10 [b=@ c=*] d=*]
    =^  cfoot  memo  $(f c.f)
    =^  dfoot  memo  $(f d.f)
    [[[%10 [b.f cfoot] dfoot] s (knit r.dfoot b.f [%gues ~])] memo]
      ::
      [%11 b=@ c=*]
    =^  cfoot  memo  $(f c.f)
    [[[%11 b.f cfoot] s r=[%hint b.f ~ r.cfoot]] memo]
      ::
      [%11 [b=@ c=*] d=*]
    =^  cfoot  memo  $(f c.f)
    ::  implement the %data hint
    ::  delete the axes which are looked up in the hint from the subject
    ::  knowledge for the hinted computation
    ?:  =(b.f %data)
      =/  c  c.f
      |-
      ?:  ?=  [[%0 @] *]  c
        =.  s  (knit s ->.c [%gues ~])
        $(c +.c)
      =^  dfoot  memo  ^$(f d.f)
      [[[%11 [b.f cfoot] dfoot] s [%hint b.f `r.cfoot r.dfoot]] memo]
    =^  dfoot  memo  $(f d.f)
    [[[%11 [b.f cfoot] dfoot] s [%hint b.f `r.cfoot r.dfoot]] memo]
      ::
      [%12 ref=* path=*]
    =^  reffoot  memo  $(f ref.f)
    =^  pathfoot  memo  $(f path.f)
    [[[%12 reffoot pathfoot] s [%gues ~]] memo]
  ==
++  hord
  |=  [nam=@tas]
  ^-  (list [s=sock f=*])
  =|  r=(list [s=sock f=*])
  =/  entries  ~(tap by memo)
  |-
  ^-  (list [s=sock f=*])
  ?~  entries
    r
  =/  f  p.i.entries
  =/  entry  q.i.entries
  |-
  ^-  (list [s=sock f=*])
  ?~  entry
    ^$(entries t.entries)
  =?  r  =(+>.i.entry `nam)  (weld (limo ~[[-.i.entry f]]) r)
  $(entry t.entry)
::  example nocks for testing
++  nocs
  |%
  ++  dec
    !=
    !.
    =>
    =>  %ed
    |%
    ++  dec
      ~/  %dec
      |=  x=@
      ~>  %data.[x ~]
      ^-  @
      =|  d=@
      |-
      ^-  d=@
      ?:  =(.+(d) x)
        d
      $(d .+(d))
    --
    (dec 8)
  ++  ad
    !=
    !.
    =>
    =>  %ed
    |%
    ++  dec
      ~/  %dec
      |=  x=@
      ~>  %data.[x ~]
      ^-  @
      =|  d=@
      |-
      ^-  d=@
      ?:  =(.+(d) x)
        d
      $(d .+(d))
    ++  add
      ~/  %add
      |=  [x=@ y=@]
      ~>  %data.[x y ~]
      ^-  @
      |-
      ^-  @
      ?:  =(x 0)
        y
      $(x (dec x), y .+(y))
    --
    (add 5 8)
  --
--
|%
+$  sock
  $%
    [%know k=*]
    [%hint s=@ d=(unit sock) r=sock]
    [%bets p=sock q=sock]
    [%gues ~]
  ==
--
