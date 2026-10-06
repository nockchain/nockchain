::  p3: path segments for @rs/@rd/@rh/@rq (r_co exponent branches, ed_co
::  positive/negative, sign, inf, nan)
|%
++  main
  |=  a=@
  :*  !>(/.1.5)
      !>(/.-1.5)
      !>(/.0.001)
      !>(/.0.01)
      !>(/.0.1)
      !>(/.~0.5)
      !>(/.~123.456e2)
      !>(/.1e-10)
      !>(/.123.456)
      !>(/.~1.5)
      !>(/.~-2.25e5)
      !>(/.~~1.5)
      !>(/.~~~1.5)
      !>(/.inf)
      !>(/.-inf)
      !>(/.nan)
  ==
--
