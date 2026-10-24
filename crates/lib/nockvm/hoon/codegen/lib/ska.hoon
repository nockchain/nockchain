/-  *sock
!:
|%
++  trip
  |=  toob=$<(%boom boot)
  ^-  (unit *)
  ?-  -.toob
    %safe  (stub sure.toob)
    %risk  (stub hope.toob)
  ==
++  stub
  |=  =sock
  ^-  (unit *)
  ?:  ?=(%know -.sock)
    `know.sock
  ~
:: Split an axis into a sock into safe and unsafe components
++  punt
  |=  [axe=@ =sock]
  ^-  [@ @ ^sock]
  ?:  =(0 axe)
    [0 0 %toss ~]
  =/  saf  1
  |-
    ?:  =(axe 1)
      [saf 1 sock]
    ?+  sock  [0 0 %toss ~]
      [%know * *]
    ?-  (cap axe)
      %2  $(axe (mas axe), sock [%know -.know.sock], saf (peg saf 2))
      %3  $(axe (mas axe), sock [%know +.know.sock], saf (peg saf 3))
    ==
      ::
      [%bets *]
    ?-  (cap axe)
      %2  $(axe (mas axe), sock hed.sock, saf (peg saf 2))
      %3  $(axe (mas axe), sock tal.sock, saf (peg saf 3))
    ==
      ::
      [%toss ~]
    [saf axe %toss ~]
  ==
:: Get an axis from a sock
++  pull
  |=  arg=[@ sock]
  ^-  boot
  =+  [saf rik ken]=(punt arg)
  ?:  =(0 saf)  [%boom ~]
  ?:  =(1 rik)  [%safe ken]
  [%risk ken]
++  yank
  |=  [axe=@ =boot]
  ?-  boot
    [%safe *]  (pull axe sure.boot)
    [%risk *]  (dare (pull axe hope.boot))
    [%boom ~]  [%boom ~]
  ==
:: Test if sock is atom or cell, or unknown
++  fits
  |=  =sock
  ^-  ^sock
  ?-  sock
      ::
      [%know @]
    [%know 1]
      ::
      [%know * *]
    [%know 0]
      ::
      [%bets *]
    [%know 0]
      ::
      [%dice ~]
    [%know 1]
      ::
      [%flip ~]
    [%know 1]
      ::
      [%toss ~]
    [%flip ~]
  ==
:: Test if we can know two socks are equal
++  pear
  |=  [a=sock b=sock]
  ^-  sock
  ?:  ?&(?=([%know *] a) ?=([%know *] b))
    ?:  =(know.a know.b)
      [%know 0]
    [%know 1]
  [%flip ~]
:: Test if we can know two boots are equal
++  bopp
  |=  [a=boot b=boot]
  ^-  boot
  ?:  ?=  [%boom ~]  a
    [%boom ~]
  ?:  ?=  [%boom ~]  b
    [%boom ~]
  ?-  a
      ::
      [%safe *]
    ?-  b
        ::
        [%safe *]
      [%safe (pear sure.a sure.b)]
        ::
        [%risk *]
      [%risk (pear sure.a hope.b)]
    ==
      ::
      [%risk *]
    ?-  b
        ::
        [%safe *]
      [%risk (pear hope.a sure.b)]
        ::
        [%risk *]
      [%risk (pear hope.a hope.b)]
    ==
  ==
:: combine two socks into a sock of a cell
++  knit
  |=  [a=sock b=sock]
  ^-  sock
  ?.  ?=  [%know *]  a
    [%bets a b]
  ?.  ?=  [%know *]  b
    [%bets a b]
  [%know [know.a know.b]]
:: combine two boots into a boot of a cell
++  cobb
  |=  [hed=boot tal=boot]
  ^-  boot
  ?:  ?=  [%boom ~]  hed
    [%boom ~]
  ?:  ?=  [%boom ~]  tal
    [%boom ~]
  ?-  hed
      ::
      [%safe *]
    ?-  tal
        ::
        [%safe *]
      [%safe (knit sure.hed sure.tal)]
        ::
        [%risk *]
      [%risk (knit sure.hed hope.tal)]
    ==
      ::
      [%risk *]
    ?-  tal
        ::
        [%safe *]
      [%risk (knit hope.hed sure.tal)]
        ::
        [%risk *]
      [%risk (knit hope.hed hope.tal)]
    ==
  ==
:: patch a sock
++  darn
  |=  [axe=@ pat=sock =sock]
  ^-  boot
  ?:  .=  0  axe
    [%boom ~]
  |-
  ^-  boot
  ?:  =(axe 1)
    [%safe pat]
  ?:  ?=  [%dice ~]  sock
    [%boom ~]
  ?:  ?=  [%flip ~]  sock
    [%boom ~]
  ?:  ?=  [%know @]  sock
    [%boom ~]
  ?-  (cap axe)
      ::
      %2
    ?-  sock
        ::
        [%know * *]
      (cobb $(axe (mas axe), sock [%know -.know.sock]) [%safe %know +.know.sock])
        ::
        [%bets * *]
      (cobb $(axe (mas axe), sock hed.sock) [%safe tal.sock])
        ::
        [%toss ~]
      (cobb $(axe (mas axe)) [%risk %toss ~])
    ==
      ::
      %3
    ?-  sock
        ::
        [%know * *]
      (cobb [%safe %know -.know.sock] $(axe (mas axe), sock [%know +.know.sock]))
        ::
        [%bets * *]
      (cobb [%safe hed.sock] $(axe (mas axe), sock tal.sock))
        ::
        [%toss ~]
      (cobb [%risk %toss ~] $(axe (mas axe)))
    ==
  ==
:: Stitch a boot into another boot
++  welt
  |=  [axe=@ pach=boot wole=boot]
  ^-  boot
  ?:  ?=  [%boom ~]  pach
    [%boom ~]
  ?:  ?=  [%boom ~]  wole
    [%boom ~]
  =/  poch
    ?-  pach
        ::
        [%safe *]
      sure.pach
        ::
        [%risk *]
      hope.pach
    ==
  =/  wool
    ?-  wole
        ::
        [%safe *]
      sure.wole
        ::
        [%risk *]
      hope.wole
    ==
  ?:  ?&  ?=  [%safe *]  wole  ?=  [%safe *]  pach  ==
    (darn axe poch wool)
  (dare (darn axe poch wool))

:: Pessimize a boot by making it %risk even if it's %safe
++  dare
  |=  =boot
  ?-  boot
      ::
      [%boom ~]
    [%boom ~]
      ::
      [%risk *]
    [%risk hope.boot]
      ::
      [%safe *]
    [%risk sure.boot]
  ==
:: Weaken a %know
++  fray
  |=  a=*
  ^-  sock
  ?:  ?=  @  a
    [%dice ~]
  [%bets [%know -.a] [%know +.a]]
:: Produce the intersection of two socks
++  mous
  |=  [a=sock b=sock]
  ?:  ?&(?=([%know *] a) ?=([%know *] b))
    ?:  =(know.a know.b)
      a
    $(a (fray know.a), b (fray know.b))
  ?:  ?=([%know *] a)
    $(a (fray know.a))
  ?:  ?=([%know *] b)
    $(b (fray know.b))
  ?:  ?&(?=([%bets *] a) ?=([%bets *] b))
    [%bets $(a hed.a, b hed.b) $(a tal.a, b tal.b)]
  ?:  ?&(?=([%dice ~] a) ?|(?=([%dice ~] b) ?=([%flip ~] b)))
    [%dice ~]
  ?:  ?&(?=([%dice ~] b) ?=([%flip ~] a))
    [%dice ~]
  ?:  ?&(?=([%flip ~] a) ?=([%flip ~] b))
    [%flip ~]
  [%toss ~]
::  Produce the intersection of two boots
::
::  Note that the intersection of a safe or risk
::  boot and a boom boot is a risk boot, since
::  in a branch between a possibly non-crashing computation
::  and a crashing computation, we might crash and we might not.
::
::  In particular, we have to handle assertions and
::  error cases where it is intended that one branch of a conditional
::  will crash
++  gnaw
  |=  [a=boot b=boot]
  ?:  ?=  [%safe *]  a
    ?:  ?=  [%safe *]  b
      [%safe (mous sure.a sure.b)]
    ?:  ?=  [%risk *]  b
      [%risk (mous sure.a hope.b)]
    [%risk sure.a]
  ?:  ?=  [%risk *]  a
    ?:  ?=  [%safe *]  b
      [%risk (mous hope.a sure.b)]
    ?:  ?=  [%risk *]  b
      [%risk (mous hope.a hope.b)]
    [%risk hope.a]
  ?:  ?=  [%safe *]  b
    [%risk sure.b]
  ?:  ?=  [%risk *]  b
    [%risk hope.b]
  [%boom ~]
::  Produce a boot of whether a given boot is a cell or atom
++  ques
  |=  non=boot
  ^-  boot
  ?:  ?=([%boom ~] non)
    [%boom ~]
  ?-  non
      ::
      [%safe %know @]
    [%safe %know 1]
      ::
      [%safe %know * *]
    [%safe %know 0]
      ::
      [%safe %bets *]
    [%safe %know 0]
      ::
      [%safe %dice ~]
    [%safe %know 1]
      ::
      [%safe %flip ~]
    [%safe %know 1]
      ::
      [%safe %toss ~]
    [%safe %flip ~]
      ::
      [%risk %know @]
    [%risk %know 1]
      ::
      [%risk %know * *]
    [%risk %know 0]
      ::
      [%risk %bets *]
    [%risk %know 0]
      ::
      [%risk %dice ~]
    [%risk %know 1]
      ::
      [%risk %flip ~]
    [%risk %know 1]
      ::
      [%risk %toss ~]
    [%risk %flip ~]
  ==
++  pile
  |=  tom=boot
  ^-  boot
  ?+  tom  [%boom ~]
      ::
      [%safe %know @]
    [%safe %dice ~]
      ::
      [%safe %dice ~]
    [%safe %dice ~]
      ::
      [%safe %flip ~]
    [%safe %dice ~]
      ::
      [%safe %toss ~]
    [%risk %dice ~]
      ::
      [%risk %know @]
    [%risk %dice ~]
      ::
      [%risk %dice ~]
    [%risk %dice ~]
      ::
      [%risk %flip ~]
    [%risk %dice ~]
      ::
      [%risk %toss ~]
    [%risk %dice ~]
  ==
::  Produce knowledge of the result given knowledge of the subject
++  wash
  |=  [subj=sock form=*]
  ^-  boot
  =|  bare=[ward=(map [sock *] boot) dir=@ ind=@]
  =.  ward.bare  (~(put by ward.bare) [subj form] [%risk %toss ~])
  |^
    =+  swab
    ~&  "direct calls: {<dir>}"
    ~&  "indirect calls: {<ind>}"
    -<
  ++  swab
    |-
    ^-  [boot _bare]
    ?>  ?=(^ form)
    ?+  form  [[%boom ~] bare]
        ::
        [[* *] *]
      =^  l  bare  $(form -.form)
      =^  r  bare  $(form +.form)
      :_  bare
      (cobb l r)
        ::
        [%0 @]
      :_  bare
      (pull +.form subj)
        ::
        [%1 *]
      :_  bare
      [%safe %know +.form]
        ::
        [%2 * *]
      =^  subn  bare  $(form +<.form)
      ?:  ?=([%boom ~] subn)
        [[%boom ~] bare]
      =^  forn  bare  $(form +>.form)
      ?:  ?=([%boom ~] forn)
        [[%boom ~] bare]
      ?:  ?=  [%safe %dice ~]  forn
        [[%boom ~] bare]
      ?:  ?=  [%safe %flip ~]  forn
        [[%boom ~] bare]
      ?:  ?=  [%risk %dice ~]  forn
        [[%boom ~] bare]
      ?:  ?=  [%risk %flip ~]  forn
        [[%boom ~] bare]
      ?+  forn  [[%risk %toss ~] bare(ind .+(ind.bare))]
          ::
          [%safe %know *]
        =.  dir.bare  .+(dir.bare)
        ?-  subn
            ::
            [%safe *]
          =/  nubs  sure.subn
          =/  norm  know.sure.forn
          =/  mem  (~(get by ward.bare) [nubs norm])
          ?.  ?=(~ mem)  [u.mem bare]
          =.  ward.bare  (~(put by ward.bare) [nubs norm] [%risk %toss ~])
          =^  r  bare  $(subj nubs, form norm)
          [r bare(ward (~(put by ward.bare) [nubs norm] r))]
            ::
            [%risk *]
          =/  nubs  hope.subn
          =/  norm  know.sure.forn
          =/  mem  (~(get by ward.bare) [nubs norm])
          ?.  ?=(~ mem)  [u.mem bare]
          =.  ward.bare  (~(put by ward.bare) [nubs norm] [%risk %toss ~])
          =^  r  bare  $(subj nubs, form norm)
          [(dare r) bare(ward (~(put by ward.bare) [nubs norm] (dare r)))] :: XX fix up ward modifications
        ==
          ::
          [%risk %know *]
        =.  dir.bare  .+(dir.bare)
        ?-  subn
            ::
            [%safe *]
          =/  nubs  sure.subn
          =/  norm  know.hope.forn
          =/  mem  (~(get by ward.bare) [nubs norm])
          ?.  ?=(~ mem)  [u.mem bare]
          =.  ward.bare  (~(put by ward.bare) [nubs norm] [%risk %toss ~])
          =^  r  bare  $(subj nubs, form norm)
          [(dare r) bare(ward (~(put by ward.bare) [nubs norm] (dare r)))]
            ::
            [%risk *]
          =/  nubs  hope.subn
          =/  norm  know.hope.forn
          =/  mem  (~(get by ward.bare) [nubs norm])
          ?.  ?=(~ mem)  [u.mem bare]
          =.  ward.bare  (~(put by ward.bare) [nubs norm] [%risk %toss ~])
          =^  r  bare  $(subj nubs, form norm)
          [(dare r) bare(ward (~(put by ward.bare) [nubs norm] (dare r)))]
        ==
      ==
        ::
        [%3 *]
      =^  s  bare  $(form +.form)
      :_  bare
      (ques s)
        ::
        [%4 *]
      =^  s  bare  $(form +.form)
      :_  bare
      (pile s)
        ::
        [%5 * *]
      =^  l  bare  $(form +<.form)
      =^  r  bare  $(form +>.form)
      :_  bare
      (bopp l r)
        ::
        [%6 * * *]
      =^  cond  bare  $(form +<.form)
      ?+  cond  [[%boom ~] bare]
          ::
          [%safe *]
        ?+  sure.cond  [[%boom ~] bare]
            ::
            [%know %0]
          $(form +>-.form)
            ::
            [%know %1]
          $(form +>+.form)
            ::
            [%flip ~]
          =^  t  bare  $(form +>-.form)
          =^  f  bare  $(form +>+.form)
          :_  bare
          (gnaw t f)
            ::
            [%dice ~]
          =^  t  bare  $(form +>-.form)
          =^  f  bare  $(form +>+.form)
          :_  bare
          (dare (gnaw t f))
            ::
            [%toss ~]
          =^  t  bare  $(form +>-.form)
          =^  f  bare  $(form +>+.form)
          :_  bare
          (dare (gnaw t f))
        ==
          ::
          [%risk *]
        ?+  hope.cond  [[%boom ~] bare]
            ::
            [%know %0]
          =^  t  bare  $(form +>-.form)
          :_  bare
          (dare t)
            ::
            [%know %1]
          =^  f  bare  $(form +>+.form)
          :_  bare
          (dare f)
            ::
            [%flip ~]
          =^  t  bare  $(form +>-.form)
          =^  f  bare  $(form +>+.form)
          :_  bare
          (dare (gnaw t f))
            ::
            [%dice ~]
          =^  t  bare  $(form +>-.form)
          =^  f  bare  $(form +>+.form)
          :_  bare
          (dare (gnaw t f))
            ::
            [%toss ~]
          =^  t  bare  $(form +>-.form)
          =^  f  bare  $(form +>+.form)
          :_  bare
          (dare (gnaw t f))
        ==
      ==
        ::
        [%7 * *]
      =^  news  bare  $(form +<.form)
      ?+  news  [[%boom ~] bare]
          ::
          [%safe *]
        $(subj sure.news, form +>.form)
          ::
          [%risk *]
        =^  r  bare  $(subj hope.news, form +>.form)
        :_  bare
        (dare r)
      ==
        ::
        [%8 * *]
      =^  news  bare  $(form +<.form)
      ?+  news  [[%boom ~] bare]
          ::
          [%safe *]
        $(subj (knit sure.news subj), form +>.form)
          ::
          [%risk *]
        =^  r  bare  $(subj (knit hope.news subj), form +>.form)
        :_  bare
        (dare r)
      ==
        ::
        [%9 @ *]
      =^  news  bare  $(form +>.form)
      ?+  news  [[%boom ~] bare]
          ::
          [%safe *]
        =/  newf  (pull +<.form sure.news)
        ?+  newf  [[%boom ~] bare]
            ::
            [%safe %know *]
          =.  dir.bare  .+(dir.bare)
          =/  nubs  sure.news
          =/  norm  know.sure.newf
          =/  mem  (~(get by ward.bare) [nubs norm])
          ?.  ?=(~ mem)  [u.mem bare]
          =.  ward.bare  (~(put by ward.bare) [nubs norm] [%risk %toss ~])
          =^  r  bare  $(subj nubs, form norm)
          :_  bare(ward (~(put by ward.bare) [nubs norm] r))
          r
            ::
            [%risk %know *]
          =.  dir.bare  .+(dir.bare)
          =/  nubs  sure.news
          =/  norm  know.hope.newf
          =/  mem  (~(get by ward.bare) [nubs norm])
          ?.  ?=(~ mem)  [u.mem bare]
          =.  ward.bare  (~(put by ward.bare) [nubs norm] [%risk %toss ~])
          =^  r  bare  $(subj nubs, form norm)
          :_  bare(ward (~(put by ward.bare) [nubs norm] (dare r)))
          (dare r)
            ::
            [%safe *]
          =.  ind.bare  .+(ind.bare)
          [[%risk %toss ~] bare]
            ::
            [%risk *]
          =.  ind.bare  .+(ind.bare)
          [[%risk %toss ~] bare]
        ==
          ::
          [%risk *]
        =/  newf  (pull +<.form hope.news)
        ?+  newf  [[%boom ~] bare]
            ::
            [%safe %know *]
          =.  dir.bare  .+(dir.bare)
          =/  nubs  hope.news
          =/  norm  know.sure.newf
          =/  mem  (~(get by ward.bare) [nubs norm])
          ?.  ?=(~ mem)  [u.mem bare]
          =.  ward.bare  (~(put by ward.bare) [nubs norm] [%risk %toss ~])
          =^  r  bare  $(subj nubs, form norm)
          :_  bare(ward (~(put by ward.bare) [nubs norm] (dare r)))
          (dare r)
            ::
            [%risk %know *]
          =.  dir.bare  .+(dir.bare)
          =/  nubs  hope.news
          =/  norm  know.hope.newf
          =/  mem  (~(get by ward.bare) [nubs norm])
          ?.  ?=(~ mem)  [u.mem bare]
          =.  ward.bare  (~(put by ward.bare) [nubs norm] [%risk %toss ~])
          =^  r  bare  $(subj nubs, form norm)
          :_  bare(ward (~(put by ward.bare) [nubs norm] (dare r)))
          (dare r)
            ::
            [%safe *]
          =.  ind.bare  .+(ind.bare)
          [[%risk %toss ~] bare]
            ::
            [%risk *]
          =.  ind.bare  .+(ind.bare)
          [[%risk %toss ~] bare]
        ==
      ==
        ::
        [%10 [@ *] *]
      =^  p  bare  $(form +<+.form)
      =^  w  bare  $(form +>.form)
      :_  bare
      (welt +<-.form p w)
        ::
        [%11 @ *]
      $(form +>.form)
        ::
        [%11 [* *] *]
      =^  hint  bare  $(form +<+.form)
      ?+  hint  [[%boom ~] bare]
          ::
          [%safe *]
        $(form +>.form)
          ::
          [%risk *]
        =^  r  bare  $(form +<.form)
        :_  bare
        (dare r)
      ==
        ::
        [%12 *]
      [[%risk %toss ~] bare]
    ==
  --
++  cuff
  |=  =sock
  =/  axe  1
  |-
  ^-  (list @)
  ?-  sock
      ::
      [%know *]
    (limo [axe ~])
      ::
      [%bets *]
    (weld $(axe (add axe axe), sock hed.sock) $(axe (add (add axe axe) 1), sock tal.sock))
      ::
      [%dice ~]
    (limo [axe ~])
      ::
      [%flip ~]
    (limo [axe ~])
      ::
      [%toss ~]
    (limo [axe ~])
  ==
--
