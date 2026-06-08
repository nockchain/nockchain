/=  *  /common/zeke
::  asert: aserti3-2d difficulty adjustment
::
::    implements the algorithm formalized by jonathan toomim and shipped on
::    bitcoin cash in 2020. pure math, no consensus-state coupling; operates
::    on atoms with bignum conversion at the +compute-target-bn boundary.
::
::    the 2^x approximation is a 3rd-order polynomial whose coefficients are
::    tied to rbits=16. changing rbits requires new polynomial coefficients,
::    so rbits is not a parameter.
::
|%
++  asert-rbits  16
++  asert-radix  ^~((bex asert-rbits))
::
::  +poly-factor: 2^(frac/radix) * radix as an integer
::
::    input frac in [0, radix); output in [radix, 2*radix).
::    3rd-order minimax polynomial approximation of 2^x on [0, 1),
::    scaled so that evaluating at frac = y*radix yields factor ≈ 2^y * radix:
::      radix + (195_766_423_245_049*frac
::               + 971_821_376       *frac^2
::               + 5_127             *frac^3
::               + 2^47) >> 48
::    the three normalized coefficients (c_k / 2^((k+1)*16)) sum to ~1 so the
::    approximation satisfies p(0) = 1 and p(1) ≈ 2. the `+ 2^47` term is
::    round-to-nearest for the fixed-point divide by 2^48, matching bch's
::    canonical aserti3-2d so published polynomial vectors reproduce exactly.
::    max error versus 2^x is under 0.13% on [0, 1).
++  poly-factor
  |=  frac=@
  ^-  @
  ::  precondition: frac < radix (see +decompose-exponent). guard here so
  ::  any future caller that constructs frac independently fails loudly
  ::  rather than silently producing a factor outside [radix, 2*radix).
  ?>  (lth frac asert-radix)
  =/  f2  (mul frac frac)
  =/  f3  (mul f2 frac)
  =/  num  :(add (mul 195.766.423.245.049 frac) (mul 971.821.376 f2) (mul 5.127 f3))
  (add asert-radix (rsh [0 48] (add num (bex 47))))
::
::  +compute-exponent: (time-diff - ideal*(blocks-since-anchor - 1)) * radix / half-life
::
::    returns sign-magnitude. sign %.y means non-negative, %.n means negative.
::    time-diff is given as sign-magnitude; blocks-since-anchor is unsigned
::    and must be >= 1 (caller guarantees current-height > anchor-height).
::    the (blocks-since-anchor - 1) factor is PDF Eq. (2) under the anchor-
::    own-ts convention (PDF §1.3 Option 2): time-diff spans parent-time -
::    anchor-time, i.e. (parent.height - anchor.height) = (blocks-since - 1)
::    ideal intervals under perfect schedule.
++  compute-exponent
  |=  $:  time-diff-sign=?
          time-diff-mag=@
          blocks-since-anchor=@
          ideal=@
          half-life=@
      ==
  ^-  [sign=? mag=@]
  =/  ideal-total  (mul ideal (dec blocks-since-anchor))
  ?:  time-diff-sign
    ?:  (gte time-diff-mag ideal-total)
      =/  delta  (sub time-diff-mag ideal-total)
      [%.y (div (mul delta asert-radix) half-life)]
    =/  delta  (sub ideal-total time-diff-mag)
    [%.n (div (mul delta asert-radix) half-life)]
  =/  delta  (add time-diff-mag ideal-total)
  [%.n (div (mul delta asert-radix) half-life)]
