# Indexed relay flight bookkeeping

The pending-flight counter used a linear vector lookup for each exact
range declaration and completion. Replace it with a BTreeMap keyed by
the same pair of offsets. Lookups are logarithmic instead of linear.
No range coalescing, changed fence order, skipped uploads, reduced
texture fidelity, or changed copyback semantics.

The existing flight regression remains; a new Rust differential test
compares the indexed implementation to the old vector on 65,536
operations. An executed JavaScript reference model (Map, not Rust's
BTreeMap) agreed on 131072 operations, 196552 range queries, and
all released ranges and counts. The old model performed 52449991
linear comparisons; maximum live entries: 519. These counts
are not timings or an FPS benchmark.

Small tables may have worse allocation/cache locality with the tree.
Native Rust tests and matched game A/B are pending. Measure relay CPU
preparation/copyback time, frame times and identical visual output
before adopting. No shader or texture policy changes. AI-assisted.
