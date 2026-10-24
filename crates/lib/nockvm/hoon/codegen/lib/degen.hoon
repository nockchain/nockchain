:: XX this whole thing is a mess and needs one more rewrite
/-  *sock
/-  *gene
/+  ska
=|  burg=town
|%
++  vent
  |=  barn
  [sub for 1 %vent]
++  dole
  |=  barn
  [sub for 1 %dole]
++  mill  ::  XX todo observe crashes
  =*  this  .
  |=  [ject=* gist=barn]
  ^-  [* _this]
  =|  quay=(list [curb=berm sign=(map @ *) vale=@])
  =^  [goes=lake uses=pool rump=@]  this  (belt gist)
  =/  sign  (~(put by *(map @ *)) rump ject)
  =/  reed  (~(got by goes) (vent gist))
  |^  ^-  [* _this]
    ?~  body.reed
      ?-  -.bend.reed
          %clq
        ?@  (loan +<.bend.reed)
          (lump +>+.bend.reed)
        (lump +>-.bend.reed)
      ::
          %eqq
        ~!  +<.bend.reed
        ~!  +>-.bend.reed
        ?:  =((loan +<.bend.reed) (loan +>-.bend.reed))
          (lump +>+<.bend.reed)
        (lump +>+>.bend.reed)
      ::
          %brn
        ?:  =(0 (loan +<.bend.reed))
          (lump +>-.bend.reed)
        ?:  =(1 (loan +<.bend.reed))
          (lump +>+.bend.reed)
        ~|  %bad-bean  !!
      ::
          %hop  (lump +.bend.reed)
          %lnk
        =/  gunk  `barn`[[%toss ~] (loan +<.bend.reed)]
        =^  [goop=lake ruse=pool rump=@]  this
          (belt [%toss ~] (loan +<.bend.reed))
        %=  $
          quay  [[+>+>.bend.reed sign +>+<.bend.reed] quay]
          goes  goop
          sign  (lend +>-.bend.reed rump)
        ==
      ::
          %cal
        =/  [goop=lake ruse=pool rump=@]  does:(~(got by land.burg) +<.bend.reed)
        %=  $
          quay  [[+>+>.bend.reed sign +>+<.bend.reed] quay]
          goes  goop
          sign  (yoke +>-.bend.reed ruse)
          reed  (~(got by goop) (vent +<.bend.reed))
        ==
      ::
          %bec  ~|  %bec-slip  !!
          %lnt
        =^  [goop=lake ruse=pool rump=@]  this
          (belt [%toss ~] (loan +<.bend.reed))
        ~!  +>.bend.reed
        %=  $
          goes  goop
          sign  (lend +>.bend.reed rump)
        ==
      ::
          %jmp
        =/  [goop=lake ruse=pool rump=@]  does:(~(got by land.burg) +<.bend.reed)
        %=  $
          goes  goop
          sign  (yoke +>.bend.reed ruse)
        ==
      ::
          %eye  ~|  %eye-slip  !!
          %spy  ~|  %fbi  !!
          %hnt  ?>((~(has by sign) +<.bend.reed) (lump +>.bend.reed))
          %don
        ?~  quay  [(loan +.bend.reed) this]
        =/  rail  [sub for]:curb.i.quay
        =/  [goop=lake ruse=pool bump=@]  does:(~(got by land.burg) rail)
        %=  $
          sign  (~(put by sign.i.quay) vale.i.quay (loan +.bend.reed))
          goes  goop
          reed  ~|(%miss-entry (~(got by goes) curb.i.quay))
          quay  t.quay
        ==
      ::
          %bom
        ~|  %boom  !!
      ==
    %=    $
        body.reed  t.body.reed
        sign
      %-  ~(put by sign)
      ?-  -.i.body.reed
          %imm  [+> +<]:i.body.reed 
          %mov
        :-  +>.i.body.reed
        (loan +<.i.body.reed)
      ::
          %inc
        :-  +>.i.body.reed
        =/  bink  (loan +<.i.body.reed)
        ?>  ?=(@ bink)
        .+(bink)
      ::
          %unc
        :-  +>.i.body.reed
        =/  bink  (loan +<.i.body.reed)
        ?>  ?=(@ bink)
        .+(bink)
      ::
          %con
        :-  +>+.i.body.reed
        :-  (loan +<.i.body.reed)
        (loan +>-.i.body.reed)
      ::
          %hed
        =/  cash  (loan +<.i.body.reed)
        ?>  ?=(^ cash)
        [+>.i.body.reed -.cash]
      ::
          %hud
        =/  cash  (loan +<.i.body.reed)
        ?>  ?=(^ cash)
        [+>.i.body.reed -.cash]
      ::
          %tal
        =/  cash  (loan +<.i.body.reed)
        ?>  ?=(^ cash)
        [+>.i.body.reed +.cash]
      ::
          %tul
        =/  cash  (loan +<.i.body.reed)
        ?>  ?=(^ cash)
        [+>.i.body.reed +.cash]
      ==
    ==
  ++  loan
    |=  @
    ~|  %loan-miss  (~(got by sign) +<)
  ++  lend
    |=  [src=@ dst=@]
    ^-  _sign
    (~(put by `_sign`~) dst (loan src))
  ++  lump
    |=  berm
    ^$(reed ~|(%miss-entry (~(got by goes) +<)))
  ++  yoke
    |=  [ox=(list @) lo=pool]
    =|  link=(map @ *)
    |-  ^-  (map @ *)
    ?~  ox
      ?~  lo  link
      ~|  %yoke-match  !!
    ?~  lo
      ~|  %yoke-match  !!
    $(link (~(put by link) ssa.i.lo (loan i.ox)), ox t.ox, lo t.lo)
  --
++  belt
  =*  this  .
  |=  gist=barn
  ^-  [rice _this]
  =.  this  +:(reap gist)
  :_  this
  does:(~(got by land.burg) gist)
++  reap
  =*  this  .
  |=  =barn
  ^-  [boot _this]
  =/  [=boot =farm]  (plot barn)
  =^  work  this  (till farm)
  :-  boot
  (weed:(rake:this work) work)
