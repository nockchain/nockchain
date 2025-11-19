/=  ztd-two  /common/ztd/two
=>  ztd-two
~%  %misc-lib  ..lift  ~
::    misc-lib
|%
::  +flec: reflect a noun, i.e. switch head and tail
++  flec
  |*  *
  ?@  +<  +<
  +<+^+<-
::
++  lib-u32
  ~%  %lib-u32  +  ~
  ::    Unsigned 32-bit Arithmetic
  |%
  +$  u32     @udthirtytwo
  ++  bex32  ^~((bex 32))        :: 4.294.967.296
  ++  max32  ^~((dec (bex 32)))  :: 4.294.967.295
  ::
  ::  +is-u32: is atom a u32?
  ++  is-u32
    ~/  %is-u32
    |=  a=@
    ^-  ?
    (lth a bex32)
  ::
  ::  +belt-to-u32: decompose a belt to u32s
  ++  belt-to-u32s
    ~/  %btu32s
    |=  sam=belt
    ^-  [lo=u32 hi=u32]
    ?>  (lth sam p)  ::NOTE: in flib and jutes, this is bex64 instead of goldilocks prime?
    ::  ?>  (lth sam (bex 64))
    (flec (dvr sam bex32))
  ::
  ++  belt-from-u32s
    ~/  %bfu32s
    |=  [lo=u32 hi=u32]
    ^-  belt
    ?>  ?&((is-u32 lo) (is-u32 hi))
    (add lo (mul bex32 hi))
  ::
  ::  +u32-add: a + b = lo + (2^32)*car
  ++  u32-add
    |=  [a=u32 b=u32]
    ^-  [lo=u32 car=u32]
    ?>  ?&((is-u32 a) (is-u32 b))
    (flec (dvr (badd a b) (bex 32)))
  ::
  ::  +u32-sub: a - b = -(2^32)*bor + com
  ::
  ::    If a>b, then a-b=c is interpreable as an ordinary u32. But if a<b, you
  ::    can imagine we "borrow" 2^32 to add to `a` before we subtract so we can
  ::    represent the difference as an ordinary u32. Equivalently we're just
  ::    adding 2^32 to any negative answer, i.e. we're doing arithmetic mod 2^32.
  ++  u32-sub
    |=  [a=u32 b=u32]
    ::  com=complement (i.e. 2's-complement), bor=borrow
    ^-  [com=u32 bor=u32]
    ?>  ?&((is-u32 a) (is-u32 b))
    [(~(dif fo (bex 32)) a b) ?:((lth a b) 1 0)]
  ::
  ::  +u32-lth:  [a b] --> 0/1 according to a < b T/F
  ++  u32-lth
    ~/  %u32-lth
    |=  [a=u32 b=u32]
    ^-  ?
    ?>  ?&((is-u32 a) (is-u32 b))
    (lth a b)
  ::
  ::  +u32-mul: a*b = lo + (2^32)*hi
  ++  u32-mul
    ~/  %u32-mul
    |=  [a=u32 b=u32]
    ^-  [lo=u32 hi=u32]
    ?>  ?&((is-u32 a) (is-u32 b))
    (flec (dvr (bmul a b) bex32))
  ::
  ::  +u32-dvr: a = qot*b + rem, rem < b
  ++  u32-dvr
    ~/  %u32-dvr
    |=  [a=u32 b=u32]
    ^-  [qot=u32 rem=u32]
    ?>  ?&((is-u32 a) (is-u32 b))
    (dvr a b)
  ::
  ::  +u32-div:  a / b = c such that a - b*c < b
  ++  u32-div
    ~/  %u32-div
    |=  [a=u32 b=u32]
    ^-  u32
    ?>  ?&((is-u32 a) (is-u32 b))
    qot:(u32-dvr a b)
  ::
  ::  +u32-mod: a - b*(a / b)
  ++  u32-mod
    ~/  %u32-mod
    |=  [a=u32 b=u32]
    ^-  u32
    ?>  ?&((is-u32 a) (is-u32 b))
    rem:(u32-dvr a b)
  --
::
++  bignum  ::  /lib/bignum
  ~%  %bignum  +  ~
  |%
  ++  l32  lib-u32
  ++  u32  u32:l32
  ::  mirrors bignum from flib
  ::  32 bits = 2^5 bits => bloq size of 5
  +$  bignum
  ::  LSB order (based on result of rip)
  ::  empty array is 0
  [%bn p=(list u32)]
  ::
  ++  validate
    |=  bn=bignum
    (levy p.bn is-u32:l32)
  ::
  ::  +p: Goldilocks prime, written in bignum form
  ::
  ::    least significant bit first, so:
  ::    p = 2^64-2^32+1 = 2^32(2^32 - 1) + 1
  ++  p
    ^-  bignum
    [%bn ~[1 4.294.967.295]]
  ::
  ::  +p2: p^2
  ++  p2
    ^-  bignum
    [%bn ~[1 4.294.967.294 2 4.294.967.294]]
  ::
  ::  +p3: p^3
  ++  p3
    ^-  bignum
    [%bn ~[1 4.294.967.293 5 4.294.967.289 5 4.294.967.293]]
  ::
  ++  chunk
    ~/  %chunk
    |=  a=@
    ^-  bignum
    [%bn (rip-correct 5 a)]
  ::
  ++  merge
    ~/  %merge
    |=  b=bignum
    ^-  @
    ::  fock always turns unchunked bignums into chunked case
    (rep 5 p.b)
  ::
  ++  valid
    ::  are all elements of the list valid big int chunks, i.e., less than u32.max_val
    ~/  %valid
    |=  b=bignum
    ^-  ?
    (levy p.b |=(c=@ (lth c (bex 32))))
  --  ::bignum
::
++  shape  ::  /lib/shape
  ~%  %shape  ..shape  ~
  =,  mp-to-mega
  |%
  ::  +dyck: produce the Dyck word describing the shape of a tree
  ++  dyck
    ~/  %dyck
    |=  t=*
    %-  flop
    ^-  (list @)
    =|  vec=(list @)
    |-
    ?@  t  vec
    $(t +.t, vec [1 $(t -.t, vec [0 vec])])
  ::
  ::  +grow: grow the tree with given shape and leaves
  ++  grow
    ~/  %grow
    |=  [shape=(list @) leaves=(list @)]
    ^-  *
    ?>  ?&(=((lent shape) (mul 2 (dec (lent leaves)))) (valid-shape shape))
    ?~  shape
      ?>  ?=([@ ~] leaves)
        i.leaves
    =/  lr-shape  (left-right-shape shape)
    =/  split-idx  (shape-size -:lr-shape)
    =/  split-leaves  (split split-idx leaves)
    :-  (grow -:lr-shape -:split-leaves)
    (grow +:lr-shape +:split-leaves)
  ::
  ::  +shape-size: size of the tree in #leaves described by a given Dyck word
  ++  shape-size
    ~/  %shape-size
    |=  shape=(list @)
    ^-  @
    (add 1 (div (lent shape) 2))
  ::
  ++  leaf-sequence
    ~/  %leaf-sequence
    |=  t=*
    %-  flop
    ^-  (list @)
    =|  vec=(list @)
    |-
    ?@  t  t^vec
    $(t +.t, vec $(t -.t))
  ::
  ++  num-of-leaves
    ~/  %num-of-leaves
    |=  t=*
    ?@  t  1
    %+  add
      (num-of-leaves -:t)
    (num-of-leaves +:t)
  ::
  ::  +left-right-shape: extract left and right tree shapes from given tree shape
  ++  left-right-shape
    ~/  %left-right-shape
    |=  shape=(list @)
    ^-  [(list @) (list @)]
    ?>  (valid-shape shape)
    ?:  =((lent shape) 0)
      ~|  "Empty tree has no left subtree."
      !!
    =.  shape  (slag 1 shape)
    =/  stack-height  1
    =|  lefsh=(list @)
    |-
    ?:  =(stack-height 0)
      ?<  ?=(~ lefsh)
      [(flop t.lefsh) shape]
    ?<  ?=(~ shape)
    ?:  =(i.shape 0)
      $(lefsh [i.shape lefsh], shape t.shape, stack-height +(stack-height))
    $(lefsh [i.shape lefsh], shape t.shape, stack-height (dec stack-height))
  ::
  ++  axis-to-axes
    ~/  %axis-to-axes
    |=  axi=@
    ^-  (list @)
    =|  lis=(list @)
    |-
    ?:  =(1 axi)  lis
    =/  hav  (dvr axi 2)
    $(axi p.hav, lis [?:(=(q.hav 0) 2 3) lis])
  ::
  ::  +valid-shape: computes whether a given vector is a valid tree shape
  ++  valid-shape
    ~/  %valid-shape
    |=  shape=(list @)
    ^-  ?
    =/  stack-height  0
    |-
    ?:  ?=(~ shape)
      ?:  =(stack-height 0)
        %.y
      %.n
    ?>  ?|(=(i.shape 0) =(i.shape 1))
    ?:  =(i.shape 0)
      $(shape t.shape, stack-height +(stack-height))
    ?:  =(stack-height 0)
      %.n
    $(shape t.shape, stack-height (dec stack-height))
  ::
  ::  +split: split ~[a_1 ... a_n] into [~[a)1 ... a_{idx -1}] ~[a_{idx} ... a_n]]
  ++  split
    ~/  %split
    |=  [idx=@ lis=(list @)]
    ^-  [(list @) (list @)]
    ~|  "Index argument must be less than list length."
    ?>  (lth idx (lent lis))
    =|  lef=(list @)
    =/  i  0
    |-
    ?:  =(i idx)
      [(flop lef) lis]
    ?<  ?=(~ lis)
    $(lef [i.lis lef], lis t.lis, i +(i))
  ::
  ++  shape-axis-to-index
    ~/  %shape-axis-to-index
    |=  [tre=* axis=@]
    ^-  [dyck-index=@ leaf-index=@]
    =/  axes   (axis-to-axes axis)
    =/  shape  (dyck tre)
    =/  dyck-index  0
    =/  leaf-index  0
    |-
    ?~  axes
      [dyck-index leaf-index]
    =/  lr-shape  (left-right-shape shape)
    ?:  =(i.axes 2)
      $(axes t.axes, shape -.lr-shape)
    ?>  =(i.axes 3)
    %_  $
      axes        t.axes
      shape       +.lr-shape
      dyck-index  (add dyck-index (lent -.lr-shape))
      leaf-index  (add leaf-index (shape-size -.lr-shape))
    ==
  ::
  ::  +path-to-axis: binary directions to input axis
  ++  path-to-axis
    |=  axis=belt
    ^-  (list belt)
    (slag 1 (flop (rip 0 axis)))
  ::
  ::  +ion-eval: eval first arg as poly at alpha
  ::
  ::    First arg is a polynomial, read high powers to low from L to R.
  ::    In practice this poly is a dyck word or leaf vector.
  ++  ion-eval
    |=  [word-vec=(list belt) alpha=belt]
    ^-  belt
    %+  roll  word-vec
    |=  [coeff=_f0 acc=_f0]
    ^-  belt
    (badd (bmul alpha acc) coeff)
  ::
  ++  ion-eval-symbolic
    |=  [word-vec=(list mp-mega) alpha=mp-mega]
    ^-  mp-mega
    %+  roll  word-vec
    |=  [coeff=mp-mega acc=mp-mega]
    ^-  mp-mega
    (mpadd (mpmul alpha acc) coeff)
  --  ::shape
