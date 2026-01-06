/=  z  /apps/jojo/jojo-imports
/=  *  /common/wrapper
=<  ((moat |) inner)  :: wrapped kernel
=>
  |%
  +$  effect  [%jojo res=vase pret=(unit @t)]
  +$  kernel-state  [%state version=%1]
  +$  cause
    :: $+  cause
    $%  [%raw pret=bean hoon=@t subject=(unit *)]
        [%sam pret=bean function-name=@t subject=(unit *) sample=*]
        [%prt pret=bean val=vase]
    ==
  --
|%
++  moat  (keep kernel-state) :: no state
++  inner
  |_  k=kernel-state
  ::  do-nothing load
  ++  load
    |=  =kernel-state  kernel-state
  ::  crash-only peek
  ++  peek
    |=  arg=*
    =/  pax  ((soft path) arg)
    ?~  pax  ~|(not-a-path+arg !!)
    ~|(invalid-peek+pax !!)
  ::  poke: try to prove a block
  ++  poke
    |=  [wir=wire eny=@ our=@ux now=@da dat=*]
    ^-  [(list effect) k=kernel-state]
    |^
    =/  cause  ((soft cause) dat)
    ?~  cause
      ~&  dat
      ~>  %slog.[0 [%leaf "error: bad cause"]]
      `k
    =/  cause  u.cause
    =/  res
      ?-  -.cause
        %sam  (do-sam function-name.cause sample.cause subject.cause)
        %raw  (do-raw hoon.cause subject.cause)
        %prt  `vase`[%noun +:val.cause] :: pretty printing with external vases may be long...
      ==
    =/  print
      ?.  pret.cause  ~
      [~ (crip (noah res))]
    :_  k
      [%jojo res print]~
    ++  do-sam
        |=  [function-name=@t sample=* subject=(unit *)]
        =/  vas  ?~  subject
          (slap !>(z) (ream function-name))
          (slap !>(u.subject) (ream function-name))
        =/  res  (slym vas sample)
        res
    ++  do-raw
        |=  [hoon=@t subject=(unit *)]
        =/  res  ?~  subject
          (slap !>(z) (ream hoon))
          (slap !>(u.subject) (ream hoon))
        res
    --
  --
--