++  plot  ::  subject knowledge analysis, emitting nock-- or "nomm"
  =*  this  .
  =|  ski=farm
  |=  ent=barn
  ^-  [boot farm]
  =/  bot  (~(get by land.burg) ent)
  ?.  ?=(~ bot)  [says.u.bot ski]  ::  no need to re-plot a barn we already know
  =/  ext  (~(get by yard.ski) ent)
  ?.  ?=(~ ext)  [says.u.ext ski]
  =;  [res=[does=nomm says=boot:ska] sku=farm]
    [says.res sku(yard (~(put by yard.sku) ent res), wood [ent wood.sku])]
  :: blackhole, guard recursion
  =.  ski  ski(yard (~(put by yard.ski) ent [[%zer 0 %.n] [%risk %toss ~]]))
  |-  ^-  [[does=nomm says=boot:ska] farm]
  =<
  ?+  for.ent  bomb
      [[* *] *]
    =^  [doth=nomm sath=boot:ska]  ski  $(for.ent -.for.ent)
    ?:  ?=([%boom ~] sath)  bomb
    =^  [toes=nomm tays=boot:ska]  ski  $(for.ent +.for.ent)
    ?:  ?=([%boom ~] tays)  bomb
    :_  ski
    :_  (cobb:ska sath tays)
    [%par doth toes]
  ::
      [%0 @]
    ::  we can decompose the axis into two axes, a safe axis which can
    ::  be implemented unchecked, and an unsafe axis which must be
    ::  checked. We then compose these two axes into safe %zer and
    ::  unsafe %zer composed by %sev
    =+  [saf rik ken]=(punt:ska +.for.ent sub.ent)
    ?:  =(0 saf)  bomb
    :_  ski
    ?:  =(1 rik)  [[%zer saf %.y] [%safe ken]]
    ?:  =(1 saf)  [[%zer rik %.n] [%risk ken]]
    :_  [%risk ken]
    [%sev [%zer saf %.y] [%zer rik %.n]]
      ::
      [%1 *]
    :_  ski
    :_  [%safe %know +.for.ent]
    [%one +.for.ent]
  ::
      [%2 * *]
    =^  [dost=nomm sass=boot:ska]  ski  $(for.ent +<.for.ent)
    ?:  ?=([%boom ~] sass)  bomb
    =^  [doff=nomm faff=boot:ska]  ski  $(for.ent +>.for.ent)
    ?:  ?=([%boom ~] faff)  bomb
    =/  skun
      ?-  sass
        [%safe *]  sure.sass
        [%risk *]  hope.sass
      ==
    ?:  ?=([%safe %know *] faff)
      =^  ret  ski  ^$(ent [skun know.sure.faff])
      :_  ski
      :_  ?:  ?=([%safe *] sass)  ret  (dare:ska ret)
      [%two dost doff skun (some know.sure.faff) %.y]
    ?:  ?=([%risk %know *] faff)
      =^  ret  ski  ^$(ent [skun know.hope.faff])
      :_  ski
      :_  (dare:ska ret)
      [%two dost doff skun (some know.hope.faff) %.n]
    :_  ski
    :_  [%risk %toss ~]
    [%two dost doff skun ~ %.n]
  ::
      [%3 *]
    =^  [deft=nomm koob=boot:ska]  ski  $(for.ent +.for.ent)
    ?:  ?=([%boom ~] koob)  bomb
    :_  ski
    :_  (ques:ska koob)
    [%thr deft]
  ::
      [%4 *]
    =^  [dink=nomm sink=boot:ska]  ski  $(for.ent +.for.ent)
    ?:  ?=([%boom ~] sink)  bomb
    =/  rink
      ?-  sink
        [%safe *]  sure.sink
        [%risk *]  hope.sink
      ==
    :_  ski
    :_  (pile:ska sink)
    [%fou dink ?|(?=([%dice ~] rink) ?=([%flip ~] rink) ?=([%know @] rink))]
  ::
      [%5 * *]
    =^  [dome=nomm foam=boot:ska]  ski  $(for.ent +<.for.ent)
    ?:  ?=([%boom ~] foam)  bomb
    =^  [doot=nomm foot=boot:ska]  ski  $(for.ent +>.for.ent)
    ?:  ?=([%boom ~] foot)  bomb
    :_  ski
    :_  (bopp:ska foam foot)
    [%fiv dome doot]
  ::
      [%6 * * *]
    =^  [dawn=nomm sond=boot:ska]  ski  $(for.ent +<.for.ent)
    ?:  ?=([%safe %know %0] sond)  $(for.ent +>-.for.ent)
    ?:  ?=([%safe %know %1] sond)  $(for.ent +>+.for.ent)
    ?:  ?=([%safe %know *] sond)  bomb
    ?:  ?=([%safe %bets *] sond)  bomb
    ?:  ?=([%safe %flip ~] sond)
      =^  [drew=nomm slew=boot:ska]  ski  $(for.ent +>-.for.ent)
      =^  [darn=nomm song=boot:ska]  ski  $(for.ent +>+.for.ent)
      :_  ski
      :_  (gnaw:ska slew song)
      [%six dawn drew darn]
    ?:  ?=([%risk %know %0] sond)
      =^  [drew=nomm slew=boot:ska]  ski  $(for.ent +>-.for.ent)
      :_  ski
      :_  (dare:ska slew)
      ::  run dawn in case it crashes, but throw it away
      [%sev [%par dawn drew] [%zer 3 %.y]]
    ?:  ?=([%risk %know %1] sond)
      =^  [darn=nomm song=boot:ska]  ski  $(for.ent +>+.for.ent)
      :_  ski
      :_  (dare:ska song)
      ::  run dawn in case it crashes, but throw it away
      [%sev [%par dawn darn] [%zer 3 %.y]]
    ?:  ?=([%risk %know *] sond)  bomb
    ?:  ?=([%risk %bets *] sond)  bomb
    =^  [drew=nomm slew=boot:ska]  ski  $(for.ent +>-.for.ent)
    =^  [darn=nomm song=boot:ska]  ski  $(for.ent +>+.for.ent)
    :_  ski
    :_  (dare:ska (gnaw:ska slew song))
    [%six dawn drew darn]
  ::
      [%7 * *]
    =^  [deck=nomm keck=boot:ska]  ski  $(for.ent +<.for.ent)
    ?:  ?=([%boom ~] keck)  bomb
    =/  news
      ?-  keck
        [%safe *]  sure.keck
        [%risk *]  hope.keck
      ==
    =^  [dest=nomm zest=boot:ska]  ski  $(sub.ent news, for.ent +>.for.ent)
    ?:  ?=([%boom ~] zest)  bomb
    :_  ski
    :_  ?:  ?=([%safe *] keck)  zest  (dare:ska zest)
    [%sev deck dest]
  ::
      [%8 * *]
    =^  [pink=nomm pest=boot:ska]  ski  $(for.ent +<.for.ent)
    ?:  ?=([%boom ~] pest)  bomb
    =/  nest
      ?-  pest
        [%safe *]  sure.pest
        [%risk *]  hope.pest
      ==
    =^  [dest=nomm zest=boot:ska]  ski
      $(sub.ent (knit:ska nest sub.ent), for.ent +>.for.ent)
    ?:  ?=([%boom ~] zest)  bomb
    :_  ski
    :_  ?:  ?=([%safe *] pest)
          zest
        (dare:ska zest)
    [%sev [%par pink %zer 1 %.y] dest]
  ::
      [%9 @ *]
    =^  [lore=nomm sore=boot:ska]  ski  $(for.ent +>.for.ent)
    ?:  ?=([%boom ~] sore)  bomb
    =/  news
      ?-  sore
        [%safe *]  sure.sore
        [%risk *]  hope.sore
      ==
    =/  fork  (pull:ska +<.for.ent news)
    ?:  ?=([%safe %know *] fork)
      =^  ret  ski  ^$(ent [news know.sure.fork])
      :_  ski
      :_  ?:  ?=([%safe *] sore)
            ret
          (dare:ska ret)
      [%sev lore [%two [%zer 1 %.y] [%zer +<.for.ent %.y] news (some know.sure.fork) %.y]]
    ?:  ?=([%risk %know *] fork)
      =^  ret  ski  ^$(ent [news know.hope.fork])
      :_  ski
      :_  (dare:ska ret)
      [%sev lore [%two [%zer 1 %.y] [%zer +<.for.ent %.n] news (some know.hope.fork) %.n]]
    :_  ski
    :_  [%risk %toss ~]
    [%sev lore [%two [%zer 1 %.y] [%zer +<.for.ent ?=(%safe -.fork)] news ~ ?=(%safe -.fork)]]
  ::
      [%10 [@ *] *]
    =^  [neat=nomm seat=boot:ska]  ski  $(for.ent +>.for.ent)
    ?:  ?=([%boom ~] seat)  bomb
    =^  [pace=nomm spat=boot:ska]  ski  $(for.ent +<+.for.ent)
    ?:  ?=([%boom ~] spat)  bomb
    =/  teak
      ?-  seat
        [%safe *]  sure.seat
        [%risk *]  hope.seat
      ==
    =+  [saf rik ken]=(punt:ska +<-.for.ent teak)
    ?:  =(0 saf)  bomb
    :_  ski
    :_  (welt:ska +<-.for.ent spat seat)
    ?:  =(1 rik)
      [%ten [+<-.for.ent pace] neat %.y]
    ^-  nomm
    :+  %sev  [%par neat pace]
    :+  %ten
      [saf %ten [rik %zer 3 %.n] [%zer (peg saf 2) %.y] %.y]
    [[%zer 2 %.y] %.y]
  ::
      [%11 @ *]
    =^  [real=nomm seal=boot:ska]  ski  $(for.ent +>.for.ent)
    ?:  ?=([%boom ~] seal)  bomb
    ^-  [[does=nomm says=boot:ska] farm]
    [[[%els +<.for.ent real] seal] ski]
  ::
      [%11 [@ *] *]
    =^  [fake=nomm sake=boot:ska]  ski  $(for.ent +<+.for.ent)
    ?:  ?=([%boom ~] sake)  bomb
    =^  [real=nomm seal=boot:ska]  ski  $(for.ent +>.for.ent)
    ?:  ?=([%boom ~] seal)  bomb
    :_  ski
    ?:  ?=([%safe *] sake)
      [[%eld [+<-.for.ent fake] real %.y] seal]
    [[%eld [+<-.for.ent fake] real %.n] seal]
  ::
      [%12 * *]
    =^  [fear=nomm sear=boot:ska]  ski  $(for.ent +<.for.ent)
    ?:  ?=([%boom ~] sear)  bomb
    =^  [pack=nomm sack=boot:ska]  ski  $(for.ent +>.for.ent)
    ?:  ?=([%boom ~] sack)  bomb
    :_  ski
    :_  [%risk %toss ~]
    [%twe fear pack]
  ==
  |%
  ++  bomb
    ^-  [[nomm boot:ska] farm]
    [[[%zer 0 %.n] [%boom ~]] ski]
  --
