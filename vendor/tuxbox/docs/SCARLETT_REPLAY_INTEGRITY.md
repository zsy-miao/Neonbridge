# Same-input private compiler comparisons

The paired emitter and full compiler tools now copy and verify each request
once before running both compilers. Ordinary/stencil requests and all three
linked inputs use the runtime's digest and size checks. Invalid input fails
both sides without invoking either compiler. The per-pair temporary snapshot
is removed on completion; no title archive is exported, no original cache is
modified, no persistent copy is created. A live archive may gain new files:
the report describes only the initial enumeration. Each active pair stages
its own request, rather than copying the entire corpus.

Both emitters are copied and hashed before execution. Full comparison also
checks DXC/DXV executable hashes before and after; dynamically loaded libraries
and changes that revert between checks are not covered. Use a trusted stable
toolchain. Neither comparison is a security sandbox for untrusted executables.

Example (replace paths with local authorized inputs):

```sh
python tools/compare_scarlett_shaders.py "$REQUESTS" --baseline "$OLD" \
  --candidate "$NEW" --dxc "$DXC" --dxv "$DXV" --jobs 4 \
  --aggregate-only --json /tmp/scarlett-paired.json
```

Share only the aggregate JSON. It contains tool hashes, fixed failure/refusal
categories and transition counts, not request names, raw diagnostics, private
paths or output digests. Detailed output stays local; it is needed for
worklists. --require-no-failures gates inconclusive results;
--require-identical-dxil optionally gates changed successful bytecode.
Different bytecode alone is not evidence of a rendering error. Candidate
refusals or failures after baseline success always fail the gate.

The emitter-only paired tool has the same snapshot checks. Both tools require
fresh output paths outside the archive. At most eight concurrent full
comparisons are allowed; the emitter-only CLI retains its existing job limit.

Null usage selection now checks the per-kind command budgets, display-bound
counts, rank completeness for intervals with at most eight tracked keys,
trigger agreement, u64 bounds and compute/raster stage identity. Incomplete
and orphan ranks, malformed lines and missing interval lower bounds are
reported rather than assumed complete. The ranking remains an association
with recorded commands, not a proven render dependency.

Synthetic validation: 9 requests (including one deliberately malformed
container), 4 -> 5 validated DXIL outputs, 1 recovery, 0 regressions, 1
inconclusive. 96 generated programs paired against the same integrated
compiler, three runs each at 1/2/4 workers: 1728 validated outputs, exact
private rows identical. Median 2.9332 / 1.5551 / 0.9698 seconds.
These are measurements of the comparison tool, not title performance.

```json
{
  "paired": {
    "scope": "CPU-only production compiler pipeline; no rendering or retail gameplay claim",
    "tool_sha256": {
      "baseline": "12268d084ec604a5827cf1b667c50221023946a121b5b180ceddc87045d294f2",
      "candidate": "1e226d47d76cd23fc278800574b859474345fcbae18a1a25d9794daf70d38925",
      "dxc": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
      "dxv": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3"
    },
    "requests": 9,
    "before": {
      "compiled": 4,
      "refused": 4,
      "failed": 1
    },
    "after": {
      "compiled": 5,
      "refused": 3,
      "failed": 1
    },
    "recovered": 1,
    "regressed": 0,
    "candidate_failures_after_success": 0,
    "inconclusive": 1,
    "transitions": {
      "compiled": {
        "compiled": 4,
        "refused": 0,
        "failed": 0
      },
      "refused": {
        "compiled": 1,
        "refused": 3,
        "failed": 0
      },
      "failed": {
        "compiled": 0,
        "refused": 0,
        "failed": 1
      }
    },
    "changed_compiled_outputs": 0,
    "aggregate_only": true,
    "schema": 1,
    "buckets": {
      "before": {
        "refused": {
          "mubuf": 1,
          "other_refusal": 1,
          "wave_divergent": 2
        },
        "failed": {
          "xshader_failure": 1
        }
      },
      "after": {
        "refused": {
          "mubuf": 1,
          "wave_divergent": 2
        },
        "failed": {
          "xshader_failure": 1
        }
      }
    },
    "baseline_refusal_buckets": {
      "mubuf": {
        "compiled": 0,
        "refused": 1,
        "failed": 0
      },
      "other_refusal": {
        "compiled": 1,
        "refused": 0,
        "failed": 0
      },
      "wave_divergent": {
        "compiled": 0,
        "refused": 2,
        "failed": 0
      }
    }
  },
  "emitter_only": {
    "schema": 1,
    "aggregate_only": true,
    "scope": "Digest-verified per-request snapshots; emitter only, not DXC, GPU or gameplay.",
    "tool_sha256": {
      "baseline": "12268d084ec604a5827cf1b667c50221023946a121b5b180ceddc87045d294f2",
      "candidate": "1e226d47d76cd23fc278800574b859474345fcbae18a1a25d9794daf70d38925"
    },
    "failure_buckets": {
      "baseline": {
        "emitter_failure": 1
      },
      "candidate": {
        "emitter_failure": 1
      }
    },
    "requests": 9,
    "totals": {
      "baseline": {
        "translated": 4,
        "refused": 4,
        "failed": 1
      },
      "candidate": {
        "translated": 5,
        "refused": 3,
        "failed": 1
      }
    },
    "transitions": [
      {
        "baseline": "failed",
        "candidate": "failed",
        "count": 1
      },
      {
        "baseline": "refused",
        "candidate": "refused",
        "count": 3
      },
      {
        "baseline": "refused",
        "candidate": "translated",
        "count": 1
      },
      {
        "baseline": "translated",
        "candidate": "translated",
        "count": 4
      }
    ],
    "baseline_refusal_buckets": {
      "mubuf": {
        "translated": 0,
        "refused": 1,
        "failed": 0
      },
      "other_refusal": {
        "translated": 1,
        "refused": 0,
        "failed": 0
      },
      "wave_divergent": {
        "translated": 0,
        "refused": 2,
        "failed": 0
      }
    },
    "recovered": 1,
    "regressed": 0,
    "inconclusive": 1
  },
  "benchmark": {
    "scope": "96 synthetic requests paired against the same compiler; production HLSL/DXC/DXV, private snapshots, 3 alternating repetitions; not retail speed",
    "requests_per_pair_run": 96,
    "validated_outputs": 1728,
    "all_private_rows_identical": true,
    "times_seconds": {
      "1": [
        2.9066,
        2.9332,
        3.0972
      ],
      "2": [
        1.5491,
        1.5551,
        1.6024
      ],
      "4": [
        0.9698,
        0.9736,
        0.9649
      ]
    },
    "median_seconds": {
      "1": 2.9332,
      "2": 1.5551,
      "4": 0.9698
    },
    "speedup_vs_one": {
      "1": 1.0,
      "2": 1.886,
      "4": 3.025
    },
    "tool_sha256": {
      "baseline": "1e226d47d76cd23fc278800574b859474345fcbae18a1a25d9794daf70d38925",
      "candidate": "1e226d47d76cd23fc278800574b859474345fcbae18a1a25d9794daf70d38925",
      "dxc": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
      "dxv": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3"
    }
  }
}
```
