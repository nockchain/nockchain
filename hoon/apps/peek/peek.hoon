::  nockchain peek nockapp
::
/=  t  /common/tx-engine
/=  *  /common/wrapper
/=  *  /common/zoon
::
=>
|%
+$  kernel-state
  $:  peek-command=peek-command
      peek-data=(unit *)
  ==
::
+$  peek-command
  $%  [%heavy ~]
      [%block block-id=@t]
      [%blocks ~]
      [%heaviest-block ~]
      [%heavy-n page-number=@ud]
      [%chknote block-id=@t]
  ==
::
++  moat  (keep kernel-state)
::
+$  cause
  $%  other-cause
      grpc-bind-cause
  ==
+$  other-cause
  $%  [%born command=peek-command]
  ==
+$  grpc-bind-cause
  $%  [%grpc-bind result=(unit (unit *))]
  ==
::
+$  effect
  $%  [%exit code=@]
      [%grpc grpc-effect]
      [%markdown @t]
      [%file file-effect]
  ==
::
+$  grpc-effect
  $%  [%peek pid=@ typ=@tas =path]
  ==
+$  file-effect
  $%  [%write path=@t data=@]
  ==
--
::
=<
%-  (moat |)
^-  fort:moat
|_  k=kernel-state
+*  util  +>
::  +load: upgrade from previous state
++  load
  |=  arg=kernel-state
  ^-  kernel-state
  arg
::
::  +peek: external inspect
++  peek
  |=  arg=path
  ^-  (unit (unit *))
  ?+  arg  ~
      [%peek-data ~]
    ``peek-data.k
      [%peek-command ~]
    ``peek-command.k
  ==
