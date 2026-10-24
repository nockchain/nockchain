/-  *sock
|%
+|  %ska
+$  barn  [sub=sock for=*]
+$  nomm  :: SKA-analyzed nock
  $~  [%one **]
  $%  [%par nomm nomm] 
      [%zer @ ?]  ::  safety-tagged lookup
      [%one *]
      [%two nomm nomm sock (unit *) ?]  ::  subject knowledge and known formula, safety-tag on metaformula
      [%thr nomm]
      [%fou nomm ?]  :: safety-tagged increment
      [%fiv nomm nomm]
      [%six nomm nomm nomm]
      [%sev nomm nomm]
      :: we omit 8, translating it to 7 + autocons
      :: we omit 9, translating it to 7 + 2
      [%ten [@ nomm] nomm ?]  ::  safety-tagged edit
      [%els @ nomm]
      [%eld [@ nomm] nomm ?]  :: safety-tagged hint formula
      [%twe nomm nomm]
  ==
+$  farm  [yard=(map barn [does=nomm says=boot]) wood=(list barn)]
+|  %lin
+$  berm  [sub=sock for=* ax=@ gen=@tas] :: local label
+$  plow :: noun<->ssa map
  $%  [%fork left=plow rite=plow safe=?] :: cons of two mappings
      [%tine @]                          :: use this SSA value at this axis
      [%disc ~]                          :: no uses here or below
  ==
+$  line  :: destination
  $%  [%moat wher=berm what=plow]  :: place result in SSA values specified by what, go wher
      [%rift troo=berm fals=berm]  :: branch on result
      [%pond ~]                    :: tail position, return result in a register
  ==
+$  bran :: instructions in a block
  $%  [%imm * @]                  :: Write a noun to an SSA value
      [%mov @ @]                  :: Copy an SSA value
      [%inc @ @]                  :: Define second SSA register as increment of first
      [%unc @ @]                  :: Define a second SSA register as increment of first, without checking atomicity
      [%con @ @ @]                :: Construct a cell, first SSA head, second SSA tail, third SSA result
      [%hed @ @]                  :: Take the head of first SSA and place in second.
                                  ::  Crash if first SSA not a cell
      [%hud @ @]                  :: Take the head of the first SSA, known to be a cell
      [%tal @ @]                  :: Take tail head of first SSA and place in second.
                                  ::  Crash if first SSA not a cell
      [%tul @ @]                  :: Take the tail of the first SSA, known to be a cell
  ==
:: These instructions end a block.
:: A block ends either because we need to transfer control
:: elsewhere (hop), we need to branch (clq, eqq, brn), we need a saved
:: control point to return to (lnk, call, hnt, spy), or we are done and
:: transfering control to another arm (jmp, lnt), our caller (don), or
:: the crash handler (bom). 
::
:: The bec and eye instructions are intermediate forms only, and are
:: translated into cal and jmp respectively once enough information is
:: available about their targets. They exist because when linearizing
:: and registerizing (mutually) recursive arms, there will be some call
:: targets for which we do not know subject use maps and thus cannot yet
:: build calls to. Once all arms are registerized, we scan for bec and
:: eye and replace them with jmp and call with registers appropriately
:: split.
+$  germ :: instructions ending a block
  $%  [%clq @ berm berm]          :: Branch left if the SSA value is a cell, right otherwise
      [%eqq @ @ berm berm]        :: Branch left if SSA registers are equal, right otherwise
      [%brn @ berm berm]          :: Branch left if SSA register is 0, right if 1
      [%hop berm]                 :: Go to berm unconditionally (local direct jump)
      [%lnk @ @ @ berm]           :: Call formula in first SSA register with subject in second,
                                  ::   result in third, return to berm
      [%cal barn (list @) @ berm] :: Call arm given by barn, subject in first SSA register,
                                  ::   result in second, return to berm
      [%bec barn @ @ berm]        :: Not quite a call: we need to know the subject registerization of an arm.
                                  ::   see %eye
      [%lnt @ @]                  :: Jump to formula in first SSA register with subject in second
      [%jmp barn (list @)]        :: Jump to the code at the label in tail position,
                                  ::   with the subject in the SSA register
      [%eye barn @]               :: Look before you jump: we need to know the subject registerization of an arm
                                  ::   before we jump to it. Until then, here's a register with
                                  ::   the whole subject
      [%spy @ @ @ berm]           :: Scry with the ref/path pair in the first 2 SSA registers
                                  ::   define the third as the result
      [%hnt @ berm]               :: Treat the result in the SSA register as a hint and continue to the given label

      [%don @]                    :: Finish the procedure, returning the value in the SSA
      [%bom ~]                    :: Crash
  ==
+$  pool  (list [axe=@ ssa=@ saf=?])    :: entry point subject uses: ordered subject/ssa/safety
+$  lock  [body=(list bran) bend=germ]  :: basic block: instructions + a terminator or branch
+$  lake  (map berm lock)               :: code table of basic blocks
+$  rice  [goes=lake uses=pool lump=@]  :: entry information and code table for an arm
+$  sack  [does=rice says=boot]         :: code table entry: basic blocks + SKA result for an arm
+$  town  [land=(map barn sack) lamb=@] :: code table
--
