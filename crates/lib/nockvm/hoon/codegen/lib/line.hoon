/-  *sock
/-  *gene
|%
++  thy  ::  generation and interpretation
  |_  burg=town
  +*  this  .
  ++  till
    |=  =farm
    ^-  [(list barn) _this]
    =/  work  (flop (skip wood.farm ~(has by land.burg)))
    :-  work
    |-  ^-  _this  ::  work loop
      ?~  work  this
      =+  ~|  %next-miss  (~(got by yard.farm) i.work)
      =/  dock  [lamb=lamb.burg lake=*lake]
      =|  flow=line
      =/  axle=@  1
      =/  fawn  does
      |^
        =^  [lout=plow tern=berm]  dock  rive
        =^  greg=@  dock  (vert lout tern)
        %=    ^$
            work  t.work
            lamb.burg  lamb.dock
            land.burg
          %+  ~(put by land.burg)  i.work
          :_  says
          [lake.dock (cite lout) greg]
        ==
      ++  rive  :: linearize nock
        ^-  [[hat=plow her=berm] dock=_dock]
        ?-    -.fawn
            %par
          =^  [one=plow two=plow her=berm]  dock  twin
          =^  [bat=plow bit=berm]  dock
            rive(fawn +>.fawn, axle (peg axle 3), flow [%moat her two])
          =^  [hat=plow hit=berm]  dock
            rive(fawn +<.fawn, axle (peg axle 2), flow [%moat bit one])
          (copy hat bat hit)
        ::
            %zer
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
            =^  her  dock  (mend %miff ~ %brn miff [troo fals]:flow)
            :_  dock
            [u.slow her]
          ::
              %pond
            =^  tend  dock  wean
            =/  slow  (take +<.fawn [%tine tend] +>.fawn)
            ?~  slow
              fail
            =^  her  dock  (mend %tend ~ %don tend)
            :_  dock
            [u.slow her]
          ==
        ::
            %one
          (bang +.fawn)
        ::
            %two
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
            %thr
          ?-    -.flow
              %moat
            ?-    -.what.flow
                %fork  fail
                %disc  rive(fawn +.fawn, axle (peg axle 3))
                %tine
              =^  pear  dock  (mend %pear [%imm 0 +.what.flow]~ [%hop wher.flow]
              =^  bock  dock  (mend %bock [%imm 1 +.what.flow]~ [%hop wher.flow]
              =^  noon  dock  wean
              =^  keck  dock  (mend %keck ~ %clq noon pear bock)
              rive(fawn +.fawn, axle (peg axle 3), flow [%moat keck %tine noon])
            ==
          ::
              %rift
            =^  noon  dock  wean
            =^  keck  dock  (mend %keck ~ %cloq noon [troo fals]:flow)
            rive(fawn +.fawn, axle (peg axle 3), flow [%moat keck %tine noon])
          ::
              %pond
            =^  tend  dock  wean
            =^  pear  dock  (mend %pear [%imm 0 tend]~ %don tend)
            =^  bock  dock  (mend %bock [%imm 1 tend]~ %don tend)
            =^  noon  dock  wean
            =^  keck  dock  (mend %keck ~ %clq noon pear bock)
            rive(fawn +.fawn, axle (peg axle 3), flow [%moat keck %tine noon])
          ==
        ::
            %fou
          ?-    -.flow
              %moat
            ?-  what.flow
                %fork  fail
                %disc
              ?:  +>.fawn
                rive(fawn +<.fawn, axle (peg axle 3))
              =^  left  dock  wean
              =^  meal  dock  wean
              =^  dink  dock  (mend %dink  [[%inc meal left]]~ %hop wher.flow)
              rive(fawn +<.fawn, axle (peg axle 3), flow [%moat dink %tine meal])
            ::  
                %tine
              =^  meal  dock  wean
              =^  rink  dock
                ?:  +>.fawn
                  (mend %rink [[%unc meal +.what.flow]]~ %hop wher.flow)
                (mend %rink [[%inc meal +.what.flow]]~ %hop wher.flow)
              rive(fawn +<.fawn, axle (peg axle 3), flow [%moat rink %tine meal])
            ==
          ::
              %rift
            =^  iffy  dock  wean
            =^  miff  dock  wean
            =^  kink  dock
              ?:  +>.fawn
                (mend %kink [[%unc miff iffy]]~ %brn iffy [troo fals]:flow)
              (mend %kink  [[%inc miff iffy]]~ %brn iffy [troo fals]:flow)
            rive(fawn +<.fawn, axle (peg axle 3), flow [%moat kink %tine miff])
          ::
              %pond
            =^  pend  dock  wean
            =^  spin  dock  wean
            =^  pink  dock
              ?:  +>.fawn
                (mend %pink [[%unc spin pend]] %don pend)
              (mend %pink [[%inc spin pend]] %don pend)
            rive(fawn +<.fawn, axle (peg axle 3), flow [%moat pink %tin spin])
          ==
        ::
            %fiv
          ?-    -.flow
              %moat
            ?-    -.what.flow
                %fork  fail
                %disc
              =^  [hit=plow his=berm]  dock
                rive(fawn +<.fawn, axle (peg axle 6))
              =^  [hit=plow his=berm]  dock
                rive(fawn +>.fawn, axle (peg axle 7), flow [%moat his %disc ~)
              (copy hit hot hog)
            ::
                %tine
              =^  root  dock
                (mend %root [[%imm 0 +.what.flow]]~ %hop wher.flow)
              =^  salt  dock
                (mend %salt [[%imm 0 +.what.flow]]~ %hop wher.flow)
              =^  load  dock  wean
              =^  toad  dock  wean
              =^  qual  dock
                (mend %qual ~ %eqq load toad root salt)
              =^  [hit=plow his=berm]  dock
                %=  rive
                  fawn  +<.fawn
                  axle  (peg axle 6)
                  flow  [%moat qual %tine load]
                ==
              =^  [hot=plow hog=berm]  dock
                %=  rive
                  fawn  +<.fawn
                  axle  (peg axle 6)
                  flow  [%moat his %tine toad]
                ==
              (copy hit hot hog)
            ==
          ::
              %rift
            =^  load  dock  wean
            =^  toad  dock  wean
            =^  rail  dock  (mend %rail ~ %eqq load toad [troo fals]:flow)
            =^  [hit=plow his=berm]  dock
              %=  rive
                fawn  +<.fawn
                axle  (peg axle 6)
                flow  [%moat rail %tine load]
              ==
            =^  [hot=plow hog=berm]  dock
              %=  rive
                fawn  +<.fawn
                axle  (peg axle 6)
                flow  [%moat his %tine toad]
              ==
            (copy hit hot hog)
          ::
              %pond
            =^  bean  dock  wean
            =^  root  dock  (mend %root [[%imm 0 bean]]~ %don bean)
            =^  salt  dock  (mend %salt [[%imm 1 bean]]~ %don bean)
            =^  load  dock  wean
            =^  toad  dock  wean
            =^  fall  dock  (mend %fall ~ %eqq load toad root salt)
            =^  [hit=plow his=berm]  dock
              %=  rive
                fawn  +<.fawn
                axle  (peg axle 6)
                flow  [%moat fall %tine load]
              ==
            =^  [hot=plow hog=berm]  dock
              %=  rive
                fawn  +>.fawn
                axle  (peg axle 7)
                flow  [%moat his %tine toad]
              ==
            (copy hit hot hog)
          ==
        ::
            %six
          =^  [hut=plow hum=berm]  dock  rive(fawn +>-.fawn, axle (peg axle 14))
          =^  [hat=plow ham=berm]  dock  rive(fawn +>+.fawn, axle (peg axle 15))
          =^  [mat=plow troo=berm fals=berm]  dock  (tamp hut hum hat ham)
          =^  [hot=plow hog=berm]  dock
            rive(fawn +<.fawn, axle (peg axle 6), flow [%rift troo fals])
          (copy hot mat hog)
        ::
            %sev
          =^  [hit=plow his=berm]  dock  rive(fawn +>.fawn, axle (peg axle 7))
          rive(fawn +<.fawn, axle (peg axle 6), flow [%moat his hit])
        ::
            %ten
          ?-    -.flow
              %moat
            =^  [out=plow inn=plow tub=berm]  dock  (tear +<-.fawn what.flow +>+.fawn wher.flow)
            =^  [hat=plow him=berm]  dock  rive(fawn +<+.fawn, axle (peg axle 13), flow [%moat tub inn])
            =^  [hut=plow mud=berm]  dock  rive(fawn +>-.fawn, axle (peg axle 14), flow [%moat him out])
            (copy hat hut mud)
          ::
              %rift
            :: this is a weird case. It only works with axis 1.
            :: Otherwise it crashes.
            :: The only use of axis 1 edit is to discard the outer
            :: result.
            ?.  =(1 +<-.fawn)  fail
            =^  hide  dock  wean
            =^  mood  dock  (mend %mood ~ %brn hide [troo fals]:flow)
            =^  [hat=plow him=berm]  dock
              rive(fawn +<+.fawn, axle (peg axle 13), flow [%moat mood %tine hide])
            =^  [hut=plow mud=berm]  dock
              rive(fawn +>-.fawn, axle (peg axle 14), flow [%moat him [%disc ~]])
            (copy hat hut mud)
          ::
              %pond
            =^  dire  dock  wean
            =^  eden  dock  (mend %eden ~ [%don dire])
            =^  [out=plow inn=plow tub=berm]  dock  (tear +<-.fawn [%tine dire] +>+.fawn eden)
            =^  [hat=plow him=berm]  dock  rive(fawn +<+.fawn, axle (peg axle 13), flow [%moat tub inn])
            =^  [hut=plow mud=berm]  dock  rive(fawn +>-.fawn, axle (peg axle 14), flow [%moat him out])
            (copy hat hut mud)
          ==
        ::
            %els
          =^  [hat=plow him=berm]  dock  rive(fawn +>.fawn, axle (peg axle 7)) 
          =^  pint  dock  wean
          =^  tint  dockk (mend %tint [[%imm +<.fawn pint]]~ %hnt pint him)
          :_  dock
          [hat tint]
        ::
            %eld
          =^  [hat=plow him=berm]  dock  rive(fawn +>-.fawn, axle (peg axle 7))
          =^  pint  dock  wean
          =^  dint  dock  wean
          =^  aint  dock  wean
          =^  tint  dock
            %:  mend
              %tint
              [[%imm +<-.fawn pint] [%con  pint dint aint]]~
              [%hnt aint him]
            ==
          =^  [hit=plow his=berm]  dock  rive(fawn +<+.fawn, axle (peg axle 13), flow [%moat tint %tine dint])
          (copy hit hat his)
        ::
            %twe
          ?-    -.flow
              %moat
            =^  [use=@ her=berm]  dock  (peel what.flow wher.flow)
            =^  fens  dock  wean
            =^  phat  dock  wean
            =^  cope  dock  (mend %cope ~ %spy fens phat use her)
            =^  [ham=plow pan=berm]  dock
              %=  rive
                fawn  +>.fawn
                axle  (peg axle 7)
                flow  [%moat cope %tine phat]
              ==
            =^  [hen=plow pen=berm]  dock
              %=  rive
                fawn  +<.fawn
                axle  (peg axle 6)
                flow  [%moat pan %tine fens]
              ==
            (copy ham hen pen)
          ::
              %pond
            =^  sped  dock  wean
            =^  sear  dock  (mend %sear ~ %don sped)
            =^  fens  dock  wean
            =^  phat  dock  wean
            =^  cope  dock  (mend %cope ~ %spy fens phat sped sear)
            =^  [ham=plow pan=berm]  dock
              %=  rive
                fawn  +>.fawn
                axle  (peg axle 7)
                flow  [%moat cope %tine phat]
              ==
            =^  [hen=plow pen=berm]  dock
              %=  rive
                fawn  +<.fawn
                axle  (peg axle 6)
                flow  [%moat pan %tine fens]
              ==
            (copy ham hen pen)
          ::
              %rift
            =^  sift  dock  wean
            =^  bars  dock  (mend %bars  ~ %brn sift [troo fals]:flow)
            =^  fens  dock  wean
            =^  phat  dock  wean
            =^  cope  dock  (mend %cope ~ %spy fens phat sift bars)
            =^  [ham=plow pan=berm]  dock
              %=  rive
                fawn  +>.fawn
                axle  (peg axle 7)
                flow  [%moat cope %tine phat]
              ==
            =^  [hen=plow pen=berm]  dock
              %=  rive
                fawn  +<.fawn
                axle  (peg axle 6)
                flow  [%moat pan %tine fens]
              ==
            (copy ham hen pen)
          ==
        ==
      ++  tool  :: codegen for calls
        |=  [flaw=(unit [rut=@ rot=berm]) sums=nomm form=nomm sunk=sock fork=(unit *) safe=?]
        ^-  [[plow berm] _dock]
        ?~  fork
          =^  lash  dock  wean
          =^  frog  dock  wean
          =^  coil  dock
            ?~  flaw
              (mend %coil ~ [%lnt frog lash])
            (mend %coil ~ [%lnk frog lash u.flaw])
          =^  [bow=plow urn=berm]  dock
            rive(fawn sums, axle (peg axle 6), flow [%moat coil %tine lash])
          =^  [fog=plow sog=berm]  dock
            rive(fawn form, axle (peg axle 7), flow [%moat urn %tine frog])
          (copy fog bow sog)
        =/  bale=barn  [sunk u.fork]
        =/  bore  (~(get by land.burg) bale)
        ?~  bore  :: no registerization info yet
          =^  lash  dock  wean
          =^  dote  dock
            ?~  flaw
              (mend %dote ~ [%eye bale lash])
            (mend %dote ~ [%bec bale lash rut.u.flaw rot.u.flaw])
          =^  [bow=plow urn=berm]  dock
            rive(fawn sums, axle (peg axle 6), flow [%mote dote %tine lash])
          ?:  safe  [[bow urn] dock]
          =^  [fog=plow sog=berm]  dock
            rive(fawn form, axle (peg axle 7), flow [%moat urn %disc ~])
          (copy fog bow sog)
        =^  uses  dock  (cool uses.does.u.bore)
        =^  dote  dock
          ?~  flaw
            (mend %dote ~ [%jmp bale (boil uses)])
          (mend %dote ~ [%cal bale (boil uses) rut.u.flaw rot.u.flaw])
        =^  [ash=plow dot=berm]  dock  (whop uses dote)
        =^  [bow=plow urn=berm]  dock
          rive(fawn sums, axle (peg axle 6), flow [%moat dot ash])
        ?:  safe  [[bow urn] dock]
        =^  [fog=plow sog=berm]  dock
          rive(fawn form, axle (peg axle 7), flow [%moat urn %disc ~])
        (copy fog bow sog)
      ++  cool  :: generate SSAs for the call side
        |=  use=(list [@ @ ?])
        ^-  [(list [@ @ ?]) _dock]
        ?~  use  [~ dock]
        =^  pan  dock  wean
        =^  lid  dock  $(use t.use)
        :_  dock
        [[-.i.use pan +>.i.use] lid]
      ++  boil  :: SSAs from a use list
        |=  use=(list [@ @ ?])
        ^-  (list @)
        (turn use |=([@ ssa=@ ?] ssa))
      ++  whop  :: turn a use list into a plow
        |=  [use=(list [@ @ ?]) her=berm]
        ^-  [[plow berm] _dock]
        ?~  use  [[*plow her] dock]
        =^  [low=plow him=berm]  dock  $(use t.use)
        =/  ace  (take -.i.use [%tine +<.i.use] +>.i.use)
        ?~  ace  fail
        (copy low u.ace him)
      ++  bang  :: distribute a constant among a plow
        |=  non=*
        ^-  [[hat=plow her=berm] _dock]
        ?-    -.flow
            %pond
          =^  ret  dock  wean
          =^  her  dock  (mend %rime [[imm +.fawn ret]]~ %don ret)
          :_  dock
          [[%disc ~] her]
        ::
            %rift
          ?:  =(0 +.fawn)  [[[%disc ~] troo.flow] dock]
          ?:  =(1 +.fawn)  [[[%disc ~] fals.flow] dock]
          fail  ::  XX ska should catch this
        ::
            %moat
          =/  what  what.flow
          |^
            =/  mitt  thud
            ?~  mitt  fail
            =^  rock  dock  (mend %toil u.mitt [%hop wher.flow])
            :_  dock
            [[%disc ~] rock]
          ++  thud
            ^-  (unit (list bran))
            ?-    -.what
                %disc  `~
                %tine  `[[%imm non +.what ~]]
                %fork
              ?@  non
                ?:  safe.what
                  ~|  %safe-axis-atom  !!
                ~
              %^    clef
                  thud(what left.what, non -.non)
                thud(what rite.what, non +.non)
              weld
            ==
          --
        ==
      ++  vert  :: add entry points
        |=  [lout=plow tern=berm]
        =^  [use=@ bull=berm]  dock  (peel lout tern)
        :-  use
        %=    dock
            lake
          %-  ~(gas by lake.dock)
          :~  
            [(vent i.work) ~ %jmp tern]
            [(dole i.work) ~ %jmp bull]
          ==
       ==
      ++  cite  :: enumerate regs
        |=  =plow
        ^-  (list @)
        ?-  -.plow
          %tine  [+.plow ~]
          %disc  ~
          %fork  (weld $(plow left.plow) $(plow rite.plow))
        ==
      ++  mend  :: add a basic block
        |=  [gen=@ =lock]
        ^-  [berm _dock]
        =/  curb  (milk gen)
        :-  curb
        dock(lake (~(put by lake.dock) curb lock))
      ++  milk  :: local label
        |=  gen=@
        ^-  berm
        [sub.next for.next axle gen]
      ++  wean :: fresh ssa
        ^-  [@ _dock]
        [lamb.dock dock(lamb .+(lamb.dock))]
      ++  peel  :: split a define among a plow of uses
        |=  [mole=plow hill=berm]
        ^-  [[use=@ her=berm] _dock]
        |^
          =^  [fine=(unit @) load=(list bran)]  dock  (pare mole)
          ?~  fine
            =^  crap  dock  wean
            =^  her  dock  (mend %peel ~ %hop hill)
            [[crap her] dock]
          ?~  load
            [[u.fine hill] dock]
          =^  her  dock  (mend %peel load %hop hill)
          [[u.fine her] dock]
      ++  pare
        |=  mole=plow
        ^-  [[fine=(unit @) load=(list bran)] dock=_dock]
        ?-    -.mole
            %tine  [[`+.mole ~] dock]
            %disc  [[~ ~] dock]
            %fork
          =^  [file=(unit @) loaf=(list bran)]  dock  $(mole left.mole)
          =^  [fire=(unit @) loaf=(list bran)]  dock  $(mole rite.mole)
          ?~  file
            ?~  fire
              [[~ ~] dock]
            [[fire road] dock]
          ?~  fire
            [[file loaf] dock]
          =^  fell  dock  wean
          :_  dock
          :-  `fell
          ?:  safe.mole
            [[%hud fell u.file] [%tul fell u.fire] (weld loaf road)]
          [[%hed fell u.file] [%al fell u.fire] (weld loaf road)]
        ==
      ++  bomb  ::  crash
        ^-  [berm _dock]
        (mend %boom ~ [%bom ~])
      ++  fail  ::  crash but yield destination
        ^-  [[hat=plow her=berm] dock=_dock]
        =^  hole  dock  bomb
        :_  dock
        [[%disc ~] hole]
      ++  tamp  :: distribute same value to plows for branch
        |=  [hat=plow her=berm cat=plow cur=berm]
        ^-  [[mat=plow troo=berm fals=berm] _dock]
        |^
          =^  [goo=plow mess=(list bran) stew=(list bran)]  dock  slop
          =^  lamp  dock  (mend %lamp mess %jmp her)
          =^  ramp  dock  (mend %ramp stew %jmp cur)
          [[goo lamp ramp] dock]
        ++  slop
          ^-  [[goo=plow mess=(list bran) stew=(list bran)] _dock]
          ?-    -.hat
              %fork
            ?-    -.cat
                %fork
              =^  [loo=plow moss=(list bran) stow=(list bran)]  dock
                slop(hat left.hat, cat left.cat)
              =^  [rue=plow ross=(list bran) thou=(list bran)]  dock
                slop(hat rite.hat, cat rite.cat)
              :_  dock
              [[%fork loo rue ?&(safe.hat safe.cat)] (weld moss ross) (weld stow thou)]
            ::
                %tine
              =^  [fine=(unit @) load=(list bran)]  dock  (pare hat)
              :_  dock
              ?~  fine
                [cat ~ ~]
              [cat [[%mov +.cat u.fine] load] ~]
            ::
                %disc
              ?:  safe.hat
                =^  [loo=plow moss=(list bran) stow=(list bran)]  dock
                  slop(hat left.hat)
                =^  [rue=plow ross=(list bran) thou=(list bran)]  dock
                  slop(hat rite.hat)
                :_  dock
                [[%fork loo rue %.y] (weld moss ross) (weld stow thou)]
              =^  [fine=(unit @) load=(list bran)]  dock  (pare cat)
              :_  dock
              ?~  fine
                [[%disc ~] ~ ~]
              [[%tine u.fine] load ~]
            ==
          ::
              %tine
            ?-    -.cat
                %fork
              =^  [fine=(unit @) load=(list bran)]  dock  (pare cat)
              :_  dock
              ?~  fine
                [hat ~ ~]
              [hat ~ [[%mov +.hat u.fine] load]]
          ::
              %disc
            ?-    -.cat
                %fork
              ?:  safe.cat
                =^  [loo=plow moss=(list bran) stow=(list bran)]  dock
                  slop(cat left.cat)
                =^  [rue=plow ross=(list bran) thou=(list bran)]  dock
                  slop(cat rite.cat)
                :_  dock
                [[%fork loo rue %.y] (weld moss ross) (weld stow thou)]
              =^  [fine=(unit @) load=(list bran)]  dock  (pare cat)
              :_  dock
              ?~  fine
                [[%disc ~] ~ ~]
              [[%tine u.fine] ~ load]
            ::
                %tine  [[cat ~ ~] dock]
                %disc  [[[%disc ~] ~ ~] dock]
            ==
          ==
        --
      ++  tear  :: split a plow for an edit
        |=  [axe=@ bit=plow safe=? her=berm]
        ^-  [[out=plow inn=plow his=berm] _dock]
        ?:  =(0 axe)
          =^  hole  dock  bomb
          [[[%disc ~] [%disc ~] hole] dock]
        |^
          =^  [out=plow inn=plow rind=(list bran)]  dock  gash
          ?~  rind
            :_  dock
            [out inn her]
          =^  him  dock  (mend %diet rind [%hop her])
          :_  dock
          [out inn him]
        ++  gash
          ?:  =(1 axe)
            :_  dock
            [[%disc ~] bit ~]
          ?-    -.bit
              %disc
            ?:  safe  [[[%disc ~] [%disc ~] ~] dock]
            ?-    (cap axe)
                %2
              =^  ruck  dock  gash(axe (mas axe))
              :_  dock
              [[%fork [%disc ~] out.ruck %.n] inn.ruck rind.ruck]
            ::
                %3
              =^  ruck  dock  gash(axe (mas axe))
              :_  dock
              [[%fork [%disc ~] out.ruck %.n] inn.ruck rind.ruck]
            ==
          ::
              %tine
            =^  tour  dock  wean
            =^  plat  dock  wean
            ?-    (cap axe)
                %2
              =^  ruck  dock  gash(axe (mas axe), bit [%tine plat])
              :_  dock
              [[%fork out.ruck [%tine tour] safe] inn.ruck [[%con plat tour +.bit] rind.ruck]]
            ::
                %3
              =^  ruck dock  gash(axe (mas axe), bit [%tine plat])
              :_  dock
              [[%fork [%tine tour] out.ruck safe] inn.ruck [[%con tour plat +.bit] rind.ruck]]
            ==
          ::
              %fork
            ?-    (cap axe)
                %2
              =^  ruck  dock  gash(axe (mas axe), bit left.bit)
              :_  dock
              [[%fork out.ruck rite.bit ?&(safe safe.bit)] inn.ruck rind.ruck]
            ::
                %3
              =^  ruck  dock  gash(axe (mas axe), bit rite.bit)
              :_  dock
              [[%fork left.bit out.ruck ?&(safe safe.bit)] inn.ruck rind.ruck]
            ==
          ==
      ++  copy  :: distribute same value to 2 plows
        |=  [hat=plow bat=plow her=berm]
        ^-  [[hat=plow her=berm] _dock]
        |^
        =^  [tog=plow moot=(list bran)] dock echo
        =^  his  dock  (mend %copy moot %hop her)
        :_  dock
        [tog blab]
        ++  echo
          ^-  [[tog=plow moot=(list bran)] _dock]
          ?:  ?=([%disc ~] hat)  [[bat ~] dock]
          ?:  ?=([%disc ~] bat)  [[hat ~] dock]
          ?-    -.hat
              %tine
            ?-    -.bat
                %tine
              ?:  =(+.hat +.bat)
                [[hat ~] dock]
              [[hat [[%mov +.hat +.bat]]~] dock]
            ::
                %fork
              =^  one  dock  wean
              =^  two  dock  wean
              =^  [hog=plow hoot=(list bran)]  dock
                echo(hat [%tine one], bat left.bat)
              =^  [hog=plow hoot=(list bran)]  dock
                echo(hat [%tine two], bat rite.bat)
              :_  dock
              :-  [%fork hog log safe.bat]
              [[%con one two +.hat] (weld hoot loot)]
            ==
          ::
              %fork
            ?-    -.bat
                %tine
              =^  one  dock  wean
              =^  two  dock  wean
              =^  [hog=plow hoot=list bran)]  dock
                echo(hat left.hat, bat [%tine one])
              =^  [log=plow loot=(list bran)]
                echo(hat rite.hat, bat [%tine two])
              :_  dock
              [[%fork hog log safe.hat] [%con one two +.bat] (weld hoot loot)]
            ::
                %fork
              =^  [hog=plow hoot=(list bran)]  dock
                echo(hat left.hat, bat left.bat)
              =^  [log=plow loot=(list bran)]  dock
                echo(hat rite.hat, bat rite.bat)
              :_  dock
              [[%fork hog log ?&(safe.hat safe.bat)] (weld hoot loot)]
            ==
          ==
        --
      ++  twin  :: split a plow to receive a cell
        ^-  [[plow plow berm] _dock]
        ?-    -.flow
            %rift
          =^  hole  dock  bomb
          :_  dock
          [[%disc ~] [%disc ~] hole]
        ::
            %pond
          =^  one  dock  wean
          =^  two  dock  wean
          =^  ret  dock  wean
          =^  her  dock  (mend %taco [[%con one two ret]]~ [%don ret])
          :_  dock
          [[%tine one] [%tine two] her]
        ::
            %moat
          ?-    -.what.flow
              %fork
            :_  dock
            [left.what.flow rite.what.flow wher.flow]
          ::
              %disc
            :_  dock
            [[%disc ~] [%disc ~] wher.flow]
          ::
              %tine
            =^  one  dock  wean
            =^  two  dock  wean
            =^  her  dock
              (mend %cons [[%con one two +.what.flow]]~ [%hop wher.flow])
            :_  dock
            [[%tine one] [%tine two] her]
          ==
        ==
      ++  take  :: push a plow down by an axis
        |=  [sax=@ tow=plow row=?]
        ^-  (unit plow)  :: null for crash
        ?:  =(0 sax)  ~
        %-  some
        |-  ^-  plow
        ?:  =(1 sax)  tow
        ?-  (cap sax)
          %2  [%fork $(sax (mas sax)) [%disc ~] row]
          %3  [%fork [%disc ~] $(sax (mas sax)) row]
        ==
      --
    --
  ++  plot  ::  subject knowledge analysis, emitting nock-- or "nomm"
    =*  this  .
    =|  ski=farm
    |=  ent=barn
    ^-  [boot farm]
    =/  bot  (~(get by land.burg) ent)
    ?.  ?=(~ bot)  [says.u.bot ski]  ::  no need to re-plot a barn
    =/  ext  (~(get by yard.ski) ent)
    ?.  ?=(~ ext)  [says.u.ext ski]
    =;  [res=[does=nomm says=boot:ska] sku=farm]
      [says.res sku(yard (~(put by yard.sku) ent res), wood [ent wood.sku])]
    =.  ski  ::  blackhole to guard recursion  
      =%    ski
          yard
        (~(put by yard.ski) ent [[%zer 0 %.n] [%risk %toss ~]]))
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
  ++  rake
    |=  work=(list barn)
    ^-  _this
    ?~  work  this
    =+  (~(got by land.burg) i.work)
    =/  ewes  [(vent i.work) ~]
    =/  bred  (set berm)
    =|  sire
      (map @ $%([%imm *] [%con @ @] [%hed @] [%tal @] [%mov @]))
    |^
      %=  ^$
        burg  cure
        work  t.work
      ==
    ++  cure
      ?~  ewes  burg
      ::  XX ewes is a queue of berms, which is populated
      ::  by control flow instructions. We keep sire which describes
      ::  the genealogy of SSA variables, so we can decompose them
      ::  if necessary. 
      

  --
++  vent  ::  entry label
  |=(barn [sub for 1 %vent])
++  dole  ::  entry label with subject in single register
  |=(barn [sub for 1 %dole])
--
