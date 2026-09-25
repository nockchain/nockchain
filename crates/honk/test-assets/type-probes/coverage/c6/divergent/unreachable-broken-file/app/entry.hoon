::  c6 divergent (HONK-ONLY): hoonc parses every source file in the
::  dependency tree (hoonc.hoon make-node over all files) and fails on
::  junk/bad.hoon although nothing imports it; honk compiles only the entry's
::  import closure (honk.rs compile_entry / compile_path).
/=  raw  /common/raw
`*`raw
