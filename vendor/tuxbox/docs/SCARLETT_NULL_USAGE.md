# Rank recorded null pipeline usage

A successful null substitution is not evidence that it was bound or used.
Use `TUXBOX_TRACE_PRESENT=1` on an authorized test run. At each existing
present sample, and after the first null command and cumulative powers of two
from 64 onward, the provider prints a bounded recording interval:

    null usage interval recorded=6 display_bound=1 tracked=2 overflow=0 overflow_display_bound=0 compute=4 raster=2 interval=1 trigger=present_sample

This counts native draw/dispatch recording calls associated with substituted
pipelines, including indirect commands (one call, not the GPU-resolved count).
Raster calls targeting a currently bound display resource are counted separately.
Compute calls are not classified as display writes. A command can be recorded
but never submitted or executed. Zero-size commands can also be recorded.
Intervals follow sampled readback reports or recording checkpoints, not exact frame boundaries.

Up to eight local-only rank rows list original stage digests, ordered by
display-bound calls, then total calls, with deterministic ties. Distinct
live pipeline allocations with the same shader stage identities are grouped.
The interval identifier links rank rows to their summary even if log lines from different recording threads interleave.
The rank tracks at most 1,024 keys per interval; excess calls remain in
aggregate overflow counts and the rank is not exhaustive. Direct compute
dispatch shape samples remain bounded to 120 unique shader/shape pairs.

The metadata follows the guest pipeline and bindings on initial creation,
reset, ClearState, SetPipelineState and ray pipeline replacement. No native
PSO pointer registry or D3D resource retention is needed for this ranking.
When tracing is disabled no tags or rank entries are created.

For public reports share only the interval counters, the tested commit and
visible milestone; keep digest-ranked rows and raw logs local. A high count
is a prioritization signal, not proof of the cause of a black frame.


## Local prioritization and replay

Keep a single process trace and unchanged recorded request archive locally.
Run:

```sh
python tools/prioritize_null_shaders.py --log "$LOG" --requests "$REQUESTS" \
  --manifest /tmp/null-priority-private.json --aggregate /tmp/null-priority-aggregate.json
```

Only the aggregate is shareable. The manifest selects actual archive names
by observed stage identity (and exact producer/pixel pair for link requests).
It ranks display-bound observations first, then recorded calls. Counts may
associate one command with several stages; they do not attribute failure
to any one stage. Top-row truncation, overflow, unknown identities and
missing rows are reported. Duplicate interval identifiers fail closed.
If no matching request exists, only the aggregate is written and exit 2
signals that no replay is possible. Use the private manifest with
`tools/compare_scarlett_shaders.py --select` to repeat a bounded same-request
comparison. Do not upload that manifest, raw shader data or private logs.
