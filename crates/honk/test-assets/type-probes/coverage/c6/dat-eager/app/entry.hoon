::  c6 (was divergence 29): hoonc kicks a /dat node's trap while building it
::  and keeps the value in an eval-vase trap (hoonc.hoon ++compile, eval.nod);
::  honk does the same for every file keyed under /dat (honk.rs
::  compile_path_uncached, is_hoonc_dat_node).
/#  const-data
`*`const-data
