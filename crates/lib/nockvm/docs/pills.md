# Making your own pill to run with Ares

> Historical note. This is a legacy pill-building workflow note and may not match current toolchains.

Ares development and testing, unlike regular development and ship operation, currently requires careful control over what pill is used to launch a ship. This document details how pills are created for the purposes of development and testing.

## Example: `baby.pill`

`baby.pill` is an extremely minimal Arvo-shaped core and Hoon standard library equipped with `%sham` jets needed to run it. `~wicdev-wisryt` [streamed a video](https://youtu.be/fOVhCx1a-9A) of its development. You can find the source Hoon for `baby.pill` in `resources/pills/src/baby/baby.hoon`, and the limited version of Hoon that it uses in `resources/pills/src/baby/cradle.hoon`. A pre-compiled `baby.pill` is already available at `resources/pills/baby.pill`. However, the steps to compile it yourself are documented below.

1. Boot a fake `zod` using an ordinary Urbit executable (not the one you created
to run Ares as serf)
2. Run `|mount %base`
3. Copy the contents of `resources/pills/src/baby/` to `/path/to/fake/zod/base/lib/`
4. Run `|commit %base`
5. Run `.baby/pill -build-file %/lib/baby/hoon`. This will make a file named `baby.pill` in `/path/to/fake/zod/.urb/put/`

You can now use this pill to boot a ship using your Vere + Ares executable:

```bash
./ares-urbit -F dev -B /path/to/fake/zod/.urb/put/baby.pill
```

If it writes `effect` to the terminal with every keystroke, you have succeeded!