::
::  +poke: external apply
++  poke
  |=  [=wire eny=@ our=@ux now=@da dat=*]
  ^-  [(list effect) kernel-state]
  ~&  >  "{<now>}: poked on wire: {<wire>}"
  =/  soft-cau  ((soft cause) dat)
  ?~  soft-cau
    ~&  >>>  "could not mold poke: {<dat>}"  !!
  =/  c=cause  u.soft-cau
  ?+    wire  ~|("unsupported wire: {<wire>}" !!)
    ::
      [%poke %grpc ver=@ pid=@ tag=@tas ~]
    ~&  "in %grpc-bind"
    ?>  ?=(%grpc-bind -.c)
    ?~  result.c
      ~&  >>  "bad peek response"
      :_  k
      [%exit 1]~
    ?~  u.result.c
      =.  peek-data.k  ~
      :_  k
      :~  :-  %markdown
          (crip "no data found for /{(trip (command-description peek-command.k))}")
          [%exit 0]
      ==
    =/  data=*  u.u.result.c
    =.  peek-data.k  `data
    =/  [effects=(list effect) markdown-content=@t]
      =-  [`(list effect)`-< (crip ->)]
      ?-    -.peek-command.k
        ::
          %heavy
        :-  ~
        ~|  "error: could not parse heaviest block data"
        =/  heaviest=(unit (unit block-id:t))
          %-  (soft (unit block-id:t))
          data
        ?~  heaviest
          "error: could not parse heaviest block data"
        ?~  u.heaviest
          "empty: no heaviest block data"
        (format-heavy:util u.heaviest)
        ::
          %block
        :-  ~
        ~|  "block {(trip block-id.peek-command.k)} not found"
        =/  page=(unit page:t)
          %-  (soft page:t)
          data
        ?~  page
          "error: could not parse block data"
        (format-page:util (cat 3 'Block ' block-id.peek-command.k) u.page)
        ::
          %heaviest-block
        :-  ~
        ~|  "heaviest block not found"
        =/  page=(unit page:t)
          %-  (soft page:t)
          data
        ?~  page
          "error: could not parse heaviest block page data"
        (format-page:util 'Heaviest Block' u.page)
        ::
          %heavy-n
        :-  ~
        ~|  "no page found at height {<page-number.peek-command.k>}"
        =/  page=(unit page:t)
          %-  (soft page:t)
          data
        ?~  page
          "error: could not parse page data at height {<page-number.peek-command.k>}"
        (format-page:util (cat 3 'Page at height ' (scot %ud page-number.peek-command.k)) u.page)
        ::
          %blocks
        ~|  "error: could not parse blocks data"
        =/  blocks=(unit (z-map block-id:t page:t))
          %-  (soft (z-map block-id:t page:t))
          data
        ?~  blocks
          :-  ~
          "error: could not parse blocks data"
        ::  jam the blocks and save to file
        =/  jammed-blocks=@  (jam u.blocks)
        :-  [%file %write 'blocks.jam' jammed-blocks]~
        (format-blocks:util u.blocks)
        ::
          %chknote
        :-  ~
        ~|  "error: could not parse block transactions"
        =/  txs=(unit (z-map tx-id:t tx:t))
          %-  (soft (z-map tx-id:t tx:t))
          data
        ?~  txs
          "error: could not parse block transactions data"
        ::  build unified input set
        =/  all-inputs=(z-set nname:t)
          %-  ~(rep z-by u.txs)
          |=  [[tid=tx-id:t tx=tx:t] acc=(z-set nname:t)]
          (~(uni z-in acc) (extract-input-nnames:util tx))
        ::  build unified output set
        =/  all-outputs=(z-set nname:t)
          %-  ~(rep z-by u.txs)
          |=  [[tid=tx-id:t tx=tx:t] acc=(z-set nname:t)]
          (~(uni z-in acc) (extract-output-nnames:util tx))
        ::  compute intersection
        =/  intersection=(z-set nname:t)
          (~(int z-in all-inputs) all-outputs)
        ::  find problematic transactions
        =/  problematic-txs=(list [tx-id:t tx:t])
          (find-problematic-txs:util u.txs intersection)
        ::  format output
        %+  format-check-notes:util
          block-id.peek-command.k
        [u.txs all-inputs all-outputs intersection problematic-txs]
      ==
    :_  k
    %+  weld  effects
    ^-  (list effect)
    :~  [%markdown markdown-content]
        [%exit 0]
    ==
    ::
      [%poke src=?(%one-punch) ver=@ *]
    ?>  ?=(other-cause c)
    ?-    -.c
        %born
      ~&  "%born: attempting to peek {<(command-description command.c)>}"
      ~&  "peek-command path: {<;;(path (command-to-path:util command.c))>}"
      =.  peek-command.k  command.c
      ~&  peek-command.k
      :_  k
      ^-  (list effect)
      [%grpc %peek 0 %peek (command-to-path:util command.c)]~
    ==
  ==
--
|%
::
++  format-blocks
  |=  blocks=(z-map block-id:t page:t)
  ^-  tape
  =/  block-count=@
    ~(wyt z-by blocks)
  """
  # Blocks
  Total blocks: {<block-count>}
  """
::
++  format-heavy
  |=  heaviest=(unit block-id:t)
  ^-  tape
  ?~  heaviest
    "no heaviest block found"
  """
  # Heaviest Block
  - id: {(trip (to-b58:hash:t u.heaviest))}
  """
::
++  format-page
  |=  [title=@t page=page:t]
  ^-  tape
  =/  pow=(unit proof:t)  ~(pow get:page:t page)
  =/  proof-version=(unit @ud)
    ?~  pow  ~
    `-.u.pow
  =/  height=@ud  ~(height get:page:t page)
  =/  digest=block-id:t  ~(digest get:page:t page)
  =/  parent=block-id:t  ~(parent get:page:t page)
  =/  timestamp=@  ~(timestamp get:page:t page)
  =/  msg=page-msg:t  ~(msg get:page:t page)
  =/  epoch-counter=@ud  ~(epoch-counter get:page:t page)
  =/  target=bignum:bn:t  ~(target get:page:t page)
  =/  tx-ids=(z-set tx-id:t)  ~(tx-ids get:page:t page)
  =/  tx-ids-tape=tape
    ;;  tape
    %+  join  ' '
    ^-  (list @t)
    %+  turn  ~(tap z-in tx-ids)
    |=  =tx-id:t
    (to-b58:hash:t tx-id)
  """
  # {(trip title)}
  ## Page Data
  - height: {<height>}
  - digest: {<(trip (to-b58:hash:t digest))>}
  - parent: {<(trip (to-b58:hash:t parent))>}
  - timestamp: {<timestamp>}
  - msg: {<msg>}
  - epoch-counter: {<epoch-counter>}
  - target: {<target>}
  - proof-version: {<proof-version>}
  - tx-ids: {tx-ids-tape}
  """
::
++  command-to-path
  |=  command=peek-command
  ^-  (list @)
  ?-  -.command
    %heavy  /heavy
    %block  /block/[block-id.command]
    %heaviest-block  /heaviest-block
    %heavy-n  [%heavy-n page-number.command ~]
    %blocks  /blocks
    %chknote  /block-transactions/[block-id.command]
  ==
::
++  command-description
  |=  command=peek-command
  ^-  @t
  ?-  -.command
    %heavy  'heavy'
    %block  (cat 3 'block/' block-id.command)
    %heaviest-block  'heaviest-block'
    %heavy-n  (cat 3 'heavy-n/' (scot %ud page-number.command))
    %blocks  'blocks'
    %chknote  (cat 3 'check-notes/' block-id.command)
  ==
::
++  extract-input-nnames
  |=  tx=tx:t
  ^-  (z-set nname:t)
  ?-  -.tx
    %0  ~(key z-by inputs.raw-tx.tx)
    %1  ~|("v1 transactions not yet supported" !!)
  ==
