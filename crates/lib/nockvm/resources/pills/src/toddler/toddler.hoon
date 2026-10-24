::  A working pill that grows towards arvo.hoon. Requires playpen.hoon from this
::  scaffolding directory.
::
/+  playpen
!.
=/  core
  =>  playpen
  !=
  =>
  |%
  +$  card  (cask)
  ++  cask  |$  [a]  (pair mark a)
  +$  goof  [mote=term =tang]
  +$  mark  @tas
  +$  ovum  [=wire =card]
  +$  vere  [[non=@ta rev=path] kel=wynn]
  +$  wire  path
  +$  wasp
    ::  %crud: reroute $ovum with $goof
    ::  %wack: iterate entropy
    ::  %wyrd: check/record runtime kelvin stack
    ::
    $%  [%crud =goof =ovum]
        [%wack p=@uvJ]
        [%wyrd p=vere]
    ==
  +$  weft  [lal=@tas num=@ud]
  +$  wynn  (list weft)
  ::  mutually recursive Ackermann functions
  ::  test turning %spot hints on/off
  ++  ack
    ::  re-enable %spot hints
    !:
    |=  [m=@ud n=@ud]
    ::  %memo hint
    ~+
    ?~  m  +(n)
    ?~  n
      (ack (dec m) 1)
    (ack (dec m) $(n (dec n)))
  ++  slow
    |=  x=@ud
    !:
    (slow-help x 0)
  ++  slow-help
    |=  [x=@ud y=@ud]
    !.
    ?:  .=  x  y
      x
    $(y .+(y))
  --  =>
  ::
  |%
  ++  load  !!
  ++  peek  _~
  ++  wish  !!
  ++  poke
    |=  [now=@da ovo=ovum]
    ^-  ^
    ::
    ?.  ?=(?(%crud %wack %wyrd) p.card.ovo)
      ~>  %slog.[0 leaf+(scow %ud (slow (bex 23)))]
      [~ ..poke]
    ::
    =/  buz
      ~>  %mean.'pith: bad wasp'
      ;;(wasp card.ovo)
    ?+  -.buz
      ~>  %slog.[0 leaf+(scow %ud (ack 2 1))]
      [~ ..poke]
    ::
      %crud
        =/  tang  tang.goof.buz
        |-
        ?~  tang
          [~ ..poke]
        ~>  %slog.[0 -.tang]
        $(tang +.tang)
    ==
  --
  ::
  |=  [now=@da ovo=ovum]
  ^-  *
  .(+> +:(poke now ovo))
::
|%
++  aeon
  ^-  *
  =>  *[arvo=* epic=*]
  !=
  |-  ^-  *
  ?@  epic  arvo
  %=  $
    epic  +.epic
    arvo  .*([arvo -.epic] [%9 2 %10 [6 %0 3] %0 2])
  ==
--
[%pill %toddler [aeon .*(playpen core) ~] ~ ~]
