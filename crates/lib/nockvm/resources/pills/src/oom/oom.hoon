::  A working pill designed to force bail:meme OOM errors. Requires
::  playpen.hoon, originally used by the toddler pill.
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
  --  =>
  ::
  =|  counter=@u
  |%
  ++  load  !!
  ++  peek  _~
  ++  wish  !!
  ++  poke
    |=  [now=@da ovo=ovum]
    ^-  ^
    ::
    ?.  ?=(?(%crud %wack %wyrd) p.card.ovo)
      ?:  (gte counter 20)
        ~>  %slog.[0 %leaf .=(~ =|(i=@ |-(?:(=(i ^~((bex 32))) ~ [i $(i +(i))])))) 0]
        [~ ..poke]
      ~>  %slog.[0 leaf+(scow %ud counter)]
      =.  counter  +(counter)
      [~ ..poke]
    ::
    =/  buz
      ~>  %mean.'pith: bad wasp'
      ;;(wasp card.ovo)
    ?+  -.buz
      ~>  %slog.[0 leaf+"%wack / %wyrd"]
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