::
++  extract-output-nnames
  |=  tx=tx:t
  ^-  (z-set nname:t)
  ?-  -.tx
    %0  =/  output-list=(list output:v0:t)  ~(val z-by outputs.tx)
        =/  nname-list=(list nname:t)
          %+  turn  output-list
          |=  out=output:v0:t
          name.note.out
        (~(gas z-in `(z-set nname:t)`~) nname-list)
    %1  ~|("v1 transactions not yet supported" !!)
  ==
::
++  find-problematic-txs
  |=  [txs=(z-map tx-id:t tx:t) intersection=(z-set nname:t)]
  ^-  (list [tx-id:t tx:t])
  =/  tx-list=(list [tx-id:t tx:t])  ~(tap z-by txs)
  %+  skim  tx-list
  |=  [tid=tx-id:t tx=tx:t]
  =/  inputs=(z-set nname:t)  (extract-input-nnames tx)
  =/  outputs=(z-set nname:t)  (extract-output-nnames tx)
  =/  all-notes=(z-set nname:t)  (~(uni z-in inputs) outputs)
  =/  has-problematic=?
    %-  ~(any z-in intersection)
    |=(n=nname:t (~(has z-in all-notes) n))
  has-problematic
::
++  format-check-notes
  |=  $:  block-id=@t
          all-txs=(z-map tx-id:t tx:t)
          all-inputs=(z-set nname:t)
          all-outputs=(z-set nname:t)
          intersection=(z-set nname:t)
          problematic-txs=(list [tx-id:t tx:t])
      ==
  ^-  tape
  =/  intersection-count=@ud  ~(wyt z-in intersection)
  =/  input-count=@ud  ~(wyt z-in all-inputs)
  =/  output-count=@ud  ~(wyt z-in all-outputs)
  =/  tx-count=@ud  ~(wyt z-by all-txs)
  ::  format all transactions
  =/  all-tx-details=tape
    =/  tx-list=(list [tx-id:t tx:t])  ~(tap z-by all-txs)
    %+  roll  tx-list
    |=  [[tid=tx-id:t tx=tx:t] acc=tape]
    =/  tx-id-str=tape  (trip (to-b58:hash:t tid))
    =/  [in-count=@ud out-count=@ud]
      ?-  -.tx
        %0  :-  ~(wyt z-by inputs.raw-tx.tx)
            ~(wyt z-by outputs.tx)
        %1  [0 0]
      ==
    =/  tx-section=tape
      """
      ## Transaction {tx-id-str}
      - Inputs: {(a-co:co in-count)}
      - Outputs: {(a-co:co out-count)}

      """
    (welp acc tx-section)
  ::  format all input notes
  =/  input-notes-formatted=tape
    =/  input-list=(list nname:t)  ~(tap z-in all-inputs)
    %+  roll  input-list
    |=  [n=nname:t acc=tape]
    =/  note-hash=tape   <(to-b58:nname:t n)>
    =/  note-line=tape
      """
        - {note-hash}

      """
    (welp acc note-line)
  ::  format all output notes
  =/  output-notes-formatted=tape
    =/  output-list=(list nname:t)  ~(tap z-in all-outputs)
    %+  roll  output-list
    |=  [n=nname:t acc=tape]
    =/  note-hash=tape   <(to-b58:nname:t n)>
    =/  note-line=tape
      """
        - {note-hash}

      """
    (welp acc note-line)
  ::  format intersection if any
  =/  intersection-section=tape
    ?:  =(0 intersection-count)
      """
      ## Intersection Analysis
      No intersection found between input and output notes.

      """
    =/  intersection-list=(list nname:t)  ~(tap z-in intersection)
    =/  intersection-formatted=tape
      %+  roll  intersection-list
      |=  [n=nname:t acc=tape]
      =/  note-hash=tape   <(to-b58:nname:t n)>
      =/  note-line=tape
        """
          - {note-hash}

        """
      (welp acc note-line)
    =/  problematic-details=tape
      %+  roll  problematic-txs
      |=  [[tid=tx-id:t tx=tx:t] acc=tape]
      =/  tx-id-str=tape  (trip (to-b58:hash:t tid))
      (welp acc (welp "  - " (welp tx-id-str "\0a")))
    """
    ## Intersection Analysis
    WARNING: Found {(a-co:co intersection-count)} note(s) in both inputs and outputs!

    ### Intersecting Notes:
    {intersection-formatted}
    ### Transactions with Intersecting Notes:
    {problematic-details}
    """
  =/  block-id-str=tape  (trip block-id)
  """
  # Check Notes

  ## Summary
  - Total Transactions: {(a-co:co tx-count)}
  - Total Input Notes: {(a-co:co input-count)}
  - Total Output Notes: {(a-co:co output-count)}

  ---

  {intersection-section}

  ---

  ## All Transactions

  {all-tx-details}
  ## All Input Notes ({(a-co:co input-count)})
  {input-notes-formatted}
  ## All Output Notes ({(a-co:co output-count)})
  {output-notes-formatted}

  ---

  https://nockblocks.com/block/{block-id-str}


  """
--
