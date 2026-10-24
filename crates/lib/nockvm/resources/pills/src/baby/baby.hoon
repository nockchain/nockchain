::  A trivial working pill which requires no jets. Requires cradle.hoon
::  from this scaffolding directory.
::
/+  cradle
!.
=/  core
  =>  cradle
  !=
  =>
  |%
  +$  card  (cask)
  ++  cask  |$  [a]  (pair mark a)
  +$  knot  @ta
  ++  list  |$  [item]  $@(~ [i=item t=(list item)])
  +$  mark  @tas
  +$  ovum  [=wire =card]
  ++  pair  |$  [head tail]  [p=head q=tail]
  +$  path  (list knot)
  +$  wire  path
  --  =>
  ::
  |%
  ++  load  !!
  ++  peek  _~
  ++  wish  !!
  ++  poke
    |=  [now=@da ovo=ovum]
    ^-  ^
    :: ~>  %slog.[0 'got']
    :: ~>  %slog.[0 -.card.ovo]
    =/  fec  [//term/1 %blit [%put "effect"] [%nel ~] ~]
    [[fec ~] ..poke]
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
[%pill %baby [aeon .*(cradle core) ~] ~ ~]
