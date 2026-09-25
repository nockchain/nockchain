::  p2 divergence: rendering a subnormal float path segment differs:
::  drg_fl utils.rs:3653-3654 computes v with the incremented precision
::  (me(b, p+1)) instead of me:ff with the field width p
|%
++  main  /.1e-40
--