::
++  tip5  ::  lib/tip5
  ~%  %tip5-lib  ..tip5  ~
  |_  num-rounds=_7
  +|  %user-types
  +$  noun-digest           [belt belt belt belt belt]
  +$  ten-cell              [noun-digest noun-digest]
  ::
  ++  digest-dyck-word
    ^-  (list @)
    ~[0 1 0 1 0 1 0 1]
  ++  ten-cell-dyck-word
    ^~  ^-  (list @)
    (weld [0 digest-dyck-word] [1 digest-dyck-word])
  ::
  ::  a sponge-tuple is a 16-tuple of belts; relevant for hash5.hoon
  ++  sponge-tuple-dyck-word
    ^~  ^-  (list @)
    (zing (reap (dec state-size) ~[0 1]))
  ::
  +|  %user-funcs
  ::
  ::  +hash-ten-cell
  ++  hash-ten-cell
    ~/  %hash-ten-cell
    |=  =ten-cell
    ^-  noun-digest
    =-  ?>  ?=(noun-digest -)  -
    %-  list-to-tuple
    %-  hash-10
    %-  leaf-sequence:shape
    ten-cell
  ::
  ::  +hash-leaf
  ++  hash-leaf
    |=  leaf=belt
    ^-  noun-digest
    ::  ?>  (based leaf)  commented out because its performed in +hash-varlen
    (hash-belts-list ~[leaf])
  ::
  ::  $hashable: a DSL for hashing anything
  +$  hashable
    $~  [%leaf p=*]
    $^  [p=hashable q=hashable]
    $%  [%leaf p=*]
        [%hash p=noun-digest]
        [%list p=(list hashable)]
        [%mary p=mary]
    ==
  ::
  ::  +hash-hashable
  ++  hash-hashable
    ~/  %hash-hashable
    |=  h=hashable
    ^-  noun-digest
    ?:  ?=(%hash -.h)
      p.h
    ?:  ?=(%leaf -.h)
      (hash-noun-varlen p.h)
    ?:  ?=(%list -.h)
      (hash-noun-varlen (turn p.h hash-hashable))
    ?:  ?=(%mary -.h)
      %-  hash-hashable
      :-  leaf+step.p.h
      :-  leaf+len.array.p.h
      hash+(hash-belts-list (bpoly-to-list array:(~(change-step ave p.h) 1)))
    %-  hash-ten-cell
    [$(h p.h) $(h q.h)]
  ::
  ++  hashable-noun-digests
    |=  lis=(list noun-digest)
    ^-  hashable
    list+(turn lis |=(nd=noun-digest hash+nd))
  ::
  ++  hashable-bpoly
    |=  bp=bpoly
    ^-  hashable
    mary+`mary`[%1 bp]
  ::
  ++  hashable-felt
    |=  f=felt
    ^-  hashable
    (hashable-bpoly [3 f])
  ::
  ++  hashable-fpoly
    |=  fp=fpoly
    ^-  hashable
    mary+`mary`[%3 fp]
  ::
  ++  hashable-mary
    |=  =mary
    ^-  hashable
    mary+mary
  ::
  ::  +hash-noun-varlen
  ++  hash-noun-varlen
    ~/  %hash-noun-varlen
    |=  n=*
    ^-  noun-digest
    =/  leaf=(list @)  (leaf-sequence:shape n)
    =/  dyck=(list @)  (dyck:shape n)
    =/  size  (lent leaf)
    (hash-belts-list [size (weld leaf dyck)])
  ::
  ::  +hash-felt
  ++  hash-felt
    ~/  %hash-felt
    |=  =felt
    ^-  noun-digest:tip5
    =/  felt-tuple=[@ @ @ @ @]
      ;;  [@ @ @ @ @]
      %-  list-to-tuple
      (weld (felt-to-list felt) ~[0 0])
    (hash-ten-cell felt-tuple [0 0 0 0 0])
  ::
  ::
  ++  hash-belts-list
    ~/  %hash-belts-list
    |=  belts=(list belt)
    ^-  noun-digest:tip5
    =-  ?>  ?=(noun-digest -)  -
    %-  list-to-tuple
    (hash-varlen belts)
  ::
  ::  +hash-pairs
  ++  hash-pairs
    ~/  %hash-pairs
    |=  lis=(list (list @))
    ^-  (list (list @))
    |^
    %+  turn
      (indices (lent lis))
    |=  b=@
    ?:  =(+(b) (lent lis))
      (snag b lis)
    (hash-10:tip5 (weld (snag b lis) (snag +(b) lis)))
    ::
    ::  TODO: there is probably a more clean way to generate indices.
    ++  indices
      |=  n=@
      ^-  (list @)
      ?<  =(n 0)
      =/  i  0
      |-
      ?:  (gte i n)
        ~
      [i $(i (add 2 i))]
    --
  ::
  ::  +snag-as-digest
  ::
  ::  Retrieve the i-th entry of the mary return it as a tip5 hash digest.
  ::  Assumes that each entry of the mary is a single hash encoded in base 64.
  ::
  ++  snag-as-digest
    ~/  %snag-as-digest
    |=  [m=mary i=@]
    ^-  noun-digest:tip5
    ?>  =(5 step.m)
    =/  buf  (~(snag ave m) i)
    :*  (cut 6 [0 1] buf)
        (cut 6 [1 1] buf)
        (cut 6 [2 1] buf)
        (cut 6 [3 1] buf)
        (cut 6 [4 1] buf)
    ==
  ::
  ::  +list-to-digest
  ++  list-to-digest
    ~/  %list-to-digest
    |=  lis=(list @)
    ^-  noun-digest:tip5
    ?>  =(5 (lent lis))
    :*  (snag 0 lis)
        (snag 1 lis)
        (snag 2 lis)
        (snag 3 lis)
        (snag 4 lis)
    ==
  ::
  ::  +atom-to-digest
  ::
  ::  Converts hex buffer into base-p representation
  ++  atom-to-digest
    ~/  %atom-to-digest
    |=  buffer=@ux
    ^-  noun-digest:tip5
    =/  [q=@ a=@]  (dvr buffer p)
    =/  [q=@ b=@]  (dvr q p)
    =/  [q=@ c=@]  (dvr q p)
    =/  [e=@ d=@]  (dvr q p)
    [a b c d e]
  ::
  ::  +digest-to-atom
  ::
  ::  Returns a hexadecimal representation of the hash.
  ::  We treat the tip-5 hash as a base-p number.
  ++  digest-to-atom
    ~/  %digest-to-atom
    |=  [a=belt b=belt c=belt d=belt e=belt]
    ^-  @ux
    =/  p2  (mul p p)
    =/  p3  (mul p2 p)
    =/  p4  (mul p3 p)
    ;:  add
      a
      (mul p b)
      (mul p2 c)
      (mul p3 d)
      (mul p4 e)
    ==
  ::
  +|  %dev-types
  +$  digest                (list melt)  ::  length = digest-length
  +$  state                 (list melt)  ::  length = state-size
  +$  domain                ?(%variable %fixed)
  +$  tip5-state            (list melt)
  ::
  +|  %dev-constants
  ++  digest-length         5
  ++  state-size            16
  ++  num-split-and-lookup  4
  ++  capacity              6
  ++  rate                  10
  ++  max-tip5-atom  (digest-to-atom [(dec p) (dec p) (dec p) (dec p) (dec p)])
  ::
  ++  state-dyck-word
    ^~  ^-  (list @)
    (zing (reap state-size ~[0 1]))
  ::
  ::  +lookup-table: represents the map x -> (x+1)^3 - 1 (mod 257) on {0, ..., 255}
  ::
  ::    Used on the first 4 state elements in the S-box layer of each round of Tip5
  ++  lookup-table
    ^-  (list @)
    :~  0    7    26   63   124  215  85   254  214  228  45   185  140  173  33   240
        29   177  176  32   8    110  87   202  204  99   150  106  230  14   235  128
        213  239  212  138  23   130  208  6    44   71   93   116  146  189  251  81
        199  97   38   28   73   179  95   84   152  48   35   119  49   88   242  3
        148  169  72   120  62   161  166  83   175  191  137  19   100  129  112  55
        221  102  218  61   151  237  68   164  17   147  46   234  203  216  22   141
        65   57   123  12   244  54   219  231  96   77   180  154  5    253  133  165
        98   195  205  134  245  30   9    188  59   142  186  197  181  144  92   31
        224  163  111  74   58   69   113  196  67   246  225  10   121  50   60   157
        90   122  2    250  101  75   178  159  24   36   201  11   243  132  198  190
        114  233  39   52   21   209  108  238  91   187  18   104  194  37   153  34
        200  143  126  155  236  118  64   80   172  89   94   193  135  183  86   107
        252  13   167  206  136  220  207  103  171  160  76   182  227  217  158  56
        174  4    66   109  139  162  184  211  249  47   125  232  117  43   16   42
        127  20   241  25   149  105  156  51   53   168  145  247  223  79   78   226
        15   222  82   115  70   210  27   41   1    170  40   131  192  229  248  255
    ==
  ::
  ::  +round-constants: 5 length=16 vectors added to the state in the final layer each round
  ++  round-constants
    ::  notice melt and montify: these are in Montgomery representation
    ^-  (list melt)
    %-  turn  :_  montify
    ?>  ?|(=(num-rounds 5) =(num-rounds 7))
    ?:  =(num-rounds 5)
      ::  length = 5 * state-size = 80
      :~
      ::  1st round constants
        13.630.775.303.355.457.758  16.896.927.574.093.233.874
        10.379.449.653.650.130.495  1.965.408.364.413.093.495
        15.232.538.947.090.185.111  15.892.634.398.091.747.074
        3.989.134.140.024.871.768   2.851.411.912.127.730.865
        8.709.136.439.293.758.776   3.694.858.669.662.939.734
        12.692.440.244.315.327.141  10.722.316.166.358.076.749
        12.745.429.320.441.639.448  17.932.424.223.723.990.421
        7.558.102.534.867.937.463   15.551.047.435.855.531.404
      ::  2nd round constants
        17.532.528.648.579.384.106  5.216.785.850.422.679.555
        15.418.071.332.095.031.847  11.921.929.762.955.146.258
        9.738.718.993.677.019.874   3.464.580.399.432.997.147
        13.408.434.769.117.164.050  264.428.218.649.616.431
        4.436.247.869.008.081.381   4.063.129.435.850.804.221
        2.865.073.155.741.120.117   5.749.834.437.609.765.994
        6.804.196.764.189.408.435   17.060.469.201.292.988.508
        9.475.383.556.737.206.708   12.876.344.085.611.465.020
      ::  3rd round constants
        13.835.756.199.368.269.249  1.648.753.455.944.344.172
        9.836.124.473.569.258.483   12.867.641.597.107.932.229
        11.254.152.636.692.960.595  16.550.832.737.139.861.108
        11.861.573.970.480.733.262  1.256.660.473.588.673.495
        13.879.506.000.676.455.136  10.564.103.842.682.358.721
        16.142.842.524.796.397.521  3.287.098.591.948.630.584
        685.911.471.061.284.805     5.285.298.776.918.878.023
        18.310.953.571.768.047.354  3.142.266.350.630.002.035
      ::  4th round constants
        549.990.724.933.663.297     4.901.984.846.118.077.401
        11.458.643.033.696.775.769  8.706.785.264.119.212.710
        12.521.758.138.015.724.072  11.877.914.062.416.978.196
        11.333.318.251.134.523.752  3.933.899.631.278.608.623
        16.635.128.972.021.157.924  10.291.337.173.108.950.450
        4.142.107.155.024.199.350   16.973.934.533.787.743.537
        11.068.111.539.125.175.221  17.546.769.694.830.203.606
        5.315.217.744.825.068.993   4.609.594.252.909.613.081
      ::  5th round constants
        3.350.107.164.315.270.407   17.715.942.834.299.349.177
        9.600.609.149.219.873.996   12.894.357.635.820.003.949
        4.597.649.658.040.514.631   7.735.563.950.920.491.847
        1.663.379.455.870.887.181   13.889.298.103.638.829.706
        7.375.530.351.220.884.434   3.502.022.433.285.269.151
        9.231.805.330.431.056.952   9.252.272.755.288.523.725
        10.014.268.662.326.746.219  15.565.031.632.950.843.234
        1.209.725.273.521.819.323   6.024.642.864.597.845.108
      ==
    ::
    ::  length = 7 * state-size = 112
    :~
    ::  1st round constants
      1.332.676.891.236.936.200   16.607.633.045.354.064.669
      12.746.538.998.793.080.786  15.240.351.333.789.289.931
      10.333.439.796.058.208.418  986.873.372.968.378.050
      153.505.017.314.310.505     703.086.547.770.691.416
      8.522.628.845.961.587.962   1.727.254.290.898.686.320
      199.492.491.401.196.126     2.969.174.933.639.985.366
      1.607.536.590.362.293.391   16.971.515.075.282.501.568
      15.401.316.942.841.283.351  14.178.982.151.025.681.389
    ::  2nd round constants
      2.916.963.588.744.282.587   5.474.267.501.391.258.599
      5.350.367.839.445.462.659   7.436.373.192.934.779.388
      12.563.531.800.071.493.891  12.265.318.129.758.141.428
      6.524.649.031.155.262.053   1.388.069.597.090.660.214
      3.049.665.785.814.990.091   5.225.141.380.721.656.276
      10.399.487.208.361.035.835  6.576.713.996.114.457.203
      12.913.805.829.885.867.278  10.299.910.245.954.679.423
      12.980.779.960.345.402.499  593.670.858.850.716.490
    ::  3rd round constants
      12.184.128.243.723.146.967  1.315.341.360.419.235.257
      9.107.195.871.057.030.023   4.354.141.752.578.294.067
      8.824.457.881.527.486.794   14.811.586.928.506.712.910
      7.768.837.314.956.434.138   2.807.636.171.572.954.860
      9.487.703.495.117.094.125   13.452.575.580.428.891.895
      14.689.488.045.617.615.844  16.144.091.782.672.017.853
      15.471.922.440.568.867.245  17.295.382.518.415.944.107
      15.054.306.047.726.632.486  5.708.955.503.115.886.019
    ::  4th round constants
      9.596.017.237.020.520.842   16.520.851.172.964.236.909
      8.513.472.793.890.943.175   8.503.326.067.026.609.602
      9.402.483.918.549.940.854   8.614.816.312.698.982.446
      7.744.830.563.717.871.780   14.419.404.818.700.162.041
      8.090.742.384.565.069.824   15.547.662.568.163.517.559
      17.314.710.073.626.307.254  10.008.393.716.631.058.961
      14.480.243.402.290.327.574  13.569.194.973.291.808.551
      10.573.516.815.088.946.209  15.120.483.436.559.336.219
    ::  5th round constants
      3.515.151.310.595.301.563   1.095.382.462.248.757.907
      5.323.307.938.514.209.350   14.204.542.692.543.834.582
      12.448.773.944.668.684.656  13.967.843.398.310.696.452
      14.838.288.394.107.326.806  13.718.313.940.616.442.191
      15.032.565.440.414.177.483  13.769.903.572.116.157.488
      17.074.377.440.395.071.208  16.931.086.385.239.297.738
      8.723.550.055.169.003.617   590.842.605.971.518.043
      16.642.348.030.861.036.090  10.708.719.298.241.282.592
    ::  6th round constants
      12.766.914.315.707.517.909  11.780.889.552.403.245.587
      113.183.285.481.780.712     9.019.899.125.655.375.514
      3.300.264.967.390.964.820   12.802.381.622.653.377.935
      891.063.765.000.023.873     15.939.045.541.699.412.539
      3.240.223.189.948.727.743   4.087.221.142.360.949.772
      10.980.466.041.788.253.952  18.199.914.337.033.135.244
      7.168.108.392.363.190.150   16.860.278.046.098.150.740
      13.088.202.265.571.714.855  4.712.275.036.097.525.581
    ::  7th round constants
      16.338.034.078.141.228.133  1.455.012.125.527.134.274
      5.024.057.780.895.012.002   9.289.161.311.673.217.186
      9.401.110.072.402.537.104   11.919.498.251.456.187.748
      4.173.156.070.774.045.271   15.647.643.457.869.530.627
      15.642.078.237.964.257.476  1.405.048.341.078.324.037
      3.059.193.199.283.698.832   1.605.012.781.983.592.984
      7.134.876.918.849.821.827   5.796.994.175.286.958.720
      7.251.651.436.095.127.661   4.565.856.221.886.323.991
    ==
  ::
  ::  +mds-matrix-first-column: the mds matrix is determined by any column
  ++  mds-matrix-first-column
    ::  length = state-size = 16
    ^-  (list belt)
    :~  61.402  1.108   28.750  33.823  7.454   43.244  53.865  12.034
        56.951  27.521  41.351  40.901  12.021  59.689  26.798  17.845
    ==
  ::
  ++  mds-first-column-fft
    ^-  (list belt)
    :~  524.757                     12.925.608.463.476.951.657
        15.523.111.717.718.611.263  16.532.524.212.944.612.299
        7.588.283.897.142.562.168   15.572.835.691.259.601.621
        2.891.241.344.421.052.990   4.554.321.248.572.910.116
        52.427                      3.009.663.708.287.279.710
        15.424.499.013.074.857.791  4.457.503.309.926.164.732
        10.858.460.172.271.996.281  243.395.401.255.089.650
        3.054.636.063.615.042.110   16.491.124.241.935.763.107
    ==
  ::
  ::  list of rows
  ++  mds-matrix
    ^~
    ^-  (list (list belt))
    |^
    ^~((gen-circulant-matrix mds-matrix-first-column))
    ::
    ::  +gen-circulant-matrix: use first column to produce mds-matrix
    ::
    ::    The first row of mds is a cyclic rotation of the flop of the
    ::    first column, and successive rows are obtained by more cyclic
    ::    rotations.
    ++  gen-circulant-matrix
      |=  first-column=(list @)
      ^-  (list (list @))
      %+  spun  (range (lent first-column))
      |=  [i=@ acc=_(flop first-column)]
      [(rotate acc) (rotate acc)]
    ::
    ::  +rotate: cyclic vector rotation
    ++  rotate
      |=  lst=(list @)
      ^-  (list @)
      [(rear lst) (snip lst)]
    --
  ::
  ++  primitive-16-roots
    ^-  (list belt)
    :~  4.096  ::  o  (o=2^12; 2 is a primitive 192nd rou, & 192=12*16)
        68.719.476.736              ::  o^3
        1.152.921.504.606.846.976   ::  o^5
        4.503.599.626.321.920       ::  o^7
        18.446.744.069.414.580.225  ::  o^9
        18.446.744.000.695.107.585  ::  o^11
        17.293.822.564.807.737.345  ::  o^13
        18.442.240.469.788.262.401  ::  o^15
    ==
  ::
  ++  layer-two-twiddles
    ^~  ^-  (map belt (list belt))
    %-  ~(gas by *(map belt (list belt)))
    %+  turn  primitive-16-roots
    |=  r=belt
    =/  fourth-rou  (bpow r (div 16 4))
    :-  r  (turn (range 2) |=(i=@ (bpow fourth-rou i)))
  ::
  ++  layer-three-twiddles
    ^~  ^-  (map belt (list belt))
    %-  ~(gas by *(map belt (list belt)))
    %+  turn  primitive-16-roots
    |=  r=belt
    =/  eighth-rou  (bpow r (div 16 8))
    :-  r  (turn (range 4) |=(i=@ (bpow eighth-rou i)))
  ::
  ++  layer-four-twiddles
    ^~  ^-  (map belt (list belt))
    %-  ~(gas by *(map belt (list belt)))
    %+  turn  primitive-16-roots
    |=  r=belt
    :-  r  (turn (range 8) |=(i=@ (bpow r i)))
  ::
  ::
  ::  For the cognoscenti:
  ::
  ::    The formal mathematical specification of Tip5 involves conversion to
  ::    and from Montgomery representation in the S-box layer of each round.
  ::    In practice it is inefficient to do this, so the MDS and round constants
  ::    layers are done in Montgomery representation. This demands that the
  ::    round constants be given in Montgomery representation, but, confusingly
  ::    enough, does not demand the same of the MDS matrix constants, for the
  ::    simple reason that ordinary multiplication of melt a' (whose underlying
  ::    belt is a) and belt b yields (ab)'; this owes to the fact that
  ::    "Montification" is multiplication by 2^64 mod p = 2^32 - 1. Basically,
  ::    we "stay in Montgomery space" if we multiply a melt and a belt.
  ::
  ::    This manifests clearly in +hash-10, where the input is montified and
  ::    the output is demontified before being returned.
  +|  %dev-funcs
  ++  init-tip5-state
    |=  =domain
    ^-  tip5-state
    ?-    domain
        %variable
      ^~((reap state-size 0))
    ::
        %fixed
      ^~((weld (reap rate 0) (reap capacity (montify 1))))
    ==
  ::
  ::  +offset-fermat-cube-map: generates and can be used to test lookup-table
  ++  offset-fermat-cube-map
    |=  x=@
    ^-  @
    ?>  (lth x 256)
    =/  xx  +(x)
    %-  mod  :_  257
    (add :(mul xx xx xx) 256)
  ::
  ::  +split-and-lookup: splits b into bytes, applies offset-fermat-cube-map to each, & recombines bytes
  ++  split-and-lookup
    |=  m=melt
    ^-  melt
    ::  split
    =/  bytes=(list @)  (weld (rip 3 m) (reap (sub 8 (lent (rip 3 m))) 0))
    ::  lookup=offset-fermat-cube
    =.  bytes  (turn bytes |=(byte=@ (snag byte lookup-table)))
    ::  recombine
    (can 3 (zip-up (reap 8 1) bytes))
  ::
  ::  +cyclomul16-fft: fft of f and g, hadamard multiply result, then ifft.
  ::
  ::    This is different than polynomial multiplication of f and g bc output length equals input lengths.
  ::    In fact, it is polynomial multiplication modulo the cyclotomic polynomial X^16 - 1. (Not obvious.)
  ++  cyclomul16-fft
    |=  [f=(list belt) g=(list belt)]
    ^-  (list belt)
    ?>  ?&(=((lent f) state-size) =((lent g) state-size))
    =/  [fx=fpoly gx=fpoly]  [(lift-to-fpoly f) (lift-to-fpoly g)]
    %-  turn  :_  drop
    %-  fpoly-to-list
    (fp-ifft (~(zip fop (fp-fft fx)) (fp-fft gx) fmul))
  ::
  ::  +fft-16-w-root:
  ++  fft-16-w-root
    ~/  %fft-16-w-root
    |=  [bp=(list belt) r=belt]
    ^-  (list belt)
    |^
    =/  current-layer=(list (list belt))
      %-  turn  :_  interpolate-linear
      (zip-up (scag 8 bp) (slag 8 bp))
    =.  current-layer
      %+  turn  (zip-up (scag 4 current-layer) (slag 4 current-layer))
      (cury interpolate-next (~(got by layer-two-twiddles) r))
    =/  current-layer
      %+  turn  (zip-up (scag 2 current-layer) (slag 2 current-layer))
      (cury interpolate-next (~(got by layer-three-twiddles) r))
    %-  interpolate-next
    :+  (~(got by layer-four-twiddles) r)
    (snag 0 current-layer)  (snag 1 current-layer)
    ::
    ++  interpolate-linear
      |=  [b=belt c=belt]
      ~[(badd b c) (bsub b c)]
    ::
    ++  interpolate-next
      |=  [twids=(list belt) dft1=(list belt) dft2=(list belt)]
      ^-  (list belt)
      =/  right  (zip dft2 twids bmul)
      %+  weld
        (zip dft1 right badd)
      (zip dft1 right bsub)
    --
  ::
  ++  fft-16
    ~/  %fft-16
    |=  bp=(list belt)
    (fft-16-w-root bp 4.096)
  ::
  ++  ifft-16
    ~/  %ifft-16
    |=  evals=(list belt)
    ^-  (list belt)
    %-  turn  :_  |=(=belt (bmul belt 17.293.822.565.076.172.801))
    (fft-16-w-root evals 18.442.240.469.788.262.401)
  ::
  ::  +mds-cyclomul: applies the mds matrix as a linear transformation to state
  ::  w/o doing matrix multiplication
  ++  mds-cyclomul
    ~/  %mds-cyclomul
    |=  =state
    ^-  ^state
    %-  ifft-16
    (zip mds-first-column-fft (fft-16 state) bmul)
  ::
  ::  +mds-cyclomul-m: applies the mds matrix as a linear transformation to state
  ::  doing matrix multiplication.
  ++  mds-cyclomul-m
  ~/  %mds-cyclomul-m
  |=  v=(list @)
  ^-  (list @)
  %+  turn
    mds-matrix
  |=  row=(list @)
  (mod (inner-product row v) p)
  ::
  ++  inner-product
  ~/  %inner-product
  |=  [l=(list @) t=(list @)]
  ^-  belt
  %^  zip-roll  l  t
  |=  [[a=@ b=@] res=@]
  (add res (mul a b))
  ::
  ::  +sbox-layer: applies fermat map to first 4 elements and 7th-power map to remainder
  ++  sbox-layer
    ~/  %sbox-layer
    |=  =state
    ^-  (list melt)
    ?>  =((lent state) state-size)
    %+  weld
      (turn (scag num-split-and-lookup state) split-and-lookup)
    %+  turn  (slag num-split-and-lookup state)
    ::  computes b^7 in 4 base field multiplications
    ::
    ::  Note that we are able to replace montiplys with
    ::  bmuls due to the fact that R^3 = 1 mod p. Thus:
    ::         m^7 = R^7*b^7
    ::            = (R^3)^2*R*b^7
    ::            = R*b^7 mod p
    |=  m=melt
    ^-  melt
    =/  sq  (bmul m m)
    =/  qu  (bmul sq sq)
    :(bmul m sq qu)
  ::
  ::  +round: one round has three components; sbox, linear (mds), add round constants
  ++  round
    ~/  %round
    |=  [sponge=tip5-state round-index=@]
    ^-  tip5-state
    =.  sponge  (mds-cyclomul-m (sbox-layer sponge))
    %^  zip  sponge  (range state-size)
    |=  [b=belt i=@]
    (badd b (snag (add (mul round-index state-size) i) round-constants))
  ::
  ::  +permutation: applies rounds iteratively, num-rounds times
  ++  permutation
    ~/  %permutation
    |=  sponge=tip5-state
    ^-  tip5-state
    %+  roll  (range num-rounds)
    |=  [round-index=@ acc=_sponge]
    (round acc round-index)
  ::
  ::  +trace: a record of the tip5-state's evolution during permutation
  ++  trace
    ~/  %trace
    |=  sponge=tip5-state
    ^-  (list tip5-state)
    :-  sponge
    %+  spun  (range num-rounds)
    |=  [i=@ sp=_sponge]
    [(round sp i) (round sp i)]
  ::
  ::  +hash-10: hash list of 10 belts into a list of 5 belts
  ++  hash-10
    ~/  %hash-10
    |=  input=(list belt)
    ::  output length is 5
    ^-  (list belt)
    ?>  =((lent input) rate)
    ?>  (levy input based)
    =.  input   (turn input montify)
    =/  sponge  (init-tip5-state %fixed)
    =.  sponge  (permutation (weld input (slag rate sponge)))
    (turn (scag digest-length sponge) mont-reduction)
  ::
  ::  +hash-varlen: hash a list of belts, but in practice only a single belt
  ::
  ::    you might think this is the function for hashing lists of belts,
  ::    but you'd be wrong. +hash-varlen is part of the tip5 spec, so
  ::    we need to have it. but because hoon is structurally typed, the
  ::    type system cannot distinguish between a list ~[1 2 3] and a tuple
  ::    [1 2 3 0]. unfortunately, +hash-noun of [1 2 3 0] is different from
  ::    +hash-varlen of ~[1 2 3]. having identical nouns of belts with different
  ::    hashes would be catastrophic.
  ::
  ::    the two tip5 primitives are +hash-varlen and +hash-ten-cell.
  ::    +hash-ten-cell can't be used on a single atom, so we must use
  ::    +hash-varlen on it. +hash-ten-cell is only to be used to combine two
  ::    hashes. so +hash-noun works out to be: +hash-varlen on every belt
  ::    atom, and +hash-ten-cell on every cell.
  ::
  ::    we also make use of +hash-varlen for hashing marys. see +hash-mary
  ::    for more information
  ++  hash-varlen
    ~/  %hash-varlen
    |=  input=(list belt)
    ^-  (list belt)
    =/  spo  (new:sponge)
    =.  spo  (absorb:spo input)
    =^  output  spo
      (squeeze:spo)
    (scag digest-length output)
  ::
  ++  sponge
    ~%  %sponge  +>  ~
    |_  sponge=tip5-state
    ++  new
      |.  ^+  +.$
      =.  sponge  (init-tip5-state %variable)
      +.$
    ::
    ++  absorb
      ~/  %absorb
      |=  input=(list belt)
      ^+  +>.$
      =*  rng  +>.$
      |^
      ::  assert that input is made of base field elements
      ?>  (levy input based)
      =/  [q=@ r=@]  (dvr (lent input) rate)
      ::  pad input with ~[1 0 ... 0] to be a multiple of rate
      =.  input  (weld input [1 (reap (dec (sub rate r)) 0)])
      ::  bring input into montgomery space
      =.  input  (turn input montify)
      |-
      =.  sponge  (absorb-rate (scag rate input))
      ?:  =(q 0)
        rng
      $(q (dec q), input (slag rate input))
      ::
      ++  absorb-rate
        |=  input=(list belt)
        ^+  sponge
        ?>  =((lent input) rate)
        =.  sponge  (weld input (slag rate sponge))
        $:permute
      --
    ::
    ++  permute
      ~%  %permute  +  ~
      |.  ^+  sponge
      (permutation sponge)
    ::
    ++  squeeze
      ~%  %squeeze  +  ~
      |.  ^+  [*(list belt) +.$]
      =*  rng  +.$
      ::  squeeze out the full rate and bring out of montgomery space
      =/  output  (turn (scag rate sponge) mont-reduction)
      =.  sponge  $:permute
      [output rng]
    --
  ::
  ::  +list-to-tuple: strips ~ from a list and yields a tuple
  ::
  ::    hash-10 returns a length=5 list and this function is useful
  ::    for converting it to a tuple
  ++  list-to-tuple
    ~/  %list-to-tuple
    |=  lis=(list @)
    ::  address of [a_{k-1} ~] (final nontrivial tail of list)
    =+  (dec (bex (lent lis)))
    .*  lis
    [10 [- [0 (mul 2 -)]] [0 1]]
  ::
  ::  +tog: Tip5 Sponge PRNG
  ::
  ++  tog
    ~%  %tog  +>  ~
    |_  spo=tip5-state
    ::
    ++  new
      |=  sponge-state=tip5-state
      ~(. tog sponge-state)
    ::
    ++  belts
      ~/  %belts
      |=  n=@
      ^+  [*(list belt) +>.$]
      =*  rng  +>.$
      =/  sponge  ~(. sponge spo)
      =/  [q=@ r=@]  (dvr n rate)
      =|  output=(list belt)
      |-
      =^  out  sponge
        (squeeze:sponge)
      =.  spo  sponge:sponge
      ?:  =(q 0)
        [(weld output (scag r out)) rng]
      $(q (dec q), output (weld output out))
    ::
    ++  felt
      ~%  %felt  +  ~
      |.  ^+  [*^felt +.$]
      =^  felt-list  +.$  (felts 1)
      [(head felt-list) +.$]
    ::
    ++  felts
      ~/  %felts
      |=  n=@
      ^+  [*(list ^felt) +>.$]
      =*  outer  +>.$
      =^  lis-belts  +>.$  (belts (mul n 3))
      =|  ret=(list ^felt)
      =/  i  0
      |-
      ?:  =(i n)
        [(flop ret) outer]
      =/  f=^felt  (frep (scag 3 lis-belts))
      $(ret [f ret], lis-belts (slag 3 lis-belts), i +(i))
    ::
    ++  index
      ~/  %index
      |=  size=@
      ^+  [*@ +>.$]
      =^  belt-list  +>.$  (belts 1)
      [(mod (head belt-list) size) +>.$]
    ::
    ++  indices
      ~/  %indices
      |=  [n=@ size=@ reduced-size=@]
      ^+  [*(list @) +>.$]
      =*  rng  +>.$
      ~|  "cannot sample more indices than available in last codeword"
      ?>  (lte n reduced-size)
      =|  indices=(list @)
      =|  reduced-indices=(list @)
      |-
      ?:  (gte (lent indices) n)
        [(flop indices) rng]
      =^  index  rng  (index size)
      =/  reduced-index  (mod index reduced-size)
      ?^  (find reduced-index^~ reduced-indices)
        $
      ?^  (find index^~ indices)  $
      %_  $
        indices          [index indices]
        reduced-indices  [reduced-index reduced-indices]
      ==
    --
  ::
  ++  test-tip5
    |%
    ::
    ++  lookup-table-test
      ^-  ?
      ?>  =((lent lookup-table) 256)
      %+  levy  (range 256)
      |=  i=@
      =((snag i lookup-table) (offset-fermat-cube-map i))
    ::
    ++  fermat-cube-map-is-permutation
      ^-  ?
      =((range 256) (sort lookup-table lth))
    ::
    ::  needs Blake3 hash function; I've painstakingly checked our list against the one in Neptune's code
    ++  round-constants-test
      ^-  ?
      !!
    ::
  ::  +reduce-mod-cyclotomic: reduce f mod X^n-1
    ++  reduce-mod-cyclotomic
      |=  [f=(list belt) n=@]
      ^-  (list belt)
      =.  f  (weld f (reap (sub n (mod (lent f) n)) 0))
      =/  result  (reap n 0)
      |-
      ?~  f
        result
      =/  f-lst  `(list belt)`f
      %_  $
        f       (slag n f-lst)
        result  (zip (scag n f-lst) result badd)
      ==
    ::
    ++  cyclomul-is-bpmul-mod-cyclotomic-test
      |=  [f=(list belt) g=(list belt)]
      ^-  ?
      ?>  ?&((lte (lent f) 16) (lte (lent g) 16))
      =.  f  (weld f (reap (sub 16 (lent f)) 0))
      =.  g  (weld g (reap (sub 16 (lent g)) 0))
      =/  prod=(list belt)  (bpoly-to-list (bpmul (init-bpoly f) (init-bpoly g)))
      =.  prod  (weld prod (reap (sub 32 (lent prod)) 0))
      .=  (cyclomul16-fft f g)
      (zip (scag 16 prod) (slag 16 prod) badd)
    ::
    ++  matrix-vector-product
      |=  [matrix=(list (list belt)) vector=(list belt)]
      ^-  (list belt)
      %+  turn  matrix
      ::  dot product
      |=  row=(list belt)
      ^-  belt
      %+  roll  (zip-up row vector)
      |=  [[entry=belt component=belt] acc=belt]
      ^-  belt
      (badd acc (bmul entry component))
    ::
    ++  mds-cyclomul-test
      |=  input=(list belt)
      ^-  ?
      ?>  =((lent input) 16)
      =((mds-cyclomul input) (matrix-vector-product mds-matrix input))
    ::
    ++  test-hash10-0
      =/  expected=(list belt)
        :~  941.080.798.860.502.477
            5.295.886.365.985.465.639
            14.728.839.126.885.177.993
            10.358.449.902.914.633.406
            14.220.746.792.122.877.272
        ==
      =/  got  (hash-10 (reap 10 0))
      (zip-up expected got)
    ::
    ++  hash10-test-vectors
      ^-  ?
      =/  input=(list belt)  (reap rate 0)
      =+  %+  roll  (range 6)
          |=  [i=@ in=_input]
          =/  out  (hash-10 in)
          :(weld (scag i in) out (reap (sub 5 i) 0))
      =/  digest  (hash-10 -)
      =/  final=(list belt)
        :~  10.869.784.347.448.351.760
            1.853.783.032.222.938.415
            6.856.460.589.287.344.822
            17.178.399.545.409.290.325
            7.650.660.984.651.717.733
        ==
      =/  expected-got=(list [belt belt])  (zip-up final digest)
      ~&  expected-got
      (levy expected-got |=([a=belt b=belt] =(a b)))
    ::
    ::  comment out the jet hint on hash-varlen before running this test
    ++  test-hash-varlen
      |=  [num=@ seed=@]
      ^-  ?
      |^
      =|  counter=@
      |-
      ?:  =(counter num)  %.y
      =/  [tv=(list belt) new-seed=@]
        %^  spin  (range counter)  seed
        |=  [i=@ sd=belt]
        =-  -^-
        (badd (bmul sd sd) 1)
      ?.  =((hash-varlen tv) (old-hash-varlen tv))
        ~&  fail-on+tv  %.n
      $(counter +(counter), seed new-seed)
      ::
      ++  old-hash-varlen
        |=  input=(list belt)
        =/  [q=@ r=@]  (dvr (lent input) rate)
        ::  append ~[1 0 ... 0] to input
        =.  input   (turn (weld input [1 (reap (dec (sub rate r)) 0)]) montify)
        =/  sponge  (init-tip5-state %variable)
        =-  (turn (scag digest-length sp) mont-reduction)
        %+  roll  (gulf 0 q)
        |=  [i=@ [sp=_sponge in=_input]]
        :_  (slag rate in)
        (permutation (weld (scag rate in) (slag rate sp)))
      --
    --
  --