++  till
  =*  this  .
  |=  =farm
  ^-  [(list barn) _this]
  =/  work  (flop (skip wood.farm ~(has by land.burg)))
  :-  work
  |-  ^-  _this
  ?~  work  this
  =/  next  i.work
  =+  ~|  %next-miss  (~(got by yard.farm) next)
  ::  now we have the nock-- in does
  =/  dock  [lamb=lamb.burg lake=*lake]
  =|  flow=line
  =/  axle=@  1
  =/  fawn  does
  |^
    =-  =.  lamb.burg  lamb.dock
        =.  land.burg
          %+  ~(put by land.burg)  next
          =/  flue  (~(got by lake.dock) her)
          :_  says
          ~|  ~(key by lake.dock)
          =.  lake.dock  (~(put by (~(del by lake.dock) her)) (vent next) flue)
          ~|  ~(key by lake.dock)
          =.  ^dock  dock
          =^  [hose=@ bole=berm]  dock  (peel hat (vent next))
          ~|  ~(key by lake.dock)
          ~|  bole
          =/  alms  (~(got by lake.dock) bole)
          =.  lake.dock  (~(put by (~(del by lake.dock) bole)) (dole next) alms)
          :-  lake.dock
          :_  hose
          =|  safe=? :: XX state maximal safe axes, as this will overly pessimize
          =/  bolt=@  1
          |-  ^-  (list [@ @ ?])
          ?-  -.hat
              %tine  [[bolt +.hat safe]]~
              %disc  ~
              %fork
            %+  weld
              $(hat left.hat, bolt (peg bolt 2), safe ?&(safe safe.hat))
            $(hat rite.hat, bolt (peg bolt 3), safe ?&(safe safe.hat))
          ==
        ^$(work t.work)
    |-  ^-  [[hat=plow her=berm] dock=_dock]
    ?-    fawn
        [%par * *]
      =^  [one=plow two=plow her=berm]  dock  twin
      =^  [bat=plow bit=berm]  dock
        $(fawn +>.fawn, axle (peg axle 3), flow [%moat her two])
      =^  [hat=plow hit=berm]  dock
        $(fawn +<.fawn, axle (peg axle 2), flow [%moat bit one])
      (copy hat bat hit)
    ::
        [%zer *]
      ?-    -.flow
          %moat
        =/  slow  (take +<.fawn what.flow +>.fawn)
        ?~  slow
          fail
        :_  dock
        [u.slow wher.flow]
      ::
          %rift
        =^  miff  dock  wean
        =/  slow  (take +<.fawn [%tine miff] +>.fawn)
        ?~  slow
          fail
        =^  her  dock  (mend %miff ~ [%brn miff [troo fals]:flow])
        :_  dock
        [u.slow her]
      ::
          %pond
        =^  tend  dock  wean
        =/  slow  (take +<.fawn [%tine tend] +>.fawn)
        ?~  slow
          fail
        =^  her  dock  (mend %tend ~ [%don tend])
        :_  dock
        [u.slow her]
      ==
    ::
        [%one *]
      (bang +.fawn)
    ::
        [%two *]
      ?-    -.flow
          %moat
        =^  flaw  dock  (peel what.flow wher.flow)
        (tool `flaw +.fawn)
      ::
          %rift
        =^  muse  dock  wean
        =^  skit  dock  (mend %skit ~ [%brn muse [troo fals]:flow])
        (tool `[muse skit] +.fawn)
      ::
          %pond
        (tool ~ +.fawn)
      ==
    ::
        [%thr *]
      ?-    -.flow
          %moat
        ?-    -.what.flow
            %fork  fail
            %disc  $(fawn +.fawn, axle (peg axle 3))
            %tine
          =^  pear  dock  (mend %pear [%imm 0 +.what.flow]~ [%hop wher.flow])
          =^  bock  dock  (mend %bock [%imm 1 +.what.flow]~ [%hop wher.flow])
          =^  noon  dock  wean
          =^  keck  dock  (mend %keck ~ [%clq noon pear bock])
          $(fawn +.fawn, axle (peg axle 3), flow [%moat keck [%tine noon]])
        ==
      ::
          %rift
        =^  noon  dock  wean
        =^  keck  dock  (mend %keck ~ [%clq noon [troo fals]:flow])
        $(fawn +.fawn, axle (peg axle 3), flow [%moat keck [%tine noon]])
      ::
          %pond
        =^  tend  dock  wean
        =^  pear  dock  (mend %pear [%imm 0 tend]~ [%don tend])
        =^  bock  dock  (mend %bock [%imm 1 tend]~ [%don tend])
        =^  noon  dock  wean
        =^  keck  dock  (mend %keck ~ [%clq noon pear bock])
        $(fawn +.fawn, axle (peg axle 3), flow [%moat keck [%tine noon]])
      ==
    ::
        [%fou *]
      ?-    -.flow
          %moat
        ?-    -.what.flow
            %fork  fail
            %disc
          =^  left  dock  wean
          ?:  +>.fawn  :: safe?
            $(fawn +<.fawn, axle (peg axle 6), flow [%moat wher.flow [%tine left]])
          =^  meal  dock  wean
          =^  dink  dock  (mend %dink ~[[%inc meal left]] [%hop wher.flow])
          $(fawn +<.fawn, axle (peg axle 6), flow [%moat dink [%tine meal]])
        ::
            %tine
          =^  meal  dock  wean
          =^  rink  dock
            ?:  +>.fawn
              (mend %rink ~[[%unc meal +.what.flow]] [%hop wher.flow])
            (mend %rink ~[[%inc meal +.what.flow]] [%hop wher.flow])
          $(fawn +<.fawn, axle (peg axle 6), flow [%moat rink [%tine meal]])
        ==
      ::
          %rift
        =^  iffy  dock  wean
        =^  miff  dock  wean
        =^  kink  dock
          ?:  +>.fawn  :: safe?
            (mend %kink ~[[%unc miff iffy]] [%brn iffy [troo fals]:flow])
          (mend %kink ~[[%inc miff iffy]] [%brn iffy [troo fals]:flow])
        $(fawn +<.fawn, axle (peg axle 6), flow [%moat kink [%tine miff]])
      ::
          %pond
        =^  pend  dock  wean
        =^  spin  dock  wean
        =^  pink  dock
          ?:  +>.fawn  :: safe?
            (mend %pink ~[[%unc spin pend]] [%don pend])
          (mend %pink ~[[%inc spin pend]] [%don pend])
        $(fawn +<.fawn, axle (peg axle 6), flow [%moat pink [%tine spin]])
      ==
    ::
        [%fiv *]
      ?-    -.flow
          %moat
        ?-    -.what.flow
            %fork  fail
            %disc
          =^  [hit=plow his=berm]  dock  $(fawn +<.fawn, axle (peg axle 6))
          =^  [hot=plow hog=berm]  dock
            $(fawn +<.fawn, axle (peg axle 7), flow [%moat his [%disc ~]])
          (copy hit hot hog)
        ::
            %tine
          =^  root  dock  (mend %root ~[[%imm 0 +.what.flow]] [%hop wher.flow]) 
          =^  salt  dock  (mend %salt ~[[%imm 1 +.what.flow]] [%hop wher.flow])
          =^  load  dock  wean
          =^  toad  dock  wean
          =^  qual  dock  (mend %qual ~ [%eqq load toad root salt])
          =^  [hit=plow his=berm]  dock
            $(fawn +<.fawn, axle (peg axle 6), flow [%moat qual [%tine load]])
          =^  [hot=plow hog=berm]  dock
            $(fawn +>.fawn, axle (peg axle 7), flow [%moat his [%tine toad]])
          (copy hit hot hog)
        ==
      ::
          %rift
        =^  load  dock  wean
        =^  toad  dock  wean
        =^  rail  dock  (mend %rail ~ [%eqq load toad [troo fals]:flow])
        =^  [hit=plow his=berm]  dock
          $(fawn +<.fawn, axle (peg axle 6), flow [%moat rail [%tine load]])
        =^  [hot=plow hog=berm]  dock
          $(fawn +>.fawn, axle (peg axle 7), flow [%moat his [%tine toad]])
        (copy hit hot hog)
      ::
          %pond
        =^  bean  dock  wean
        =^  root  dock  (mend %root ~[[%imm 0 bean]] [%don bean]) 
        =^  salt  dock  (mend %salt ~[[%imm 1 bean]] [%don bean])
        =^  load  dock  wean
        =^  toad  dock  wean
        =^  fall  dock  (mend %fall ~ [%eqq load toad root salt])
        =^  [hit=plow his=berm]  dock
          $(fawn +<.fawn, axle (peg axle 6), flow [%moat fall [%tine load]])
        =^  [hot=plow hog=berm]  dock
          $(fawn +>.fawn, axle (peg axle 7), flow [%moat his [%tine toad]])
        (copy hit hot hog)
      ==
    ::
        [%six *]
      =^  [hut=plow hum=berm]  dock  $(fawn +>-.fawn, axle (peg axle 14))
      =^  [hat=plow ham=berm]  dock  $(fawn +>+.fawn, axle (peg axle 15))
      =^  [hot=plow hog=berm]  dock
        $(fawn +<.fawn, axle (peg axle 6), flow [%rift hum ham])
      =^  [hit=plow him=berm]  dock  (copy hut hat hog)
      (copy hit hot him)
    ::
        [%sev *]
      =^  [hit=plow his=berm]  dock  $(fawn +>.fawn, axle (peg axle 7))
      $(fawn +<.fawn, axle (peg axle 6), flow [%moat his hit])
    ::
        [%ten *]
      ?-    -.flow
          %moat
        =^  [out=plow inn=plow tub=berm]  dock  (tear +<-.fawn what.flow +>+.fawn wher.flow)
        =^  [hat=plow him=berm]  dock  $(fawn +<+.fawn, axle (peg axle 13), flow [%moat tub inn])
        =^  [hut=plow mud=berm]  dock  $(fawn +>-.fawn, axle (peg axle 14), flow [%moat him out])
        (copy hat hut mud)
      ::
          %rift
        :: this is a weird case. It only works if the axis is one,
        :: otherwise it crashes, and there's no point in an axis edit of
        :: one except to discard the first result
        ?.  =(1 +<-.fawn)  fail
        =^  hide  dock  wean
        =^  mood  dock  (mend %mood ~ [%brn hide [troo fals]:flow])
        =^  [hat=plow him=berm]  dock
          $(fawn +<+.fawn, axle (peg axle 13), flow [%moat mood [%tine hide]])
        =^  [hut=plow mud=berm]  dock
          $(fawn +>-.fawn, axle (peg axle 14), flow [%moat him [%disc ~]])
        (copy hat hut mud)
      ::
        %pond
        =^  dire  dock  wean
        =^  eden  dock  (mend %eden ~ [%don dire])
        =^  [out=plow inn=plow tub=berm]  dock  (tear +<-.fawn [%tine dire] +>+.fawn eden)
        =^  [hat=plow him=berm]  dock  $(fawn +<+.fawn, axle (peg axle 13), flow [%moat tub inn])
        =^  [hut=plow mud=berm]  dock  $(fawn +>-.fawn, axle (peg axle 14), flow [%moat him out])
        (copy hat hut mud)
      ==
    ::
        [%els *]
      =^  [hat=plow him=berm]  dock  $(fawn +>.fawn, axle (peg axle 7))
      =^  pint  dock  wean
      =^  tint  dock  (mend %tint ~[[%imm +<.fawn pint]] [%hnt pint him])
      :_  dock
      [hat tint]
    ::
        [%eld *]
      =^  [hat=plow him=berm]  dock  $(fawn +>-.fawn, axle (peg axle 7))
      =^  pint  dock  wean
      =^  dint  dock  wean
      =^  aint  dock  wean
      =^  tint  dock  (mend %tint ~[[%imm +<-.fawn pint] [%con pint dint aint]] [%hnt aint him])
      =^  [hit=plow his=berm]  dock  $(fawn +<+.fawn, axle (peg axle 13), flow [%moat tint [%tine dint]])
      (copy hat hit his)
    ::
        [%twe *]
      ?-    -.flow
          %moat
        =^  [use=@ her=berm]  dock  (peel what.flow wher.flow)
        =^  fens  dock  wean
        =^  phat  dock  wean
        =^  cope  dock  (mend %cope ~ [%spy fens phat use her])
        =^  [ham=plow pan=berm]  dock
          $(fawn +>.fawn, axle (peg axle 7), flow [%moat cope [%tine phat]])
        =^  [hen=plow pen=berm]  dock
          $(fawn +<.fawn, axle (peg axle 6), flow [%moat pan [%tine fens]])
        (copy ham hen pen)
      ::
          %rift
        =^  sift  dock  wean
        =^  bars  dock  (mend %bars ~ [%brn sift [troo fals]:flow])
        =^  fens  dock  wean
        =^  phat  dock  wean
        =^  cope  dock  (mend %cope ~ [%spy fens phat sift bars])
        =^  [ham=plow pan=berm]  dock
          $(fawn +>.fawn, axle (peg axle 7), flow [%moat cope [%tine phat]])
        =^  [hen=plow pen=berm]  dock
          $(fawn +<.fawn, axle (peg axle 6), flow [%moat pan [%tine fens]])
        (copy ham hen pen)
      ::
          %pond
        =^  sped  dock  wean
        =^  sear  dock  (mend %sear ~ [%don sped])
        =^  fens  dock  wean
        =^  phat  dock  wean
        =^  cope  dock  (mend %cope ~ [%spy fens phat sped sear])
        =^  [ham=plow pan=berm]  dock
          $(fawn +>.fawn, axle (peg axle 7), flow [%moat cope [%tine phat]])
        =^  [hen=plow pen=berm]  dock
          $(fawn +<.fawn, axle (peg axle 6), flow [%moat pan [%tine fens]])
        (copy ham hen pen)
      ==
    ==
  ++  fail
    ^-  [[hat=plow her=berm] dock=_dock]
    =^  hole  dock  bomb
    :_  dock
    [[%disc ~] hole]
  ++  tear :: take apart an ssa map for an edit
    |=  [axe=@ bit=plow safe=? her=berm]
    ^-  [[out=plow inn=plow his=berm] _dock]
    ?:  =(0 axe)
      =^  hole  dock  bomb
      [[[%disc ~] [%disc ~] hole] dock]
    =+
      |-  ^-  [[out=plow inn=plow rind=(list bran)] deck=_dock]
      ?:  =(1 axe)
        :_  dock
        [[%disc ~] bit ~]
      ?-  -.bit
          %disc
        ?:  safe  [[[%disc ~] [%disc ~] ~] dock]
        ?-  (cap axe)
            %2
          =^  ruck  dock  $(axe (mas axe))
          :_  dock
          [[%fork out.ruck [%disc ~] %.n] inn.ruck rind.ruck]
            %3
          =^  ruck  dock  $(axe (mas axe))
          :_  dock
          [[%fork [%disc ~] out.ruck %.n] inn.ruck rind.ruck]
        ==
      ::
          %tine
        =^  tour  dock  wean
        =^  plat  dock  wean
        ?-  (cap axe)
            %2
          =^  ruck  dock  $(axe (mas axe), bit [%tine plat])
          :_  dock
          [[%fork out.ruck [%tine tour] safe] inn.ruck [[%con plat tour +.bit] rind.ruck]]
            %3
          =^  ruck  dock  $(axe (mas axe), bit [%tine plat])
          :_  dock
          [[%fork [%tine tour] out.ruck safe] inn.ruck [[%con tour plat +.bit] rind.ruck]]
        ==
      ::
          %fork
        ?-  (cap axe)
            %2
          =^  ruck  dock  $(axe (mas axe), bit left.bit)
          :_  dock
          [[%fork out.ruck rite.bit ?&(safe safe.bit)] inn.ruck rind.ruck]
            %3
          =^  ruck  dock  $(axe (mas axe), bit rite.bit)
          :_  dock
          [[%fork left.bit out.ruck ?&(safe safe.bit)] inn.ruck rind.ruck]
        ==
      ==
    =.  dock  deck
    ?~  rind
      :_  dock
      [out inn her]
    =^  him  dock  (mend %diet rind [%hop her])
    :_  dock
    [out inn him]
  ++  tool :: generate calls
    |=  [flaw=(unit [rut=@ rot=berm]) sums=nomm form=nomm sunk=sock fork=(unit *) safe=?]
    ^-  [[plow berm] _dock]
    ?~  fork
      =^  lash  dock  wean
      =^  frog  dock  wean
      =^  coil  dock
        ?~  flaw
          (mend %coil ~ [%lnt frog lash])
        (mend %coil ~ [%lnk frog lash rut.u.flaw rot.u.flaw])
      =^  [bow=plow urn=berm]  dock
        $(fawn sums, axle (peg axle 6), flow [%moat coil [%tine lash]])
      =^  [fog=plow sog=berm]  dock
        $(fawn form, axle (peg axle 14), flow [%moat urn [%tine frog]])
      (copy fog bow sog)
    =/  bale=barn  [sunk u.fork]
    =/  bore  (~(get by land.burg) bale)
    ?~  bore :: we don't know the registerization of the subject for the call, yet
      =^  lash  dock  wean
      =^  dote  dock
        ?~  flaw
          (mend %dote ~ [%eye bale lash])
        (mend %dote ~ [%bec bale lash rut.u.flaw rot.u.flaw])
      =^  [bow=plow urn=berm]  dock
        $(fawn sums, axle (peg axle 6), flow [%moat dote [%tine lash]])
      ?:  safe  [[bow urn] dock]
      =^  [fog=plow sog=berm]  dock
        $(fawn form, axle (peg axle 14), flow [%moat urn [%disc ~]])
      (copy fog bow sog)
    =^  uses  dock  (cool uses.does.u.bore)
    =^  dote  dock
      ?~  flaw
        (mend %dote ~ [%jmp bale (boil uses)])
      (mend %dote ~ [%cal bale (boil uses) rut.u.flaw rot.u.flaw])
    =^  [ash=plow dot=berm]  dock  (whop uses dote)
    =^  [bow=plow urn=berm]  dock
      $(fawn sums, axle (peg axle 6), flow [%moat dot ash])
    ?:  safe  [[bow urn] dock]
    =^  [fog=plow sog=berm]  dock
      $(fawn form, axle (peg axle 14), flow [%moat urn [%disc ~]])
    (copy fog bow sog)
  ++  cool :: generate SSAs for the call side of a use list
    |=  use=(list [@ @ ?])
    ^-  [(list [@ @ ?]) _dock]
    ?~  use  [~ dock]
    =^  pan  dock  wean
    =^  lid  dock  $(use t.use)
    :_  dock
    [[-.i.use pan +>.i.use] lid]
  ++  boil :: ssas from a use list
    |=  use=(list [@ @ ?])
    ^-  (list @)
    (turn use |=([@ ssa=@ ?] ssa))
  ++  whop  ::  turn a use list into a plow
    |=  [use=(list [@ @ ?]) her=berm]
    ^-  [[plow berm] _dock]
    ?~  use  [[*plow her] dock]
    =^  [low=plow him=berm]  dock  $(use t.use)
    =/  ace  (take -.i.use [%tine +<.i.use] +>.i.use)
    ?~  ace  fail
    (copy low u.ace him)
  ++  bang
    |=  non=*
    ^-  [[hat=plow her=berm] _dock]
    ?-  flow
        [%pond ~]
      =^  ret  dock  wean
      =^  her  dock  (mend %rime ~[[%imm +.fawn ret]] [%don ret])
      :_  dock
      [[%disc ~] her]
    ::
        [%rift *]
      ?:  =(0 +.fawn)  [[[%disc ~] troo.flow] dock]
      ?:  =(1 +.fawn)  [[[%disc ~] fals.flow] dock]
      :: XX maybe we should assert that SKA should have caught this?
      =^  hole  dock  bomb
      :_  dock
      [[%disc ~] hole]
    ::
        [%moat *]
      =/  what  what.flow
      =/  mitt
        |-  ^-  (unit (list bran))
        ?-    what
            [%disc ~]
          (some ~)
        ::
            [%tine @]
          (some ~[[%imm non +.what]])
        ::
            [%fork *]
          ?@  non
            ?:  safe.what
              ~|  %safe-axis-atom  !!
            ~
          (clap $(what left.what, non -.non) $(what rite.what, non +.non) weld)
        ==
      ?~  mitt
        =^  hole  dock  bomb
        :_  dock
        [[%disc ~] hole]
      =^  rock  dock  (mend %toil u.mitt [%hop wher.flow])
      :_  dock
      [[%disc ~] rock]
    ==
 ++  take  :: axis
    |=  [sax=@ tow=plow row=?]  :: axis, destination, safety
    ^-  (unit plow)  :: nullary case = crash
    ?:  =(0 sax)  ~
    %-  some
    |-  ^-  plow
    ?:  =(1 sax)  tow
    ?-  (cap sax)
      %2  [%fork $(sax (mas sax)) [%disc ~] row]
      %3  [%fork [%disc ~] $(sax (mas sax)) row]
    ==
  ++  copy  :: replicate values to two destinations
    |=  [hat=plow bat=plow her=berm]
    ^-  [[hat=plow her=berm] _dock]
    =^  [tog=plow moot=(list bran)]  dock
      |-
      ^-  [[tog=plow moot=(list bran)] _dock]
      ?:  ?=([%disc ~] hat)  [[bat ~] dock]
      ?:  ?=([%disc ~] bat)  [[hat ~] dock]
      ?-  hat
          [%tine @]
        ?-  bat
            [%tine @]
          ?:  =(+.hat +.bat)
            [[hat ~] dock]
          [[hat ~[[%mov +.hat +.bat]]] dock]
            ::
            [%fork *]
          =^  one  dock  wean
          =^  two  dock  wean
          =^  [hog=plow hoot=(list bran)]  dock
            $(hat [%tine one], bat left.bat)
          =^  [log=plow loot=(list bran)]  dock
            $(hat [%tine two], bat rite.bat)
          :_  dock
          :-  ^-  plow
              [%fork hog log safe.bat]
          [[%con one two +.hat] (weld hoot loot)]
        ==
          ::
          [%fork *]
        ?-  bat
            [%tine @]
          =^  one  dock  wean
          =^  two  dock  wean
          =^  [hog=plow hoot=(list bran)]  dock
            $(hat left.hat, bat [%tine one])
          =^  [log=plow loot=(list bran)]  dock
            $(hat rite.hat, bat [%tine two])
          :_  dock
          [[%fork hog log safe.hat] [%con one two +.bat] (weld hoot loot)]
        ::
            [%fork *]
          =^  [hog=plow hoot=(list bran)]  dock  $(hat left.hat, bat left.bat)
          =^  [log=plow loot=(list bran)]  dock  $(hat rite.hat, bat rite.bat)
          :_  dock
          [[%fork hog log ?&(safe.hat safe.bat)] (weld hoot loot)]
        ==
      ==
    =/  blab  (milk %copy)
    :_  dock(lake (~(put by lake.dock) blab [moot %hop her]))
    [tog blab]
  ++  twin  :: split sans from flow
    ^-  [[plow plow berm] _dock]
    ?-  flow
        [%rift *]
      =^  hole  dock  bomb
      :_  dock
      [[%disc ~] [%disc ~] hole]
        ::
        [%pond ~]
      =^  one  dock  wean
      =^  two  dock  wean
      =^  ret  dock  wean
      =^  her  dock  (mend %taco ~[[%con one two ret]] [%don ret])
      :_  dock
      [[%tine one] [%tine two] her]
        ::
        [%moat *]
      ?-  what.flow
          [%fork *]
        :_  dock
        [left.what.flow rite.what.flow wher.flow]
      ::
          [%disc ~]
        :_  dock
        [[%disc ~] [%disc ~] wher.flow]
      ::
          [%tine @]
        =^  one  dock  wean
        =^  two  dock  wean
        =^  her  dock
          (mend %cons ~[[%con one two +.what.flow]] [%hop wher.flow])
        :_  dock
        [[%tine one] [%tine two] her]
      ==
    ==
  ++  bomb
    ^-  [berm _dock]
    (mend %boom ~ [%bom ~])
  ++  milk  :: local label
    |=  gen=@
    ^-  berm
    ~!  next
    [sub.next for.next axle gen]
  ++  mend
    |=  [gen=@ =lock]
    ^-  [berm _dock]
    =/  curb  (milk gen)
    :-  curb
    dock(lake (~(put by lake.dock) curb lock))
  ++  wean  :: fresh ssa
    ^-  [@ _dock]
    [lamb.dock dock(lamb .+(lamb.dock))]
  ++  peel  :: split a define among a plow's worth of uses
    |=  [mole=plow hill=berm]
    ^-  [[use=@ her=berm] _dock]
    ~&  ~(key by lake.dock)
    =+
      |-  ^-  [[fine=(unit @) load=(list bran)] dock=_dock]
      ?-    -.mole
          %tine  [[`+.mole ~] dock]
          %disc  [[~ ~] dock]
          %fork
        =^  [file=(unit @) loaf=(list bran)]  dock  $(mole left.mole)
        =^  [fire=(unit @) road=(list bran)]  dock  $(mole rite.mole)
        ?~  file
          ?~  fire
            [[~ ~] dock]
          [[fire road] dock]
        ?~  fire
          [[file loaf] dock]
        =^  fell  dock  wean
        ?:  safe.mole
          :_  dock
          :-  `fell
          [[%hud fell u.file] [%tul fell u.fire] (weld loaf road)]
        :_  dock
        :-  `fell
        [[%hed fell u.file] [%tal fell u.fire] (weld loaf road)]
      ==
    ?~  fine
      =^  crap  dock  wean  :: no uses in the plow, so just make a trash register for the result and return
      =^  her  dock  (mend %peel ~ [%hop hill])
      [[crap her] dock]
    =^  her  dock  (mend %peel load [%hop hill])  ::  loads necessary, add those to the dock and return
    [[u.fine her] dock]
 
  --
++  rake  ::  clean up unused basic blocks, and rewrite bec/eye into cal/jmp
  =*  this  .
  |=  work=(list barn)
  ^-  _this
  ?~  work  this
  %=  $
      burg
    =+  ~|  %barn-miss  (~(got by land.burg) i.work)
    ^-  town
    =|  loch=lake
    =|  sigh=(map @ $%([%mov @] [%con @ @] [%rug ~]))
    =/  tack=[(list berm) (list berm)]  [[(vent i.work) ~] ~] :: label queue
    |-  ^-  town  ::  loop over basic blocks using a queue
    ?~  -.tack
      ?~  +.tack
        %=    burg
            land
          (~(put by land.burg) i.work [[loch uses.does lump.does] says]) 
        ==
      $(tack [(flop +.tack) ~])
    =/  hock  ~|  %miss-berm  ~|  i.-.tack  (~(got by goes.does) i.-.tack)
    =/  bock  body.hock
    |^  ^-  town  ::  loop over instructions in a basic block
      ?~  body.hock
        ?:  ?=(%bec -.bend.hock)
          (rend [+< +>- `+>+]:bend.hock)
        ?:  ?=(%eye -.bend.hock)
          (rend [+< +> ~]:bend.hock)
        =.  loch  (~(put by loch) i.-.tack [bock bend.hock])
        ?-  bend.hock
            [%clq *]
          ^$(-.tack t.-.tack, +.tack [+>-.bend.hock +>+.bend.hock +.tack])
        ::
            [%eqq *]
          ^$(-.tack t.-.tack, +.tack [+>+<.bend.hock +>+>.bend.hock +.tack])
        ::
            [%brn *]
          ^$(-.tack t.-.tack, +.tack [+>-.bend.hock +>+.bend.hock +.tack])
        ::
            [%hop *]
          ^$(-.tack t.-.tack, +.tack [+.bend.hock +.tack])
        ::
            [%lnk *]
          %=  ^$
            sigh  (~(put by sigh) +>+<.bend.hock [%rug ~])
            -.tack  t.-.tack
            +.tack  [+>+>.bend.hock +.tack]
          ==
        ::
            [%cal *]
          %=  ^$
            sigh  (~(put by sigh) +>+<.bend.hock [%rug ~])
            -.tack  t.-.tack
            +.tack  [+>+>.bend.hock +.tack]
          ==
        ::
            [%lnt *]  ^$(-.tack t.-.tack)
            [%jmp *]  ^$(-.tack t.-.tack)
            [%spy *]
          %=  ^$
            sigh  (~(put by sigh) +>+<.bend.hock [%rug ~])
            -.tack  t.-.tack
            +.tack  [+>+>.bend.hock +.tack]
          ==
        ::
            [%hnt *]
          ^$(-.tack t.-.tack, +.tack [+>.bend.hock +.tack])
        ::
            [%don *]  ^$(-.tack t.-.tack)
            [%bom *]  ^$(-.tack t.-.tack)
        ==
      ?-  i.body.hock
          [%imm *]  ::  XX we should split immediates too
        $(body.hock t.body.hock, sigh (~(put by sigh) +>.i.body.hock [%rug ~]))
      ::
          [%mov *]
        %=  $
          body.hock  t.body.hock
          sigh  (~(put by sigh) +>.i.body.hock [%mov +<.i.body.hock])
        ==
      ::
          [%inc *]
        $(body.hock t.body.hock, sigh (~(put by sigh) +>.i.body.hock [%rug ~]))
      ::
          [%unc *]
        $(body.hock t.body.hock, sigh (~(put by sigh) +>.i.body.hock [%rug ~]))
      ::
          [%con *]
        %=  $
          body.hock  t.body.hock
          sigh
            %+  ~(put by sigh)
              +>+.i.body.hock
            [%con +<.i.body.hock +>-.i.body.hock]
        ==
      ::
          [%hed @ @]
        $(body.hock t.body.hock, sigh (~(put by sigh) +>.i.body.hock [%rug ~]))
      ::
          [%hud @ @]
        $(body.hock t.body.hock, sigh (~(put by sigh) +>.i.body.hock [%rug ~]))
      ::
          [%tal @ @]
        $(body.hock t.body.hock, sigh (~(put by sigh) +>.i.body.hock [%rug ~]))
      ::
          [%tul @ @]
        $(body.hock t.body.hock, sigh (~(put by sigh) +>.i.body.hock [%rug ~]))
      ==
    ++  rend  ::  make register assignments to translate a bec/eye into a cal/jmp.
      |=  [=barn tart=@ poem=(unit [@ berm])]
      =/  uses  ~|  %uses-miss  uses:does:(~(got by land.burg) barn)
      ^-  town
      =-
        =.  burg  fort
        =?  sigh  ?=([~ *] poem)  (~(put by sigh) -.u.poem [%rug ~])
        =/  term
          ?~  poem
            [%jmp barn bits]
          [%cal barn bits u.poem] 
        %=    ^^$
            loch
          (~(put by loch) i.-.tack [(weld bock bins) term])
        ::
          -.tack  t.-.tack
        ==
      =/  gasp  :: turn the sigh register-relating map into a register-for-axis map
        =/  axe  1
        |-  ^-  (map @ @)
        =/  waft  (~(put by *(map @ @)) axe tart) 
        =/  puff  (~(gut by sigh) tart [%rug ~])
        ?-    puff
            [%rug ~]  waft
            [%mov *]  (~(uni by waft) $(tart +.puff))
            [%con *]
          =/  left  $(tart +<.puff, axe (peg axe 2))
          %-  ~(uni by waft)
          %-  ~(uni by left)
          $(tart +>.puff, axe (peg axe 3))
        ==
      =|  bits=(list @)
      =|  bins=(list bran)
      |-  ^-  [bits=(list @) bins=(list bran) fort=town]
      ?~  uses  [(flop bits) bins burg]
      =/  sour  -.i.uses
      =/  axle  1
      =/  vale  ~|  %vale-miss  (~(got by gasp) 1)
      |-  ^-  [bits=(list @) bins=(list bran) fort=town]
      ?:  =(1 sour)
        ^$(bits [vale bits], uses t.uses)
      ?-    (cap sour)
          %2
        =.  axle  (peg axle 2)
        =.  sour  (mas sour)
        =/  pale  (~(get by gasp) axle)
        ?~  pale
          %=    $
            bins  [[%hed vale lamb.burg] bins]
            vale  lamb.burg
            gasp  (~(put by gasp) axle lamb.burg)
            lamb.burg  .+(lamb.burg)
          ==
        $(vale u.pale)
      ::
          %3
        =.  axle  (peg axle 3)
        =.  sour  (mas sour)
        =/  pale  (~(get by gasp) axle)
        ?~  pale
          %=    $
            bins  [[%tal vale lamb.burg] bins]
            vale  lamb.burg
            gasp  (~(put by gasp) axle lamb.burg)
            lamb.burg  .+(lamb.burg)
          ==
        $(vale u.pale)
      ==
    --
  ::
      work  t.work
  ==
