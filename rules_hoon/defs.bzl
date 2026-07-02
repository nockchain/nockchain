""" Hoon rules for Bazel. """

load("//:rules_hoon/hoon.bzl", _honk_jam = "honk_jam", _honk_library = "honk_library", _hoon_jam = "hoon_jam", _hoon_library = "hoon_library")

hoon_library = _hoon_library
hoon_jam = _hoon_jam
honk_library = _honk_library
honk_jam = _honk_jam
