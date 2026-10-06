::  c6 rejection (both compilers): a /= after a /# is out of order, so
::  hoonc parses it as a body path followed by an unbound `x`.
/#  thing
/=  x  /common/x
`*`42