::
::  TODO: needs to be audited and thoroughly tested
++  cheetah
  ~%  %cheetah  ..cheetah  ~
  ::  degree-six extension of F_p is cheetah curve's base field
  |%
  ::  f6lt: element of F_p[x]/(x^6 - 7)
  +$  f6lt     [a0=belt a1=belt a2=belt a3=belt a4=belt a5=belt]
  ++  f6lt-based
    |=  f=f6lt
    =+  [a=belt b=belt c=belt d=belt e=belt f=belt]=f
    ?&  (based a)
        (based b)
        (based c)
        (based d)
        (based e)
        (based f)
    ==

  ++  f6lt-dyck-word
    ^-  (list @)
    ~[0 1 0 1 0 1 0 1 0 1]
  ++  f6lt-cell-dyck-word
    ^~  ^-  (list @)
    (weld [0 f6lt-dyck-word] [1 f6lt-dyck-word])
  ++  f6lt-triple-dyck-word
    ^~  ^-  (list @)
    :(weld [0 f6lt-dyck-word] [1 [0 f6lt-dyck-word]] [1 f6lt-dyck-word])
  ++  f6lt-triple-cell-dyck-word
    ^~  ^-  (list @)
    (weld [0 f6lt-triple-dyck-word] [1 f6lt-triple-dyck-word])
  ++  f6-zero  `f6lt`[0 0 0 0 0 0]
  ++  f6-one   `f6lt`[1 0 0 0 0 0]
  ::
  ++  f6lt-to-list
    |=  f=f6lt
    ^-  (list belt)
    ~[a0.f a1.f a2.f a3.f a4.f a5.f]
  ::
  ++  list-to-f6lt
    |=  lis=(list belt)
    ^-  f6lt
    ?>  =((lent lis) 6)
    ::  63  = axis of [a_5 ~] in ~[a0 ... a_5]
    ::  126 = axis of a_5 in ~[a0 ... a_5]
    ::  replace axis 63 (=[a_5 ~]) of *[lis [0 1]]=lis with *[lis [0 126]]=a_5
    =/  n
      .*  lis
      [10 [63 [0 126]] [0 1]]
    ?>  ?=(f6lt n)  n
  ::
  ++  f6-add
    |=  [f1=f6lt f2=f6lt]
    ^-  f6lt
    :*  (badd a0.f1 a0.f2)
        (badd a1.f1 a1.f2)
        (badd a2.f1 a2.f2)
        (badd a3.f1 a3.f2)
        (badd a4.f1 a4.f2)
        (badd a5.f1 a5.f2)
    ==
  ::
  ++  f6-neg
    |=  f=f6lt
    ^-  f6lt
    :*  (bneg a0.f)
        (bneg a1.f)
        (bneg a2.f)
        (bneg a3.f)
        (bneg a4.f)
        (bneg a5.f)
    ==
  ::
  ++  f6-sub
    |=  [f1=f6lt f2=f6lt]
    ^-  f6lt
    (f6-add f1 (f6-neg f2))
  ::
  ++  f6-scal
    |=  [c=belt f=f6lt]
    ^-  f6lt
    :*  (bmul c a0.f)
        (bmul c a1.f)
        (bmul c a2.f)
        (bmul c a3.f)
        (bmul c a4.f)
        (bmul c a5.f)
    ==
  ::
  ::  +karat3: mults 2 quadratic polys w only 6 bmuls (vs naive 9)
  ++  karat3
    |=  [[a0=belt a1=belt a2=belt] [b0=belt b1=belt b2=belt]]
    ^-  [c0=belt c1=belt c2=belt c3=belt c4=belt]
    =/  [m0=belt m1=belt m2=belt]
      [(bmul a0 b0) (bmul a1 b1) (bmul a2 b2)]
    :*  m0
        (bsub (bmul (badd a0 a1) (badd b0 b1)) (badd m0 m1))
        (badd (bsub (bmul (badd a0 a2) (badd b0 b2)) (badd m0 m2)) m1)
        (bsub (bmul (badd a1 a2) (badd b1 b2)) (badd m1 m2))
        m2
    ==
  ::
  ::  +karat3-square: squares quadratic poly w only 5 bmuls
  ++  karat3-square
    |=  [a0=belt a1=belt a2=belt]
    ^-  [c0=belt c1=belt c2=belt c3=belt c4=belt]
    =/  [m0=belt m2=belt]    [(bmul a0 a0) (bmul a2 a2)]
    =/  [n01=belt n12=belt]  [(bmul a0 a1) (bmul a1 a2)]
    =:  n01  (badd n01 n01)
        n12  (badd n12 n12)
      ==
    =/  tri2=belt
      =/  tri  :(badd a0 a1 a2)
      (bmul tri tri)
    =/  coeff2  (bsub tri2 :(badd m0 m2 n01 n12))
    [m0 n01 coeff2 n12 m2]
  ::
  ++  f6-mul
    |=  [f=f6lt g=f6lt]
    ^-  f6lt
    =/  f0g0  (karat3 [a0.f a1.f a2.f] [a0.g a1.g a2.g])
    =/  f1g1  (karat3 [a3.f a4.f a5.f] [a3.g a4.g a5.g])
    =/  foil
      %-  karat3
      :-  [(badd a0.f a3.f) (badd a1.f a4.f) (badd a2.f a5.f)]
      [(badd a0.g a3.g) (badd a1.g a4.g) (badd a2.g a5.g)]
    =/  cross=[c0=belt c1=belt c2=belt c3=belt c4=belt]
      :*  (bsub c0.foil (badd c0.f0g0 c0.f1g1))
          (bsub c1.foil (badd c1.f0g0 c1.f1g1))
          (bsub c2.foil (badd c2.f0g0 c2.f1g1))
          (bsub c3.foil (badd c3.f0g0 c3.f1g1))
          (bsub c4.foil (badd c4.f0g0 c4.f1g1))
      ==
    :*  (badd c0.f0g0 (bmul 7 (badd c3.cross c0.f1g1)))
        (badd c1.f0g0 (bmul 7 (badd c4.cross c1.f1g1)))
        (badd c2.f0g0 (bmul 7 c2.f1g1))
        :(badd c3.f0g0 c0.cross (bmul 7 c3.f1g1))
        :(badd c4.f0g0 c1.cross (bmul 7 c4.f1g1))
        c2.cross
    ==
  ::
  ::  +f6-square: uses karat3-square for more efficiency than (f6-mul f f)
  ++  f6-square
    |=  f=f6lt
    ^-  f6lt
    =/  lo   [a0.f a1.f a2.f]
    =/  hi   [a3.f a4.f a5.f]
    =/  lo2  (karat3-square lo)
    =/  hi2  (karat3-square hi)
    =/  folded2  ::  (lo + hi)^2
      (karat3-square [(badd a0.f a3.f) (badd a1.f a4.f) (badd a2.f a5.f)])
    =/  cross=[c0=belt c1=belt c2=belt c3=belt c4=belt]
      :*  (bsub c0.folded2 (badd c0.lo2 c0.hi2))
          (bsub c1.folded2 (badd c1.lo2 c1.hi2))
          (bsub c2.folded2 (badd c2.lo2 c2.hi2))
          (bsub c3.folded2 (badd c3.lo2 c3.hi2))
          (bsub c4.folded2 (badd c4.lo2 c4.hi2))
      ==
    :*  :(badd c0.lo2 (bmul 7 c3.cross) (bmul 7 c0.hi2))
        :(badd c1.lo2 (bmul 7 c4.cross) (bmul 7 c1.hi2))
        (badd c2.lo2 (bmul 7 c2.hi2))
        :(badd c3.lo2 c0.cross (bmul 7 c3.hi2))
        :(badd c4.lo2 c1.cross (bmul 7 c4.hi2))
        c2.cross
    ==
  ::
  ++  f6-pow
    |=  [f=f6lt n=@]
    ^-  f6lt
    =/  acc=f6lt  f6-one
    |-
    ?:  =(n 0)  acc
    %_  $
      acc  ?:(=((end 0 n) 0) acc (f6-mul acc f))
      f    (f6-square f)
      n    (rsh 0 n)
    ==
  ::
  ++  f6-inv
    |=  f=f6lt
    ^-  f6lt
    ?:  =(f f6-zero)
      ~|('+f6-inv: zero point has no inverse' !!)
    =/  eucl
      %+  bpegcd
        (init-bpoly (f6lt-to-list f))
      (init-bpoly ~[(bneg 7) 0 0 0 0 0 1])
    %-  list-to-f6lt
    =+  %-  bpoly-to-list
        (bpscal (binv (snag 0 (bpoly-to-list d.eucl))) u.eucl)
    (weld - (reap (sub 6 (lent -)) 0))
  ::
  ++  f6-div
    ~/  %f6-div
    |=  [f1=f6lt f2=f6lt]
    ^-  f6lt
    (f6-mul f1 (f6-inv f2))
  ::
  ::  elliptic cheetah curve operations
  ++  curve
    ~%  %curve  ..curve  ~
    |%
    ++  b     `f6lt`[395 1 0 0 0 0]
    ::
    ::  +gx: x-coordinate of g in affine coordinates
    ++  gx
      ^-  f6lt
      :*  2.754.611.494.552.410.273
          8.599.518.745.794.843.693
          10.526.511.002.404.673.680
          4.830.863.958.577.994.148
          375.185.138.577.093.320
          12.938.930.721.685.970.739
      ==
    ::  +gy: y-coordinate of g in affine coordinates
    ++  gy
      ^-  f6lt
      :*  15.384.029.202.802.550.068
          2.774.812.795.997.841.935
          14.375.303.400.746.062.753
          10.708.493.419.890.101.954
          13.187.678.623.570.541.764
          9.990.732.138.772.505.951
      ==
    ::
    ::  +g-order: order of g; 255 bits
    ++  g-order
      0x7af2.599b.3b3f.22d0.563f.bf0f.990a.37b5.327a.a723.3015.7722.d443.623e.aed4.accf
    ::  +a-pt: affine coordinates
    ::
    ::    If the infinity flag inf if %.n, this is an (x, y) point in the
    ::    affine plane, which we identify with the z=1 plane in projective
    ::    space. If %.y, this is a point on the projective line
    ::    "at infinity," i.e. (x, y) is identified with [x, y, 0] in
    ::    projective space. By the projective equivalence relation, this
    ::    representation is not unique.
    +$  a-pt  [x=f6lt y=f6lt inf=?]
    ::
    ::  +a-pt-based: checks if elements in a-pt are in base field.
    ++  a-pt-based
      |=  a-pt
      ?&  (f6lt-based x)
          (f6lt-based y)
      ==
    ::
    ++  a-pt-dyck-word
      ^~  ^-  (list @)
      (snoc (weld [0 f6lt-dyck-word] [1 0 f6lt-dyck-word]) 1)
    ++  a-pt-cell-dyck-word
      ^~  ^-  (list @)
      (weld [0 a-pt-dyck-word] [1 a-pt-dyck-word])
    ::
    ::  +a-id
    ::
    ::    The curve is defined by y^2 = x^3 + x + b over F^6.
    ::    To add the point at infinity we interpret these (x, y)
    ::    points as [x, y, 1] in P^2 over F^6. In projective [x, y, z]
    ::    coordinates the equation is y^2z = x^3 + xz^2 + bz^3. A
    ::    point at infinity (z=0), must satisfy x^3=0 so [0, y, 0] (y≠0)
    ::    is the only point at infinity on the curve (this is the same
    ::    pt for any y by the projective equivalence relation). Thus we
    ::    can take [0 1 %.y] as the identity point.
    ::
    ::    Note that [0 -1 %.y] also represents the identity point.
    ++  a-id  `a-pt`[f6-zero f6-one %.y]
    ++  a-gen
      ^-  a-pt
      [gx gy %.n]
    ::
    ::  +affine: curve operations in affine coordinates
    ++  affine
      ~%  %affine  ..affine  ~
      |%
      ++  in-g
        |=  p=a-pt
        =(a-id (ch-scal g-order p))
      ::
      ::  +ch-neg: negate a cheetah point
      ::
      ::    In Weierstrass form an elliptic curve has f([x y z]) = [x -y z] symmetry.
      ::    The line in the z=constant plane thru p and f(p) is vertical so passes
      ::    through O, the point at infinity; thus by the straight line relation for
      ::    elliptic curve addition, p + f(p) + O = O i.e. f(p) = -p.
      ++  ch-neg
        |=  p=a-pt
        ^-  a-pt
        [x.p (f6-neg y.p) inf.p]
      ::
      ::  +ch-add: add two cheetah points
      ++  ch-add-unsafe
        |=  [p=a-pt q=a-pt]
        ^-  a-pt
        =/  slope  (f6-div (f6-sub y.p y.q) (f6-sub x.p x.q))
        =/  x-out  (f6-sub (f6-square slope) (f6-add x.p x.q))
        :+  x-out
          (f6-sub (f6-mul slope (f6-sub x.p x-out)) y.p)
        %.n
      ::
      ++  ch-add
        |=  [p=a-pt q=a-pt]
        ^-  a-pt
        ?:  inf.p  q
        ?:  inf.q  p
        ?:  =(p (ch-neg q))  a-id
        ?:  =(p q)  (ch-double p)
        (ch-add-unsafe p q)
      ::
      ::  +ch-double-unsafe: generic add w/o special case checks
      ++  ch-double-unsafe
        |=  p=a-pt
        ^-  a-pt
        =/  slope
          %+  f6-div
            (f6-add (f6-scal 3 (f6-square x.p)) f6-one)
          (f6-scal 2 y.p)
        =/  x-out  (f6-sub (f6-square slope) (f6-scal 2 x.p))
        :+  x-out
          (f6-sub (f6-mul slope (f6-sub x.p x-out)) y.p)
        %.n
      ::
      ::  +ch-double: [2]p, p a cheetah point
      ::
      ::    Analog of squaring; fundamental for computing [n]p quickly.
      ::    Two special cases: the double of the point at infinity is itself;
      ::    and the double of any point with infinite slope is infinite. A
      ::    point with infinite slope is any point with y=0 by the equation
      ::    dy/dx = (3x^2 + 1)/2y.
      ++  ch-double
        |=  p=a-pt
        ^-  a-pt
        ?:  inf.p  a-id
        ?:  =(y.p f6-zero)  a-id
        (ch-double-unsafe p)
      ::
      ::  +ch-scal: compute [n]p, p a cheetah point
      ::
      ::    This is the action of Z on cheetah as an abelian group.
      ++  ch-scal
        ~/  %ch-scal
        |=  [n=@ p=a-pt]
        ^-  a-pt
        =/  acc  a-id
        |-
        ?:  =(n 0)  acc
        %_  $
          acc  ?:(=((end 0 n) 0) acc (ch-add acc p))
          n    (rsh 0 n)
          p    (ch-double p)
        ==
      --
    --
  ::
  ++  schnorr
    ~%  %schnorr  ..schnorr  ~
    |%
    ++  affine
      ~%  %affine  ..affine  ~
      |%
      ++  sign
        ~/  %sign
        |=  [sk-as-32-bit-belts=(list belt) m=noun-digest:tip5]
        ^-  [c=@ux s=@ux]
        =/  m-list  (leaf-sequence:shape m)
        =/  b-32  (bex 32)
        ?>  (levy sk-as-32-bit-belts |=(n=@ (lth n b-32)))
        =/  sk      (rep 5 sk-as-32-bit-belts)
        ?<  =(sk 0)
        ?>  (lth sk g-order:curve)
        =/  pubkey  (ch-scal:affine:curve sk a-gen:curve)
        =/  transcript=(list (list belt))
          [(f6lt-to-list x.pubkey) (f6lt-to-list y.pubkey) m-list sk-as-32-bit-belts ~]
        =/  nonce
          (trunc-g-order (hash-varlen:tip5 (zing transcript)))
        ?<  =(nonce 0)
        =/  scalar  (ch-scal:affine:curve nonce a-gen:curve)
        =/  pre-image
          %-  zing
          :~  (f6lt-to-list x.scalar)
              (f6lt-to-list y.scalar)
              (f6lt-to-list x.pubkey)
              (f6lt-to-list y.pubkey)
              m-list
          ==
        =/  chal
          (trunc-g-order (hash-varlen:tip5 pre-image))
        ?<  =(chal 0)
        =/  sig
          %+  mod
            (add nonce (mul chal sk))
          g-order:curve
        ?<  =(sig 0)
        [chal sig]
      ::
      ++  verify
        ~/  %verify
        |=  [pubkey=a-pt:curve m=noun-digest:tip5 chal=@ux sig=@ux]
        ^-  ?
        =/  m-list  (leaf-sequence:shape m)
        ?&
          (gth chal 0)  (lth chal g-order:curve)
        ::
          (gth sig 0)   (lth sig g-order:curve)
        ::
          =/  scalar
            %+  ch-add:affine:curve
              (ch-scal:affine:curve sig a-gen:curve)
            (ch-neg:affine:curve (ch-scal:affine:curve chal pubkey))
          ?<  =(scalar f6-zero)
          .=  chal
          %-  trunc-g-order
          %-  hash-varlen:tip5
          %-  zing
          :~  (f6lt-to-list x.scalar)  (f6lt-to-list y.scalar)
              (f6lt-to-list x.pubkey)  (f6lt-to-list y.pubkey)
              m-list
          ==
        ==
      ::
      ++  batch-verify
        ~/  %batch-verify
        |=  batch=(list [pubkey=a-pt:curve m=noun-digest:tip5 chal=@ux sig=@ux])
        (levy batch verify)
      --
    --
  ::
  ::  +trunc-g-order: truncates a list of ≥4 belts to a 255-bit number
  ++  trunc-g-order
    |=  a=(list belt)
    ^-  @
    %+  mod
      ;:  add
        (snag 0 a)
        (mul p (snag 1 a))
        :(mul p p (snag 2 a))
        :(mul p p p (snag 3 a))
      ==
    g-order:curve
  ::
  ::  +a-pt-to-base58: concatenate a-pt coords
  ::
  ::    we treat an a-pt as 12 64-bit atoms (6 for x, 6 for y). we concatenate them as
  ::    fixed-width atoms, put a leading 1 in front of it, and then
  ::    convert to a base58 cord.
  ::
  ::    we crash when inf=%.y since that is for projective coordinates, which does not
  ::    have a unique representation and so must be treated differently.
  ++  a-pt-to-base58
    ~/  %a-pt-to-base58
    |=  a=a-pt:curve
    ^-  cord
    ?:  inf.a  !!
    (crip (en-base58 (ser-a-pt a)))
  ::
  ++  ser-a-pt
    ~/  %ser-a-pt
    |=  a=a-pt:curve
    ^-  @ux
    ?>  &((in-g:affine:curve a) !=(a-id:curve p))
    ?:  inf.a  !!
    %+  rep  6  :: 64 bit atoms
    :~  a0.x.a  a1.x.a  a2.x.a  a3.x.a  a4.x.a  a5.x.a
        a0.y.a  a1.y.a  a2.y.a  a3.y.a  a4.y.a  a5.y.a
        1  ::  the leading 1
    ==
  ::
  ++  de-a-pt
    ~/  %de-a-pt
    |=  a=@ux
    ^-  a-pt:curve
    |^
    =/  pt-list=(list @)  (rip-correct 6 a)
    =/  x=f6lt  (conv (scag 6 pt-list))
    =/  y=f6lt  (conv (scag 6 (oust [0 6] pt-list)))
    ::
    ::  We assume the point we are provided is not projective
    ::  and set inf to %.n. This will be true so long
    ::  as `a` was encoded using ser-a-pt. This also means that a-pt
    ::  will never be the identity point, so we skip the check.
    =/  =a-pt:curve  [x y %.n]
    ?>  (in-g:affine:curve a-pt)
    a-pt
    ++  conv
      |=  n=(list @)
      ^-  f6lt
      :*  (snag 0 n)  (snag 1 n)  (snag 2 n)
          (snag 3 n)  (snag 4 n)  (snag 5 n)
      ==
    --
  ++  base58-to-a-pt
    ~/  %base58-to-a-pt
    |=  a=cord
    ^-  a-pt:curve
    (de-a-pt (de-base58 (trip a)))
    ::
  ::
  ::
  ::  +belt-schnorr: a wrapper for Schnorr signatures that works only with belts
  ::  TODO: audit this around how rip and rep are used
  ++  belt-schnorr
    |%
    +$  t8  [@ux @ux @ux @ux @ux @ux @ux @ux]  :: 8-tuple of belts
    +$  sk  t8
    +$  sig  t8
    +$  chal  t8
    ++  based
      |=  =t8
      ^-  ?
      =+  [a=@ux b=@ux c=@ux d=@ux e=@ux f=@ux g=@ux h=@ux]=t8
      ?&  (^based a)
          (^based b)
          (^based c)
          (^based d)
          (^based e)
          (^based f)
          (^based g)
          (^based h)
      ==
    ::
    ++  atom-to-t8
      |=  a=@ux
      ^-  t8
      =/  ripped=(list @)  (rip-correct 5 a)
      ::  most of the time, .ripped will be 8 @, but if it has enough leading
      ::  zeroes then it won't. +rip reverses the endianness, so we put the
      ::  leading zeroes at the end.
      =/  length-dif=@  (sub 8 (lent ripped))
      =.  ripped  (weld ripped (reap length-dif 0))
      ;;(t8 (list-to-tuple:tip5 ripped))
    ::
    ++  t8-to-atom
      |=  t=t8
      ^-  @ux
      (rap 5 (leaf-sequence:shape t))
    ::
    ++  t8-to-list
      |=  t=t8
      ^-  (list belt)
      (leaf-sequence:shape t)
    ::
    ++  affine
      |%
      ++  sign
        |=  [=sk m=noun-digest:tip5]
        ^-  [c=chal s=sig]
        =/  [c=@ux s=@ux]
          (sign:affine:schnorr (t8-to-list sk) m)
        [(atom-to-t8 c) (atom-to-t8 s)]
      ::
      ++  verify
        |=  [pk=a-pt:curve m=noun-digest:tip5 =chal =sig]
        ^-  ?
        %-  verify:affine:schnorr
        :*  pk  m
            (t8-to-atom chal)
            (t8-to-atom sig)
        ==
      ::
      ++  batch-verify
        |=  batch=(list [pk=a-pt:curve m=noun-digest:tip5 =chal =sig])
        ^-  ?
        (levy batch verify)
      ::
      --  ::+affine
    --  ::+belt-schnorr
  --  ::+cheetah