::
::  +decompose-exponent: split signed exponent into (integer shift, fractional)
::
::    mirrors arithmetic right shift semantics:
::      positive x: shifts = x >> rbits, frac = x mod radix (both in [0, ...))
::      negative x: shifts = floor(x / radix) (toward -inf),
::                  frac = x - shifts*radix, always in [0, radix)
::    so for x = -5 with radix=4: shifts = -2, frac = 3.
++  decompose-exponent
  |=  [exp-sign=? exp-mag=@]
  ^-  [shifts-sign=? shifts-mag=@ frac=@]
  =/  rem-mag  (mod exp-mag asert-radix)
  =/  quo-mag  (rsh [0 asert-rbits] exp-mag)
  ?:  exp-sign
    [%.y quo-mag rem-mag]
  ?:  =(0 rem-mag)
    [%.n quo-mag 0]
  [%.n +(quo-mag) (sub asert-radix rem-mag)]
::
::  +compute-target: aserti3-2d target for a block given the anchor
::
::    current-height must be > anchor-height (the caller is computing the
::    target for a strict descendant of the anchor). anchor-min-timestamp
::    is the anchor block's OWN median-of-11 (PDF §1.3 Option 2);
::    current-min-timestamp is the parent block's median-of-11 — so
::    time-diff = parent.min-ts - anchor.min-ts spans (blocks-since - 1)
::    ideal intervals under a perfect schedule. timestamps in seconds.
::    anchor-target is an atom; callers on bignum can use +compute-target-bn.
::    result is clamped to [1, max-target-atom].
++  compute-target
  |=  $:  anchor-target=@
          anchor-min-timestamp=@
          anchor-height=@
          current-min-timestamp=@
          current-height=@
          ideal-block-time=@
          half-life=@
          max-target-atom=@
      ==
  ^-  @
  ?<  (lte current-height anchor-height)
  ::  anchor-target = 0 would silently produce 0 * factor = 0, which then
  ::  clamps to 1 below — effectively freezing the chain. reject the
  ::  misconfiguration at the boundary rather than absorbing it.
  ?<  =(0 anchor-target)
  =/  time-diff-sign=?  (gte current-min-timestamp anchor-min-timestamp)
  =/  time-diff-mag=@
    ?:  time-diff-sign
      (sub current-min-timestamp anchor-min-timestamp)
    (sub anchor-min-timestamp current-min-timestamp)
  =/  blocks-since-anchor  (sub current-height anchor-height)
  =/  exp
    %-  compute-exponent
    :*  time-diff-sign
        time-diff-mag
        blocks-since-anchor
        ideal-block-time
        half-life
    ==
  =/  dec  (decompose-exponent sign.exp mag.exp)
  =/  factor  (poly-factor frac.dec)
  =/  unshifted  (mul anchor-target factor)
  =/  max-bits  (met 0 max-target-atom)
  ::
  ::  cap shift magnitude so intermediate noun stays bounded. positive
  ::  shifts beyond max-bits+rbits+2 are guaranteed to saturate at
  ::  max-target-atom; negative shifts beyond unshifted's bit-length
  ::  zero the result, which then clamps to 1.
  =/  result=@
    ?:  shifts-sign.dec
      =/  cap  (add max-bits (add asert-rbits 2))
      =/  eff  ?:((gth shifts-mag.dec cap) cap shifts-mag.dec)
      (rsh [0 asert-rbits] (lsh [0 eff] unshifted))
    =/  unshifted-bits  (met 0 unshifted)
    ?:  (gte shifts-mag.dec unshifted-bits)  0
    (rsh [0 asert-rbits] (rsh [0 shifts-mag.dec] unshifted))
  ?:  =(0 result)  1
  ?:  (gth result max-target-atom)  max-target-atom
  result
::
::  +compute-target-bn: thin bignum wrapper over +compute-target
++  compute-target-bn
  |=  $:  anchor-target=bignum:bignum
          anchor-min-timestamp=@
          anchor-height=@
          current-min-timestamp=@
          current-height=@
          ideal-block-time=@
          half-life=@
          max-target-atom=@
      ==
  ^-  bignum:bignum
  %-  chunk:bignum
  %-  compute-target
  :*  (merge:bignum anchor-target)
      anchor-min-timestamp
      anchor-height
      current-min-timestamp
      current-height
      ideal-block-time
      half-life
      max-target-atom
  ==
--
