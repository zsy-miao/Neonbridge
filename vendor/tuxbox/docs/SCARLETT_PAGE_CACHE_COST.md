# Bound page-cache reclamation cost

The existing weak page cache preserves resource ownership but scans its
entire map whenever a miss occurs at capacity. If 4096 entries remain
live, every new page request pays the full scan and is not cached.
Rotate at most 16 keys per full miss; expire dead entries, retain live
entries, and replace the same key after a remap even at capacity. The
queue has one key per map entry (bounded at 4096). It does not own any
D3D resources. An expired entry behind live entries can take up to 256
full misses to reclaim; this deliberately trades immediate reclamation
of all dead metadata for bounded work.

## Local controls

Five standalone Rust tests pass, including full-cache remap replacement
and bounded eventual reclamation. Windows-target D3D12 typecheck and full
provider build pass. RTX 4080 / Proton Experimental general controls:

```
d3d12-tables PASS pci=10de:2704 words=16/16 guards=4 heaps=2 rejected=3 placed-srv=16/16 range-copies=4 descriptor-mode=1
d3d12-heaps PASS pci=10de:2704 heap=32MiB offsets=0,65536 words=256/256 invalid=4 display=3840x2160/RGB10A2 pixel=c00ffc00 volume=45/45 priorities=4/4 invalid_priority_calls=4
d3d12-placement PASS pci=10de:2704 slot=44 VA-offsets=0,4096,0 passes=2 BC1-blocks=522 cube-blocks=16404 R8-texels=262144 rejected=3 configured_vm=0
```

These do not force 4096 simultaneous live imported GPU pages, nor measure
VRAM usage or retail frame rate. The optimization does not change strong
GPU-resource ownership, so no VRAM reduction is claimed.

## Microbenchmark

Copy the baseline and current `heap_page_cache.rs` to a temporary folder
as baseline.rs and candidate.rs, and copy
`tools/reference/page_cache_bench.rs` beside them. Run
`rustc --edition 2024 -O page_cache_bench.rs -o bench && ./bench`.
Five alternating trials, 100000 misses with all 4096 fake Arc imports
held live. Median baseline 0.412239 s, candidate 0.016384 s (25.16x).
This includes fake allocation overhead but no GPU imports, and is not a
runtime speedup estimate.

```json
{
  "scope": "100000 misses with 4096 live fake Arc imports, Rust -O, no D3D allocation or GPU measurement",
  "samples_seconds": [
    [
      0.431063768,
      0.016383531
    ],
    [
      0.412239186,
      0.016259151
    ],
    [
      0.412377265,
      0.01664997
    ],
    [
      0.409754253,
      0.017349338
    ],
    [
      0.409799833,
      0.016207491
    ]
  ],
  "median_seconds": [
    0.412239186,
    0.016383531
  ],
  "speedup": 25.16180339879114
}
```