++  weed :: remove unused safe operations (imm,mov,unc,con,hud,tul)
  =*  this  .
  |=  work=(list barn) 
  ^-  _this
  ?~  work  this
  =/  herd  (~(got by land.burg) i.work)  ::  sack for this arm
  =|  dead=(jug berm @)  ::  values used by a label and its successor code
  =/  furs=(list berm)  [[sub for 1 %vent]:i.work ~]
  |-  ^-  _this
  ?~  furs
    ^$(work t.work, land.burg (~(put by land.burg) i.work herd))
  ?:  (~(has by dead) i.furs)  :: did we already analyze this arm
    $(furs t.furs)
  =/  meat  (~(got by goes.does.herd) i.furs)
  |^
    ?-  -.bend.meat
        %clq
      =/  troo  (~(get by dead) +>-.bend.meat)
      ?~  troo  $(furs [+>-.bend.meat furs])
      =/  fals  (~(get by dead) +>+.bend.meat)
      ?~  fals  $(furs [+>+.bend.meat furs])
      ~!  u.troo
      ~!  u.fals
      ~!  +<.bend.meat
      (vein (~(uni in u.troo) (~(put in u.fals) +<.bend.meat)))
    ::
        %eqq
      =/  troo  (~(get by dead) +>+<.bend.meat)
      ?~  troo  $(furs [+>+<.bend.meat furs])
      =/  fals  (~(get by dead) +>+>.bend.meat)
      ?~  fals  $(furs [+>+>.bend.meat furs])
      (vein (~(uni in u.troo) (~(gas in u.fals) [+<.bend.meat +>-.bend.meat ~])))
    ::
        %brn
      =/  troo  (~(get by dead) +>-.bend.meat)
      ?~  troo  $(furs [+>-.bend.meat furs])
      =/  fals  (~(get by dead) +>+.bend.meat)
      ?~  fals  $(furs [+>+.bend.meat furs])
      (vein (~(uni in u.troo) (~(put in u.fals) +<.bend.meat)))
    ::
        %hop
      =/  want  (~(get by dead) +.bend.meat)
      ?~  want  $(furs [+.bend.meat furs])
      (vein u.want)
    ::
        %lnk
      =/  want  (~(get by dead) +>+>.bend.meat)
      ?~  want  $(furs [+>+>.bend.meat furs])
      (vein (~(gas in u.want) [+<.bend.meat +>-.bend.meat ~]))
    ::
        %cal
      =/  want  (~(get by dead) +>+>.bend.meat)
      ?~  want  $(furs [+>+>.bend.meat furs])
      (vein (~(gas in u.want) +>-.bend.meat))
    ::
        %bec
      ~|  %bec-trip  !!
    ::
        %lnt
      (vein (silt [+<.bend.meat]~))
    ::
        %jmp
      (vein (silt +>.bend.meat))
    ::
        %eye
      ~|  %eye-trip  !!
    ::
        %spy
      =/  want  (~(get by dead) +>+>.bend.meat)
      ?~  want  $(furs [+>+>.bend.meat furs])
      (vein (~(gas in u.want) [+<.bend.meat +>-.bend.meat ~]))
    ::
        %hnt
      =/  want  (~(get by dead) +>.bend.meat)
      ?~  want  $(furs [+>.bend.meat furs])
      (vein (~(put in u.want) +<.bend.meat))
    ::
        %don
      (vein (silt [+.bend.meat]~))
    ::
        %bom
      (vein ~)
    ==
  ++  vein
    |=  uses=(set @)
    =/  boyd  (flop body.meat)
    =|  bond=(list bran)
    |-  ^-  _this
    ~!  goes.does.herd
    ~!  i.furs
    ?~  boyd
      %=    ^^^$
          furs  t.furs
          goes.does.herd
        (~(put by goes.does.herd) i.furs [bond bend.meat])
          dead
        (~(put by dead) i.furs uses)
      ==
    ?-  -.i.boyd
        %imm
      ?:  (~(has in uses) +>.i.boyd)
        $(bond [i.boyd bond], boyd t.boyd)
      $(boyd t.boyd)
    ::
        %mov
      ?:  (~(has in uses) +>.i.boyd)
        $(bond [i.boyd bond], boyd t.boyd, uses (~(put in uses) +<.i.boyd))
      $(boyd t.boyd)
    ::
        %inc
      $(bond [i.boyd bond], boyd t.boyd, uses (~(put in uses) +<.i.boyd))
    ::  
        %unc
      ?:  (~(has in uses) +>.i.boyd)
        $(bond [i.boyd bond], boyd t.boyd, uses (~(put in uses) +<.i.boyd))
      $(boyd t.boyd)
    ::
        %con
      ?:  (~(has in uses) +>+.i.boyd)
        %=  $
          bond  [i.boyd bond]
          boyd  t.boyd
          uses  (~(gas in uses) [+<.i.boyd +>-.i.boyd ~])
        ==
      $(boyd t.boyd)
    ::
        %hed
      $(bond [i.boyd bond], boyd t.boyd, uses (~(put in uses) +<.i.boyd))
    ::
        %hud
      ?:  (~(has in uses) +>.i.boyd) 
        $(bond [i.boyd bond], boyd t.boyd, uses (~(put in uses) +<.i.boyd))
      $(boyd t.boyd)
    ::
        %tal
      $(bond [i.boyd bond], boyd t.boyd, uses (~(put in uses) +<.i.boyd))
    ::
        %tul
      ?:  (~(has in uses) +>.i.boyd) 
        $(bond [i.boyd bond], boyd t.boyd, uses (~(put in uses) +<.i.boyd))
      $(boyd t.boyd)
    ==
  --
--
