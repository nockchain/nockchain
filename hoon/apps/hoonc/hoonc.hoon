/=  *  /common/wrapper
=>
~%  %choo  ..keep  ~
|%
+$  state-0  [%0 *]
+$  state-1  [%1 *]
+$  state-2  [%2 cached-hoon=(unit (trap vase)) *]
+$  state-3  [%3 cached-hoon=(unit (trap vase)) bc=build-cache pc=parse-cache-3]
+$  state-4  [%4 cached-hoon=(unit (trap vase)) bc=build-cache pc=parse-cache]
+$  versioned-state
  $%  state-0
      state-1
      state-2
      state-3
      state-4
  ==
+$  choo-state  state-4
::
++  empty-trap-vase
  ^-  (trap vase)
  =>  vaz=!>(~)
  |.(vaz)
::
++  moat  (keep choo-state)
+$  cause
  $%  $:  %build
          pat=cord
          tex=cord
          directory=(list [cord cord])
          arbitrary=?
          dynock=?
          out=cord
      ==
      $:  %parse
          pat=cord
          tex=cord
          directory=(list [cord cord])
      ==
      [%file %write path=@t contents=@ success=?]
      [%boot hoon-txt=cord]
      [%clear ~]
  ==
::  backward-compatible %build payload shape used by older bootstrap kernels
+$  cause-legacy
  $%  $:  %build
          pat=cord
          tex=cord
          directory=(list [cord cord])
          arbitrary=?
          out=cord
      ==
      $:  %parse
          pat=cord
          tex=cord
          directory=(list [cord cord])
      ==
      [%file %write path=@t contents=@ success=?]
      [%boot hoon-txt=cord]
      [%clear ~]
  ==
+$  effect
  $%  [%file %write path=@t contents=@]
      [%exit id=@]
  ==
::
++  cause-from-legacy
  |=  legacy=cause-legacy
  ^-  cause
  ?-    -.legacy
      %build
    [%build pat.legacy tex.legacy directory.legacy arbitrary.legacy %.n out.legacy]
  ::
      %parse
    [%parse pat.legacy tex.legacy directory.legacy]
  ::
      %file
    [%file %write path.legacy contents.legacy success.legacy]
  ::
      %boot
    [%boot hoon-txt.legacy]
  ::
      %clear
    [%clear ~]
  ==
::
::
::  hash of file contents
+$  file-hash  @uvI
::
::  hash of file contents along with the merk-hash of its dependencies
+$  merk-hash  @uvI
+$  build-cache  (map merk-hash (trap vase))
::
::  $parse-cache-3: hash addressed map of preprocessed hoon files (v3 state)
+$  parse-cache-3  (map file-hash [=path pil=pile deps=(list raut)])
::  $parse-cache-entry: parse cache bucket entry (v4 state)
+$  parse-cache-entry  [=path fil=cord pil=pile deps=(list raut)]
::  $parse-cache: hash addressed map with collision buckets (v4 state)
+$  parse-cache  (map file-hash (list parse-cache-entry))
::
::  $taut: file import from /lib or /sur
+$  taut  [face=(unit term) pax=term]
::
::  $raut: resolved taut.
::    pax contains real path to file after running +get-fit
+$  raut
  [face=(unit @tas) pax=path]
