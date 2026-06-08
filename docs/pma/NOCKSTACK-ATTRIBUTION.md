# PMA / NockStack / Heap Attribution Plan

Below is a targeted, actionable plan to bucket memory into **NockStack**, **PMA**, and **heap/other anon**. This is designed to work in Docker or on host, and gives you both a point‑in‑time snapshot and a time‑series view.

**Plan Overview**
- **PMA**: file‑backed mappings under `.../pma/*.mmap` → derive resident % from `smaps` or `mincore`.
- **NockStack**: the single anonymous `mmap` sized to `NOCK_STACK_SIZE * 8` bytes → find by size + anon mapping.
- **Heap/other anon**: `[heap]` + remaining anonymous mappings (excluding NockStack and thread stacks).

---

## 1) Get the exact NockStack size (bytes)
Find the stack size used by your runtime (e.g. `StackSize::Normal`), then compute bytes:

- Find the size constant:
  - `rg -n "StackSize|NOCK_STACK_SIZE" crates/nockapp/src/kernel/boot.rs`
- Compute:
  - `stack_bytes = stack_words * 8`

This value is what you’ll match against `smaps` mappings.

---

## 2) Collect `smaps` inside the container
Inside the container you usually can read `/proc/1/smaps` without host root.

```
docker exec -it <container> sh -c 'cat /proc/1/smaps' > /tmp/smaps.txt
```

---

## 3) Bucket the mappings (PMA / Stack / Heap)
Use `smaps` to sum RSS and Size into buckets. Heuristic rules:

- **PMA**: pathname contains `/pma/` and ends with `.mmap`
- **NockStack**: anonymous mapping (no pathname), `rw-p`, `Size ~= stack_bytes`  
  (allow a small tolerance, e.g. ±1–2 pages)
- **Thread stacks**: `[stack]` or `[stack:<tid>]` → ignore or report separately
- **Heap/other anon**: `[heap]` + remaining anonymous `rw-p` that aren’t the stack mapping

If you want a quick oneliner to sanity check:
```
grep -n "pma/.*\\.mmap" -A20 /tmp/smaps.txt
```

---

## 4) PMA residency ratio (percent in RAM)
Two options:

**A) From smaps**  
For PMA mappings: `rss_ratio = Rss / Size`.

**B) From mincore/vmtouch**  
If you can install it in the container:
```
vmtouch -v /data/.data.nockchain/pma/*.mmap
```
This gives a direct resident‑pages %.

---

## 5) Time‑series: sample every N seconds
Sample buckets every 5–10 seconds during sync and again after sync slows:

- Phase A: startup / initial sync
- Phase B: mid‑sync
- Phase C: post‑sync idle

This will show:
- **PMA “hotness”** (resident ratio rising/falling)
- **NockStack high‑water** (stack RSS plateau)
- **Heap/other anon** growth (allocator churn, slabs, jam buffers)

---

## 6) Optional: control checkpoint noise
Even though you said checkpoints are “transparent enough,” for clean baselines:
- Run once with save interval disabled: `--save-interval 0`
- Run once with normal saves enabled  
Compare heap/anon spikes to see checkpoint amplification.

---

## What this tells you
- If **PMA** is doing what we want: PMA mapping size grows, but **resident % should drop** when things go cold or under pressure.
- If **NockStack** is the issue: you’ll see a single large anon map with RSS near its size and never dropping.
- If **heap/other anon** is the issue: that bucket grows (NounSlab allocations, jam buffers, libp2p, etc.).

If you want, I can wire this into the existing `compare_pma_mem.rs` to output all three buckets (plus PMA residency %) on each run. That would make the time‑series trivial.
