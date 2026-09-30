::  p2: float path segments re-rendered as @ta knots: rlys/rlyd/rlyh/rlyq ->
::  sea (zero, normal, inf, nan) -> drg_fl -> drg digit loop, including the
::  power-of-two halfway tightening (hatch utils.rs ~3482-3668, 4409-4476)
|%
++  main
  :*  /.1.5/.-2.5/.0.5/.-2/.0/.-0/.inf/.-inf/.nan/.-16777215/.-4096
      /.0.1/.3.3333333e-1/.-9.999e-1/.1.17549435e-38/.-6.5e-5/.2.5e-1
      /.~0.1/.~-1e15/.~2.5e-1/.~-0/.~nan/.~inf/.~2.2250738585072014e-308
      /.~~1.5/.~~-1024/.~~0.1/.~~-inf/.~~nan/.~~6.104e-5/.~~0.5
      /.~~~2.25/.~~~0.1/.~~~-1/.~~~-0/.~~~inf/.~~~nan/.~~~0.5
  ==
--