::
::  $pile:  preprocessed hoon file
::
+$  pile
  $:  sur=(list taut)  ::  /-
      lib=(list taut)  ::  /+
      raw=(list [face=(unit term) pax=path])  ::  /=
      bar=(list [face=term mark=@tas =path])  ::  /*
      hax=(list taut)                         ::  /#
      =hoon
  ==
::
+$  octs  [p=@ud q=@]
::
::  $node: entry of adjacency matrix with metadata
::
+$  node
  $:  =path
      hash=@uvI           ::  hash of path contents, not to be confused with merkle dag hash
      deps=(list raut)    ::  holds only outgoing edges
      leaf=graph-leaf
      eval=?              :: whether or not to kick it
  ==
::
+$  graph-leaf
  $%  [%hoon =hoon]
      [%octs =octs]
  ==
::
::
::  $merk-dag: content-addressed map of nodes
::
::    maps content hashes to nodes. each hash is computed from the node's
::    content and the hashes of its dependencies, forming a merkle tree.
::    used to detect changes in the dependency graph and enable caching.
::
+$  merk-dag  [merk=(map merk-hash node) file=(map file-hash merk-hash)]
::
::
::  $graph-view: adjacency matrix with easier access to neighbors
::
::    used to keep track of traversal when building the merkle DAG
::
+$  graph-view  (map path (set path))
--
::
=<
%-  (moat &)
^-  fort:moat
|_  k=choo-state
+*  builder  +>
::
::  +load: upgrade from previous state
::
++  load
  |=  old=versioned-state
  ^-  choo-state
  ::
  ::  We do not use the result of the soft because
  ::  clamming (trap vase) overwrites the contents
  ::  with the bunt resulting in the honc and the build
  ::  artifacts being replaced with empty-trap-vase.
  ::
  ?~  ((soft versioned-state) old)
    ~>  %slog.[0 leaf+"choo: +load old state does not nest under versioned-state. Try booting with --new to start from scratch."]
    !!
  ?-    -.old
      %0
    ~>  %slog.[0 leaf+"update 0-to-2, starting from scratch"]
    *choo-state
  ::
      %1
    ~>  %slog.[0 leaf+"update 1-to-2, starting from scratch"]
    *choo-state
  ::
      %2
    ~>  %slog.[0 leaf+"update 2-to-3, erasing caches but keeping honc"]
    %*  .  *choo-state
      cached-hoon  cached-hoon.old
    ==
  ::
      %3
    ~>  %slog.[0 leaf+"update 3-to-4, erasing caches but keeping honc"]
    %*  .  *choo-state
      cached-hoon  cached-hoon.old
    ==
  ::
      %4
    ~>  %slog.[0 leaf+"no upgrade"]
    old
  ==
::
::  +peek: external inspect
::
++  peek
  |=  =path
  ^-  (unit (unit *))
  ?+    path  ~
      [%booted ~]
    ``?=(^ cached-hoon.k)
  ==
::
::  +poke: external apply
::
++  poke
  |=  [=wire eny=@ our=@ux now=@da dat=*]
  ^-  [(list effect) choo-state]
  =/  maybe-cause=(unit cause)  ((soft cause) dat)
  =/  cause=cause
    ?~  maybe-cause
      =/  legacy=(unit cause-legacy)  ((soft cause-legacy) dat)
      ?~  legacy
        ~&  "hoonc: warning: input is not a proper cause"
        !!
      (cause-from-legacy u.legacy)
    u.maybe-cause
  ?-    -.cause
      %file
    ?:  success.cause
      ~&  "hoonc: output written successfully to {<path.cause>}"
      [[%exit 0]~ k]
    ~&  "hoonc: failed to write output to {<path.cause>}"
    [[%exit 1]~ k]
  ::
      %clear
    [[%exit 0]~ k(pc *parse-cache, bc *build-cache)]
  ::
      %boot
    =/  cached=?  ?=(^ cached-hoon.k)
    ~&  >>  [hoon-version+hoon-version cached+cached]
    ?:  cached
      [~ k]
   [~ k(cached-hoon `(build-honc hoon-txt.cause))]
  :::
      %build
    =/  target-path=path  (parse-file-path pat.cause)
    ::
    ::  Create map of dep directory, includes target
    =/  dir
      %-  ~(gas by *(map path cord))
      :-  [target-path tex.cause]
      (turn directory.cause |=((pair @t @t) [(stab p) q]))
    ?>  ?=(^ cached-hoon.k)
    =/  [compiled=(unit *) new-bc=build-cache new-pc=parse-cache]
      %-  ~(create builder u.cached-hoon.k bc.k pc.k)
      [target-path dir arbitrary.cause dynock.cause]
    :_  k(bc new-bc, pc new-pc)
    ?~  compiled
      ~&  "hoonc: build failed, skipping write and exiting"
      [%exit 1]~
    ~&  "hoonc: build succeeded, sending out write effect"
    [%file %write path=out.cause contents=(jam u.compiled)]~
  ::
      %parse
    =/  target-path=path  (parse-file-path pat.cause)
    ::
    ::  Create map of dep directory, includes target
    =/  dir
      %-  ~(gas by *(map path cord))
      :-  [target-path tex.cause]
      (turn directory.cause |=((pair @t @t) [(stab p) q]))
    =/  parse-res  (parse-dir dir)
    =/  new-pc=parse-cache  +.parse-res
    [~ k(pc new-pc)]
  ==
--
=>
::
::  dependency system
::
~%  %dependency-system  +  ~
|%
::  +parse-file-path: parse cord of earth file path to $path
++  parse-file-path
  |=  pat=cord
  (rash pat gawp)
::
::  +gawp: parse an absolute earth file path
++  gawp
  %+  sear
    |=  p=path
    ^-  (unit path)
    ?:  ?=([~ ~] p)  `~
    ?.  =(~ (rear p))  `p
    ~
  ;~(pfix fas (most fas bic))
::
::  +bic: parse file/dir name in earth file path
++  bic
  %+  cook
  |=(a=tape (rap 3 ^-((list @) a)))
  (star ;~(pose nud low hig hep dot sig cab))
::
++  to-wain                                           ::  cord to line list
  |=  txt=cord
  ^-  wain
  ?~  txt  ~
  =/  len=@  (met 3 txt)
  =/  cut  =+(cut -(a 3, c 1, d txt))
  =/  sub  sub
  =|  [i=@ out=wain]
  |-  ^+  out
  =+  |-  ^-  j=@
      ?:  ?|  =(i len)
              =(10 (cut(b i)))
          ==
        i
      $(i +(i))
    =.  out  :_  out
    (cut(b i, c (sub j i)))
  ?:  =(j len)
    (flop out)
  $(i +(j))
::
++  parse-pile
  |=  [pax=path tex=tape]
  ^-  pile
  =/  [=hair res=(unit [=pile =nail])]
    %-  road  |.
    ((pile-rule pax) [1 1] tex)
  ?^  res  pile.u.res
  %-  mean
  =/  lyn  p.hair
  =/  col  q.hair
  ^-  (list tank)
  :~  leaf+"syntax error at [{<lyn>} {<col>}] in {<pax>}"
    ::
      =/  =wain  (to-wain (crip tex))
      ?:  (gth lyn (lent wain))
        '<<end of file>>'
      (snag (dec lyn) wain)
    ::
      leaf+(runt [(dec col) '-'] "^")
  ==
::
++  pile-rule
  |=  pax=path
  %-  full
  %+  ifix
    :_  gay
    ::  parse optional /? and ignore
    ::
    ;~(plug gay (punt ;~(plug fas wut gap dem gap)))
  |^
  ;~  plug
    %+  cook  (bake zing (list (list taut)))
    %+  rune  hep
    (most ;~(plug com gaw) taut-rule)
  ::
    %+  cook  (bake zing (list (list taut)))
    %+  rune  lus
    (most ;~(plug com gaw) taut-rule)
  ::
    %+  rune  tis
    ;~(plug ;~(pose (cold ~ tar) (stag ~ sym)) ;~(pfix gap stap))
  ::
    %+  rune  tar
    ;~  (glue gap)
      sym
      ;~(pfix cen sym)
      ;~(pfix stap)
    ==
  ::
    %+  cook  (bake zing (list (list taut)))
    %+  rune  hax
    (most ;~(plug com gaw) taut-rule)
  ::
    %+  stag  %tssg
    (most gap tall:(vang [& |] pax))
  ==
  ::
  ++  pant
    |*  fel=rule
    ;~(pose fel (easy ~))
  ::
  ++  mast
    |*  [bus=rule fel=rule]
    ;~(sfix (more bus fel) bus)
  ::
  ++  rune
    |*  [bus=rule fel=rule]
    %-  pant
    %+  mast  gap
    ;~(pfix fas bus gap fel)
  --
::
++  taut-rule
  %+  cook  |=(taut +<)
  ;~  pose
    (stag ~ ;~(pfix tar sym))               ::  *foo -> [~ %foo]
    ;~(plug (stag ~ sym) ;~(pfix tis sym))  ::  bar=foo -> [[~ %bar] %foo]
    (cook |=(a=term [`a a]) sym)            ::  foo    -> [[~ %foo] %foo]
  ==
::
++  segments
  |=  suffix=@tas
  ^-  (list path)
  =/  parser
    (most hep (cook crip ;~(plug ;~(pose low nud) (star ;~(pose low nud)))))
  =/  torn=(list @tas)  (fall (rush suffix parser) ~[suffix])
  %-  flop
  |-  ^-  (list (list @tas))
  ?<  ?=(~ torn)
  ?:  ?=([@ ~] torn)
    ~[torn]
  %-  zing
  %+  turn  $(torn t.torn)
  |=  s=(list @tas)
  ^-  (list (list @tas))
  ?>  ?=(^ s)
  ~[[i.torn s] [(crip "{(trip i.torn)}-{(trip i.s)}") t.s]]
::
++  get-fit
  |=  [pre=@ta pax=@tas dir=(map path cord)]
  ^-  (unit path)
  =/  paz=(list path)  (segments pax)
  |-
  ?~  paz
    ~&  >>  "choo: missing dependency {<pax>}"
    ~
  =/  last=term  (rear i.paz)
  =.  i.paz   `path`(snip i.paz)
  =/  puz
    ^-  path
    %+  snoc
      `path`[pre i.paz]
    `@ta`(rap 3 ~[last %'.' %hoon])
  ?^  (~(get by dir) puz)
    `puz
  $(paz t.paz)
::
++  resolve-pile
  ::  turn fits into resolved path suffixes
  |=  [=pile dir=(map path cord)]
  ^-  (list raut)
  ;:  weld
    (turn sur.pile |=(taut ^-(raut [face (need (get-fit %sur pax dir))])))
    (turn lib.pile |=(taut ^-(raut [face (need (get-fit %lib pax dir))])))
  ::
    %+  turn  raw.pile
    |=  [face=(unit term) pax=path]
    =/  pax-snip  (snip pax)
    =/  pax-rear  (rear pax)
    ^-  raut
    =/  pat=path  (snoc pax-snip `@ta`(rap 3 ~[pax-rear %'.' %hoon]))
    ?.  (~(has by dir) pat)
      ~&  "hoonc: missing dependency {<pat>}"
      !!
    [face pat]
  ::
    %+  turn  bar.pile
    |=  [face=term mark=@tas pax=path]
    =/  pax-snip  (snip pax)
    =/  pax-hind  (rear pax-snip)
    =/  pax-rear  (rear pax)
    ^-  raut
    =/  pat=path  (snoc (snip pax-snip) `@ta`(rap 3 ~[pax-hind %'.' pax-rear]))
    ?.  (~(has by dir) pat)
      ~&  "hoonc: missing dependency {<pat>}"
      !!
    [`face pat]
  ::
    (turn hax.pile |=(taut ^-(raut [face (need (get-fit %dat pax dir))])))
  ==
--
::
::  builder core
::
~%  %builder  +>  ~
|_  [honc=(trap vase) bc=build-cache pc=parse-cache]
::
++  build-honc
  |=  hoon-txt=cord
  ^-  (trap vase)
  ~&  "Please be patient, compiling hoon for the first time takes a while."
  (swet empty-trap-vase (ream hoon-txt))
::
::
::  $create: build a trap from a hoon/jock file with dependencies
::
::    .tar: the path to build
::    .dir: the directory to get dependencies from
::    .arb: arbitrary flag
::    .dyn: dynock flag
::
::    If arb is true, we are building a noun of arbitrary shape.
::
::    If dyn is true, we emit minimal dynock [type (trap nock)].
::
::    If both arb and dyn are true, emit typed dynock [inferred-type (trap nock)].
::
::    If arb is false,we are building a kernel gate that takes a hash
::    of the dependency directory.
::
::    returns a noun, a build-cache, and a parse-cache
++  create
  ~/  %create
  |=  [tar=path dir=(map path cord) arb=? dyn=?]
  ^-  [(unit *) build-cache parse-cache]
  =/  dir-hash  `@uvI`(mug dir)
  ~&  >>  dir-hash+dir-hash
  =/  typed=?  ?:(dyn arb |)
  =/  [tase=(unit (trap vase)) =build-cache =parse-cache]
    (create-target tar dir dyn typed)
  :_  [build-cache parse-cache]
  ::  build failure, just return the bunted trap
  ?~  tase
    ~
  %-  some
  ::
  ::  If dynock mode, return the dynock noun directly.
  ?:  dyn
    =>  [swetted=u.tase vase=vase]
    =/  dyn-vase=vase  $:swetted
    dyn-vase
  ::
  ::  If arbitrary, return the trap.
  ?:  arb
   =>  u.tase
   |.(+:^$)
  ::
  ::  Otherwise, defer slam the dir hash into the kernel gate
  ::  +shot calls the kernel gate to tell it the hash of the dependency directory
  =>  %+  shot  u.tase
    =>  d=!>(dir-hash)
    |.(d)
  |.(+:^$)
::
::  $create-target: builds a hoon/jock file with dependencies
::
::    .path: the path to build
::    .dir: the directory to get dependencies from
::    .dyn: dynock flag
::    .typed: typed dynock flag
::
::    returns a noun with the compiled hoon/jock file and the updated caches
++  create-target
  ~/  %create-target
  |=  [tar=path dir=(map path cord) dyn=? typed=?]
  ^-  [(unit (trap vase)) build-cache parse-cache]
  =^  all-nodes=(map path node)  pc
    (parse-dir dir)
  =/  dag=merk-dag  (build-merk-dag all-nodes)
  ::
  ::  delete invalid cache entries in bc
  =.  bc
    %+  roll
      ~(tap by bc)
    |=  [[=merk-hash *] bc=_bc]
    ?:  (~(has by merk.dag) merk-hash)
      bc
    (~(del by bc) merk-hash)
  ::
  =^  res=(unit (trap vase))  bc
    ?:  dyn
      (~(dynock bil [all-nodes dag]) [tar typed])
    (~(try bil [all-nodes dag]) tar)
  ::
  [res bc pc]
::
::
::  $parse-cache-find: locate entry by hash and file contents
::
::    .file-hash: hash of file contents
::    .fil: file contents
::    .pc: parse-cache
::
::    returns a cache entry if contents match
++  parse-cache-find
  |=  [file-hash=@ fil=cord pc=parse-cache]
  ^-  (unit [path pile (list raut)])
  =/  entries=(list parse-cache-entry)
    ?~  e=(~(get by pc) file-hash)
      ~
    u.e
  |-
  ?~  entries
    ~
  =/  entry=parse-cache-entry  i.entries
  ?:  =(fil fil.entry)
    `[path.entry pil.entry deps.entry]
  $(entries t.entries)
::  $parse-cache-upsert: replace or append an entry in a bucket
++  parse-cache-upsert
  |=  [pat=path fil=cord pil=pile deps=(list raut) entries=(list parse-cache-entry)]
  ^-  (list parse-cache-entry)
  ?~  entries
    [[pat fil pil deps] ~]
  =/  entry=parse-cache-entry  i.entries
  ?:  =(fil fil.entry)
    [[pat fil pil deps] t.entries]
  [entry $(entries t.entries)]
::  $parse-cache-put: insert a cache entry using hash bucket + content equality
++  parse-cache-put
  |=  [file-hash=@ pat=path fil=cord pil=pile deps=(list raut) pc=parse-cache]
  ^-  parse-cache
  =/  entries=(list parse-cache-entry)
    ?~  e=(~(get by pc) file-hash)
      ~
    u.e
  =/  entries  (parse-cache-upsert pat fil pil deps entries)
  (~(put by pc) file-hash entries)
::  $parse-dir: create nodes from dir
::
::    .dir: directory of deps, includes build target
::
::    returns a map of nodes and the updated parse cache
++  parse-dir
  |^
  |=  dir=(map path cord)
  ^-  [(map path node) parse-cache]
  =|  nodes=(map path node)
  =|  new-pc=parse-cache
  =/  files=(list path)  ~(tap in ~(key by dir))
  |-
  ?~  files
    [nodes new-pc]
  =^  nod=node  new-pc
    (make-node i.files dir new-pc)
  %=    $
    nodes  (~(put by nodes) i.files nod)
    new-pc     new-pc
    files  t.files
  ==
::
  ++  make-node
    |=  [pat=path dir=(map path cord) new-pc=parse-cache]
    ^-  [node parse-cache]
    =/  fil=cord  (~(got by dir) pat)
    =/  file-hash  (shax fil)                                  ::  hash dep file
    ?.  (is-hoon pat)
      :_  new-pc
      :*  pat                                           ::  path
          file-hash                                     ::  hash
          ~                                             ::  deps
          [%octs [(met 3 fil) fil]]                     ::  octs
          %.n                                           ::  no kick
      ==
    =/  tex=tape  (trip fil)
    =/  cached=(unit [path pile (list raut)])  (parse-cache-find file-hash fil pc)
    =/  [pil=pile deps=(list raut)]
      ?~  cached
        ~&  "parsing {<pat>}"
        (process-pile pat tex dir)
      ~&  "reusing parse cache entry for {<pat>}"
      +.u.cached
    :_  (parse-cache-put file-hash pat fil pil deps new-pc)
    :*  pat                                              ::  path
        file-hash                                        ::  hash
        deps                                             ::  deps
        [%hoon hoon.pil]                                 ::  hoon
        (is-dat pat)                                     ::  whether to eval or not
    ==
  ::
  ++  process-pile
    |=  [pax=path tex=tape dir=(map path cord)]
    ^-  [pile (list raut)]
    =/  pil  (parse-pile pax tex)
    [pil (resolve-pile pil dir)]
  --
::
::  +strip-dbug: remove %dbug nodes from hoon ASTs
++  strip-dbug
  |=  gen=*
  ^-  *
  ?@  gen  gen
  =/  head  -.gen
  =/  tail  +.gen
  ?:  ?=(%dbug head)
    ?@  tail  gen
    $(gen +.tail)
  :*  $(gen head)
      $(gen tail)
  ==
::
::  $build-merk-dag: builds a merkle DAG out dependencies + target
::
::    .nodes: the nodes of the dependency graph
::
::    returns a merkle DAG and a path-dag
++  build-merk-dag
  |=  nodes=(map path node)
  |^
  ^-  merk-dag
  ::
  =|  dag=merk-dag
  =/  graph  (build-graph-view nodes)
  =/  next=(map path node)  (update-next nodes graph)
  ::
  ::  traverse via a topological sorting of the DAG using Kahn's algorithm
  |-
  ?:  .=(~ next)
    ?.  .=(~ graph)
      ~|(cycle-detected+~(key by graph) !!)
    dag
  =-
    %=  $
      next   (update-next nodes graph)
      graph  graph
      dag  dag
    ==
  ^-  [graph=(map path (set path)) dag=merk-dag]
  ::
  ::  every node in next is put into file-dag and dep-dag along with
  ::  its hash
  %+  roll
    ~(tap by next)
  |=  [[p=path n=node] graph=_graph dag=_dag]
  =/  =merk-hash  (calculate-hash n dag)
  :+  (update-graph-view graph p)
    (~(put by merk.dag) merk-hash n)
  (~(put by file.dag) hash.n merk-hash)
  ::
  ::  $calculate-hash: calculate the hash of a node
  ::
  ::    .n: the node to calculate the hash of
  ::    .dag: the DAG of the dependency graph
  ::
  ::    returns the hash of the node
  ++  calculate-hash
    |=  [nod=node dag=merk-dag]
    ^-  @
    %+  roll
      deps.nod
    |=  [raut running-hash=_hash.nod]
    =/  dep-file-hash  hash:(~(got by nodes) pax)
    ?~  dep-merk-hash=(~(get by file.dag) dep-file-hash)
      ~&  "hoonc: calculate-hash: Missing entry for {<pax>}"
      !!
    (shax (rep 8 ~[running-hash u.dep-merk-hash]))
  ::
  ::  $build-graph-view: build a graph-view from a node map
  ::
  ::    .nodes: the nodes of the dependency graph
  ::
  ::    returns a graph-view of the dependency graph
  ::
  ++  build-graph-view
    |=  nodes=(map path node)
    ^-  graph-view
    %-  ~(urn by nodes)
    |=  [* n=node]
    %-  silt
    (turn deps.n |=(raut pax))
  ::
  ::  $update-graph-view: updates a $graph-view by removing a $path
  ::
  ::    .gv: the graph-view to update
  ::    .p: the path to remove from the graph-view
  ::
  ::    deletes the $path from the $graph-view and removes it from all edge sets
  ::
  ++  update-graph-view
    |=  [gv=graph-view p=path]
    ^-  graph-view
    =.  gv  (~(del by gv) p)
    %-  ~(urn by gv)
    |=  [pax=path edges=(set path)]
    (~(del in edges) p)
  ::
  ::  $update-next: returns nodes from a $graph-view that have no outgoing edges
  ::
  ::    .nodes: the nodes of the dependency graph
  ::    .gv: the graph-view of the dependency graph
  ::
  ::    assumes that entries in $nodes that are not in the $graph-view have
  ::    already been visited.
  ::
  ++  update-next
    |=  [nodes=(map path node) gv=graph-view]
    ^-  (map path node)
    ::
    ::  if we don't have the path in gv, already visited
    %+  roll
      ~(tap by gv)
    |=  [[pax=path edges=(set path)] next=(map path node)]
    ::
    :: if a node has no out edges, add it to next
    ?.  =(*(set path) edges)
      next
    %+  ~(put by next)
      pax
    (~(got by nodes) pax)
  --
::
::
::  Builder core
++  bil
  ~%  %bil  +>  ~
  |_  $:  nodes=(map path node)
          dag=merk-dag
      ==
  ::
  ++  try
    ~/  %try
    |=  tar=path
    ^-  [(unit (trap vase)) build-cache]
    ~+
    =/  nod=node  (~(got by nodes) tar)
    ?^  target=(grab hash.nod)
      ~&  "reusing build cache entry for: {<tar>}"
      [target bc]
    ::
    ::  recursively build dependencies of node
    =^  dep-vaz=(unit (trap vase))  bc
      =+  [dep-vaz=empty-trap-vase deps=`(list raut)`deps.nod]
      |-
      ?~  deps
        [`dep-vaz bc]
      ::
      =^  vaz=(unit (trap vase))  bc
        (try pax.i.deps)
      ::
      ::  Exit trap early due to build failure
      ?~  vaz
        [~ bc]
      ::
      ::  append build dependecy to dep-vaz and put it in the build cache
      %=  $
        dep-vaz  (slat dep-vaz (label-vase u.vaz face.i.deps))
        bc       bc
        deps     t.deps
      ==
    ::
    ::  If one of the deps did not build succesfully, we need to
    ::  Propagate failure back up to caller.
    ::
    ?~  dep-vaz
      [~ bc]
    ?~  target-vaz=(compile nod u.dep-vaz)
      [~ bc]
    ::
    ::  get merk hash of dependency, this is the key to the build cache
    =/  =merk-hash  (~(got by file.dag) hash.nod)
    [target-vaz (~(put by bc) merk-hash u.target-vaz)]
  ::
  ++  dynock
    ~/  %dynock
    |=  [tar=path typed=?]
    ^-  [(unit (trap vase)) build-cache]
    ~+
    =/  nod=node  (~(got by nodes) tar)
    ::
    ::  recursively build dependencies in normal mode so import values are available
    =^  dep-vaz=(unit (trap vase))  bc
      =+  [dep-vaz=empty-trap-vase deps=`(list raut)`deps.nod]
      |-
      ?~  deps
        [`dep-vaz bc]
      =^  vaz=(unit (trap vase))  bc
        (try pax.i.deps)
      ?~  vaz
        [~ bc]
      %=  $
        dep-vaz  (slat dep-vaz (label-vase u.vaz face.i.deps))
        bc       bc
        deps     t.deps
      ==
    ?~  dep-vaz
      [~ bc]
    ?~  target-dynock=(compiledynock nod u.dep-vaz typed)
      [~ bc]
    [target-dynock bc]
  ::
  ++  compile
    ~/  %compile
    |=  [nod=node dep-vaz=(trap vase)]
    ^-  (unit (trap vase))
    =;  result=(each (trap vase) tang)
      ?-  -.result
        %&  `p.result
        %|  ((slog p.result) ~)
      ==
    %-  mule
    |.
    ?.  ?=(%hoon -.leaf.nod)
      =>  octs=!>(octs.leaf.nod)
      |.(octs)
    ~>  %bout
    ~&  "compiling {<path.nod>}"
    ::
    ::  Faces are resolved via depth-first search into the subject.
    ::  We append the honc (hoon.hoon) to the end of the vase
    ::  because imports have higher precedence when resolving faces.
    ::  To avoid shadowing issues with hoon.hoon, attach faces to your
    ::  imports or avoid shadowed names altogether.
    =/  swetted=(trap vase)  (swet (slat dep-vaz honc) hoon.leaf.nod)
    ?.  eval.nod
      swetted
    ~&  "node {<path.nod>} is eval, kicking"
    =>  [swetted=swetted vase=vase]
    =/  vaz=vase  $:swetted
    =>  vaz=vaz
    |.(vaz)
 ::
  ++  compiledynock
    ~/  %compiledynock
    |=  [nod=node dep-vaz=(trap vase) typed=?]
    ^-  (unit (trap vase))
    =;  result=(each (trap vase) tang)
      ?-  -.result
        %&  `p.result
        %|  ((slog p.result) ~)
      ==
    %-  mule
    |.
    ?.  ?=(%hoon -.leaf.nod)
      ~&  "dynock mode only supports %hoon nodes: {<path.nod>}"
      !!
    ~>  %bout
    ~&  "dynock compiling {<path.nod>}"
    =/  dyn=vase
      ?:  typed
        (swyt (slat dep-vaz honc) hoon.leaf.nod)
      (swyn (slat dep-vaz honc) hoon.leaf.nod)
    |.(dyn)
 ::
  ++  grab
    |=  =file-hash
    ^-  (unit (trap vase))
    =/  =merk-hash  (~(got by file.dag) file-hash)
    (~(get by bc) merk-hash)
  --
::
::  $label-vase: label a (trap vase) with a face
::
::    .vaz: the (trap vase) to label
::    .face: the face to label the (trap vase) with
::
::    returns a (trap vase) labeled with the given face
++  label-vase
  |=  [vaz=(trap vase) face=(unit @tas)]
  ^-  (trap vase)
  ?~  face  vaz
  =>  [vaz=vaz face=u.face]
  |.
  =/  vas  $:vaz
  [[%face face p.vas] q.vas]
::
::  $slat: merge two (trap vase)s
::
::    .hed: the first (trap vase)
::    .tal: the second (trap vase)
::
::    returns a merged (trap vase)
++  slat
  |=  [hed=(trap vase) tal=(trap vase)]
  ^-  (trap vase)
  =>  +<
  |.
  =+  [bed bal]=[$:hed $:tal]
  [[%cell p:bed p:bal] [q:bed q:bal]]
::  +shot: deferred slam
::
::    .gat: the gate to slam with the sample as a (trap vase)
::    .sam: the sample to slam with the gate
::
::    NOTE: this should never run inside of a trap. if it does, the builder
::    dependencies will leak into the result.
::
++  shot
  ~/  %shot
  |=  [gat=(trap vase) sam=(trap vase)]
  ^-  (trap vase)
  =/  [typ=type gen=hoon]
    :-  [%cell p:$:gat p:$:sam]
    [%cnsg [%$ ~] [%$ 2] [%$ 3] ~]
  =+  gun=(~(mint ut typ) %noun gen)
  =>  [typ=p.gun +<.$]
  |.
  [typ .*([q:$:gat q:$:sam] [%9 2 %10 [6 %0 3] %0 2])]
::
::  +swet: deferred +slap
::
::  NOTE: this is +swat but with a bug fixed that caused a space leak in
::  the resulting trap vases.
::
++  swet
  ~/  %swet
  |=  [tap=(trap vase) gen=hoon]
  ^-  (trap vase)
  =/  gun  (~(mint ut p:$:tap) %noun gen)
  =>  [gun=gun tap=tap]
  |.  ~+
  [p.gun .*(q:$:tap q.gun)]
::
::  +swyn: mint-only dynock output
::
::    returns [type (trap nock)] without evaluating q.gun in the build subject.
++  swyn
  ~/  %swyn
  |=  [tap=(trap vase) gen=hoon]
  ^-  vase
  =/  gun  (~(mint ut p:$:tap) %noun gen)
  [%noun [[1 q.gun] 0]]
::
::  +swyt: mint-only typed dynock output
::
::    returns [inferred-type (trap nock)] without evaluating q.gun in the build subject.
++  swyt
  ~/  %swyt
  |=  [tap=(trap vase) gen=hoon]
  ^-  vase
  =/  gun  (~(mint ut p:$:tap) %noun gen)
  [p.gun [[1 q.gun] 0]]
::
++  is-hoon
  |=  pax=path
  ^-  ?
  =/  end  (rear pax)
  !=(~ (find ".hoon" (trip end)))
::
::
++  is-dat
  |=  pax=path
  ^-  ?
  =('dat' (head pax))
--
