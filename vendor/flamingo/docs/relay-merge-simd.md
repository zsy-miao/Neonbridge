# Relay merge SIMD

The relay's CPU merge processes the GPU copy, the previous reference,
and current title memory. Only bytes changed by the GPU replace title
bytes. The reference takes the GPU bytes, not concurrent CPU-only writes.
This differs from the older upstream synchronous merge, so its patch
cannot be transplanted verbatim.

The AVX2 path compares and selects 32 bytes at once, updates the
reference and reduces the changed-title flag. Runtime feature detection
selects the vector path; short inputs and tails use the scalar path.
The original common-prefix behavior for unequal slice lengths is retained.
No changes to flight accounting, fences, resource lifetimes, or GPU commands.

A separately compiled C equivalent was tested on an AVX2-capable CPU:
16,777,216 byte triples and 4,416 length/alignment cases, including
guard bytes outside the slices, with no disagreements. This verifies
the algorithm, not Rust integration. The compiler may already vectorize
the original loop. No measured speedup is claimed.

Run:
    cargo test -p victory-gpu shadow::relay_merge_simd --locked
    cargo test -p victory-gpu shadow::relay_tests --locked

Compare matched warm-cache release runs with identical GPU settings
and relay configuration before attributing any frame-time improvement.