::
++  merkle  ::  /lib/merkle
  ~%  %merkle  ..merkle  ~
  |%
  +|  %types
  ::  TODO: switch merk over to this type once tip5 changes are finalized
  ++  other-merk
    |$  node
    $:  h=noun-digest:tip5
        $@  ~
        (pair (merk node) (merk node))
    ==
  ++  merk
    |$  [node]
    $~  [%leaf *noun-digest:tip5 ~]
    $%  [%leaf h=noun-digest:tip5 ~]
        [%tree h=noun-digest:tip5 t=(pair (merk node) (merk node))]
    ==
  +$  vector       (list @)         ::  replace with bitvector
  +$  merk-proof   [root=noun-digest:tip5 path=(list noun-digest:tip5)]
  +$  merk-heap    [h=noun-digest:tip5 m=mary]
  ++  mee
    |$  [node]
    $~  [%leaf *node]
    $%  [%leaf n=node]
        [%tree l=(mee node) r=(mee node)]
    ==
  ::
  +|  %work
  ++  build-merk
    ~/  %build-merk
    |=  m=mary
    ^-  (pair @ (merk mary))
    =/  [h=@ n=(mee mary)]  (list-to-balanced-tree m)
    :-  h
    |-
    ?:  ?=([%leaf *] n)
      [%leaf (hash-hashable:tip5 (hashable-mary:tip5 n.n)) ~]
    =/  l=(merk mary)  $(n l.n)
    =/  r=(merk mary)  $(n r.n)
    [%tree (hash-ten-cell:tip5 h.l h.r) l r]
  ::
  ++  build-merk-heap
    ~/  %build-merk-heap-hoon
    |=  m=mary
    (do-build-merk-heap +<)
  ::
  ++  do-build-merk-heap
    ~/  %build-merk-heap
    |=  m=mary
    ^-  [depth=@ heap=merk-heap]
    |^
    =/  heap-mary  (heapify-mary m)
    :-    (xeb len.array.m)
    [(snag-as-digest:tip5 heap-mary 0) heap-mary]
    ::
    ::  +heapify-mary
    ::  Take a mary of felts, merklize it, and return it as a heap
    ++  heapify-mary
      |=  m=mary
      ^-  mary
      =/  size  (dec (bex (xeb len.array.m)))
      =/  high-bit  (lsh [6 (mul size 5)] 1)
      ::  make leaves
      =/  res=(list (list @))
        %+  turn
          (range len.array.m)
        |=  i=@
        =/  t  (~(snag-as-fpoly ave m) i)
        (leaf-sequence:shape (hash-hashable:tip5 (hashable-fpoly:tip5 t)))
      :+  5
        size
      %+  add
        high-bit
      %+  rep  6
      %-  zing
      ^-  (list (list @))
      =/  curr  res
      |-
      ?:  =((lent curr) 1)
        res
      =/  pairs  (hash-pairs:tip5 curr)
      %=  $
        res      (weld pairs res)
        curr     pairs
      ==
    --
  ::
  ++  bp-build-merk-heap
    ~/  %bp-build-merk-heap
    |=  m=mary
    ^-  (pair @ merk-heap)
    |^
    =/  heap-mary  (heapify-mary m)
    :-    (xeb len.array.m)
    [(snag-as-digest:tip5 heap-mary 0) heap-mary]
    ::
    ::  +heapify-mary
    ::  Take a mary of belts, merklize it, and return it as a heap
    ++  heapify-mary
      |=  m=mary
      ^-  mary
      =/  size  (dec (bex (xeb len.array.m)))
      =/  high-bit  (lsh [6 (mul size 5)] 1)
      ::  make leaves
      =/  res=(list (list @))
        %+  turn
          (range len.array.m)
        |=  i=@
        =/  t  (~(snag-as-bpoly ave m) i)
        (leaf-sequence:shape (hash-hashable:tip5 (hashable-bpoly:tip5 t)))
      :+  5
        size
      %+  add
        high-bit
      %+  rep  6
      %-  zing
      ^-  (list (list @))
      =/  curr  res
      |-
      ?:  =((lent curr) 1)
        res
      =/  pairs  (hash-pairs:tip5 curr)
      %=  $
        res      (weld pairs res)
        curr     pairs
      ==
    --
  ::
  ++  index-to-axis
    ~/  %index-to-axis
    |=  [h=@ i=@]
    ^-  axis
    =/   min  (bex (dec h))
    (add min i)
  ::
  ++  list-to-balanced-merk
    ~/  %list-to-balanced-merk
    |=  lis=mary
    ^-  (pair @ (merk mary))
    (build-merk lis)
  ::
  ++  list-to-balanced-tree
    ~/  %list-to-balanced-tree
    |=  lis=mary
    ^-  [h=@ t=(mee mary)]
    :-  (xeb len.array.lis)
    |-
    ?>  !=(0 len.array.lis)
    =/  len  len.array.lis
    ?:  =(1 len)
      [%leaf (~(change-step ave [step.lis 1 (~(snag ave lis) 0)]) 3)]
    ?:  =(2 len)
      :+  %tree
        [%leaf (~(change-step ave [step.lis 1 (~(snag ave lis) 0)]) 3)]
      [%leaf (~(change-step ave [step.lis 1 (~(snag ave lis) 1)]) 3)]
    =/  l=(mee mary)
      ?:  =((mod len 2) 0)
        $(lis (~(scag ave lis) (div len 2)))
      $(lis (~(scag ave lis) +((div len 2))))
    =/  r=(mee mary)
      ?:  =((mod len 2) 0)
        $(lis (~(slag ave lis) (div len 2)))
      $(lis (~(slag ave lis) +((div len 2))))
    [%tree l r]
  ::
  ::  +prove-hashable-by-index: build proof directly over a hashable
  ++  prove-hashable-by-index
    |=  [h=hashable:tip5 idx=@]
    ^-  [axis=@ proof=merk-proof]
    ?<  =(idx 0)
    =/  res
      =+  |%
          ++  node-digest  hash-hashable:tip5
          ++  leaf-count
            |=  n=hashable:tip5
            ^-  @
            ?.  ?=(^ -.n)  1
            (add (leaf-count p.n) (leaf-count q.n))
          ++  go
            |=  [n=hashable:tip5 i=@]
            ^-  [root=noun-digest:tip5 path=(list noun-digest:tip5) axis=@]
            ?.  ?=(^ -.n)
              [(node-digest n) ~ 1]
            =/  lc=@  (leaf-count p.n)
            ?:  (lte i lc)
              =/  rec  (go [p.n i])
              =/  sib  (node-digest q.n)
              :+  (hash-ten-cell:tip5 root.rec sib)
                (weld path.rec ~[sib])
              (peg 2 axis.rec)
            =/  rec  (go [q.n (sub i lc)])
            =/  sib  (node-digest p.n)
            :+  (hash-ten-cell:tip5 sib root.rec)
              (weld path.rec ~[sib])
            (peg 3 axis.rec)
          --
      (go [h idx])
    [axis.res [root.res path.res]]
  ::
  ++  build-merk-proof
    ~/  %build-merk-proof
    |=  [merk=merk-heap axis=@]
    ^-  merk-proof
    ?:  =(0 axis)  !!
    :-  h.merk
    ?:  =(1 axis)  ~
    ::
    ::  Convert axis to heap index by decrementing
    =.  axis  (dec axis)
    ^-  (list noun-digest:tip5)
    |-
    ?:  =(0 axis)
      ~
    =/  parent  (div (dec axis) 2)
    =/  sibling
      ?:  =(1 (mod axis 2))
        (add axis 1)
      (sub axis 1)
    [(snag-as-digest:tip5 m.merk sibling) $(axis parent)]
  ::
  ++  snag-as-merk-proof
    |=  [i=@ root=noun-digest:tip5 merk=mary]
    ^-  merk-proof
    =/  mary-pat=mary  (~(snag-as-mary ave merk) i)
    =/  pat  (~(change-step ave mary-pat) 5)
    =/  merk-path=(list noun-digest:tip5)
      %+  turn  (range len.array.pat)
      |=  i=@
      (snag-as-digest:tip5 pat i)
    [root merk-path]
  ::
  +$  merk-data        [leaf=noun-digest:tip5 axis=@ p=merk-proof]
  ++  verify-merk-proof
    ~/  %verify-merk-proof
    |=  [leaf=noun-digest:tip5 axis=@ merk-proof]
    ^-  ?
    ?:  =(0 axis)  %.n
    |-
    ?:  =(1 axis)
      &(=(root leaf) ?=(~ path))
    ?~  path           %.n
    =*  sib  i.path
    ::
    ::  axis=2 when your parent is the root and you are the left child
    ?:  =(2 axis)
      &(=(root (hash-ten-cell:tip5 leaf sib)) ?=(~ t.path))
    ::
    ::  axis=3 when your parent is the root and you are the right child
    ?:  =(3 axis)
      &(=(root (hash-ten-cell:tip5 sib leaf)) ?=(~ t.path))
    ?:  =((mod axis 2) 0)
      $(axis (div axis 2), leaf (hash-ten-cell:tip5 leaf sib), path t.path)
    $(axis (div (dec axis) 2), leaf (hash-ten-cell:tip5 sib leaf), path t.path)
  ::
  --
--
