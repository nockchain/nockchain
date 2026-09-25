::  c6 divergent (MISMATCH, deliberate): the entry sits under a different
::  open/hoon root than the dependency root. honk matches the roots by their
::  open|closed/hoon marker and keys the entry as /app/kernel.hoon (honk.rs
::  entry_path_for_hoon ~4720, build_import_wer ~922, matching_hoon_root_marker
::  ~895); hoonc keys it by its canonical absolute path.
/=  raw  /common/raw-thing
|=  hash=@uvI
^-  *
[hash raw]
