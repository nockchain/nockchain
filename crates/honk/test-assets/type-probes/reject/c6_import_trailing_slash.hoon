::  c6 rejection (both compilers): an import path whose last knot is empty
::  fails hoon-138's `stap` (pipeline.rs HeaderParser::stap), so the clause
::  and then the body fail to parse before any import is resolved.
/=  foo  /lib/foo/
`*`foo
