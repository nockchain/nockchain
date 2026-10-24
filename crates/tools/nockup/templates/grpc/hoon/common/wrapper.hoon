~%  %wrapper  ..ut  ~
|%
+$  goof    [mote=term =tang]
+$  wire    path
+$  ovum    [=wire =input]
+$  crud    [=goof =input]
+$  input   [eny=@ our=@ux now=@da cause=*]
::
++  keep
  |*  inner=mold
  =>
  |%
  +$  inner-state  inner
  +$  outer-state
    $%  [%0 desk-hash=(unit @uvI) internal=inner]
    ==
  +$  outer-fort
    $_  ^|
    |_  outer-state
    ++  load
      |~  arg=outer-state
      **
    ++  peek
      |~  arg=path
      *(unit (unit *))
    ++  poke
      |~  [num=@ ovum=*]
      *[(list *) *]
    ++  wish
      |~  txt=@
      **
    --
  ::
  +$  fort
    $_  ^|
    |_  state=inner-state
    ++  load
      |~  arg=inner-state
      *inner-state
    ++  peek
      |~  arg=path
      *(unit (unit *))
    ++  poke
      |~  arg=ovum
      [*(list *) *inner-state]
    --
  --
  ::
  |=  crash=?
  |=  inner=fort
  |=  hash=@uvI
  =<  .(desk-hash.outer `hash)
  |_  outer=outer-state
  +*  inner-fort  ~(. inner internal.outer)
  ++  load
    |=  old=outer-state
    ~&  build-hash+hash
    ?+    -.old  ~&("wrapper +load: invalid old state" !!)
        %0
      =/  new-internal  (load:inner-fort internal.old)
      ..load(internal.outer new-internal)
    ==
  ::
  ++  peek
    |=  arg=path
    ^-  (unit (unit *))
    =/  pax  ((soft path) arg)
    ?~  pax
      ~>  %slog.[0 leaf+"wrapper +poke: arg is not a path"]
      ~
    (peek:inner-fort u.pax)
  ::
  ++  wish
    |=  txt=@
    ^-  *
    q:(slap !>(~) (ream txt))
  ::
  ++  poke
    |=  [num=@ ovum=*]
    ^-  [(list *) _..poke]
    =/  effects=(list *)  ?:(crash ~[exit/0] ~)
    ?+   ovum  ~&("wrapper +poke invalid arg: {<ovum>}" effects^..poke)
        [[%$ %arvo ~] *]
      =/  g  ((soft crud) +.ovum)
      ?~  g  ~&(%invalid-goof effects^..poke)
      ?:  ?=(%intr mote.goof.u.g)
        [effects ..poke]
      =-  [effects ..poke]
      (slog tang.goof.u.g)
    ::
        [[%poke *] *]
      =/  ovum  ((soft ^ovum) ovum)
      ?~  ovum  ~&("wrapper +poke invalid arg: {<ovum>}" ~^..poke)
      =/  o  ((soft input) input.u.ovum)
      ?~  o
        ~&  "wrapper: could not mold poke type: {<ovum>}"
        ~^..poke
      =^  effects  internal.outer
        (poke:inner-fort u.ovum)
      [effects ..poke(internal.outer internal.outer)]
    ==
  --
--
