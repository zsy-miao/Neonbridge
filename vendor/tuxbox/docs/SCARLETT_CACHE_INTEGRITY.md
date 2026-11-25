# Runtime shader cache integrity and bounded preparation

The host runtime cache previously accepted a checksum-matching DXIL
without checking that its stage was appropriate. A valid pixel DXIL
under a compute request key was returned as a successful hit, whereas
the guest's stage gate rejects it. The host could also copy a file
after checking a different inode's size. Check regular-file snapshots,
the 64 MiB cap, exact container/program bounds, unique DXIL chunk,
portable bitcode, supported stage, and matching checksum. Infer the
expected stage from the recognizable source container, including the
ACCC-only compute shape (full extraction still belongs to xshader).
For linked/stencil requests constrain the permissible stages. Check
the emitted profile and output stage before publishing any new result.
Unknown source stage is not a proof of exact stage identity; the
normal emitter and DXV still validate cold outputs. Warm hits do not
rerun DXV. A checksum is an integrity check, not authentication.

Two independent launcher/preparation processes can request the same
cold shader under the same compiler fingerprint. Use an advisory
per-key lock only after a cache miss and recheck after acquiring it.
The lock wait is bounded (90 seconds); timeout remains retryable.
Unrelated keys never share a lock. Keep the empty lock files for the
lifetime of that fingerprint directory, avoiding inode-replacement
races. This adds one small inode per cold key; does not coordinate with
older launchers that do not acquire the locks. Cache hit replies are
the exact verified bytes; prelaunch warm hits no longer write an
unnecessary temporary DXIL copy.

Prepare recorded requests with at most twice the number of workers
in flight instead of submitting the entire archive at once. Preserve
parallel execution and completion-order progress, and keep the
existing sorted source list. This bounds Future/task allocation,
not total process memory or DXC working sets.

## Validation

239 Python tests pass, including corrupt/wrong-stage cache entries,
cold stage mismatch, FIFO/symlink/oversized input, concurrent same-key
publication, unrelated-key progress, lock release, and bounded queue.
The existing mock fixture had incorrectly returned a pixel shader for
a geometry profile; correct its stage and align the mock DXIL words.

Real compiler campaign: six independent Linux processes start at a
barrier and request the same synthetic ALU compute shader from
tools/reference/scarlett_compare_fixture.py, each with its own session
and a shared empty cache. Count actual xshader/DXC/DXV invocations
and compare returned binary digests. Baseline six compiles / 18 calls;
candidate one compile / three calls; identical binary in all sessions.
This tiny workload was slower in wall time with synchronization;
no title load-time speedup is claimed. Inject a real DXC/DXV pixel
shader with a matching checksum under the same compute request key:
baseline returns pixel stage 0, candidate repairs to compute stage 5.

A separate 20,059-task no-op scheduler test with eight workers uses
tracemalloc to compare the old eager Future list/as_completed with the
new bounded_results generator. Both return all indices exactly once
in aggregate (count and sum); 38,611,697 versus 79,629 peak traced
bytes. This is Python scheduler memory, not process RSS, GPU VRAM, or
real shader compile throughput.

Physical regression: use the synthetic slot 86 cold/warm harness from
#119 and the provider/emitter built from 465318f, importing this branch's
host runtime_shaders module. RTX 4080: 8 draws, 2048 exact guarded
pixels, 3 cold compiles, 0 warm, identical persistent cache hashes.
No retail or AMD execution, FPS, or partial-EXEC claim.

```json
{
  "compiler": {
    "baseline": {
      "compiled": 6,
      "tool_calls": 18,
      "elapsed_s": 0.0573569390035118,
      "sha256": "9d101e7d0b98b308ff9b55e34d6dec00dae349530a0b22490156522e78c5905c",
      "sessions": 6
    },
    "candidate": {
      "compiled": 1,
      "tool_calls": 3,
      "elapsed_s": 0.1367751700017834,
      "sha256": "9d101e7d0b98b308ff9b55e34d6dec00dae349530a0b22490156522e78c5905c",
      "sessions": 6
    },
    "wrong_stage": {
      "baseline_returned_stage": 0,
      "candidate_repaired_stage": 5,
      "warm_reused": true
    },
    "tools_sha256": {
      "xshader": "8cc80b0c89f14448d8722b17cb2618a4a0b3ca92cf836d908f85fe5df184af0e",
      "dxc": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
      "dxv": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3"
    }
  },
  "scheduler": {
    "baseline": {
      "tasks": 20059,
      "max_pending_futures": 20059,
      "peak_traced_bytes": 38611697,
      "elapsed_s": 0.5352433779989951
    },
    "candidate": {
      "tasks": 20059,
      "max_pending_futures": 16,
      "peak_traced_bytes": 79629,
      "elapsed_s": 0.44508371700067073
    }
  },
  "slot86": {
    "schema": 1,
    "passed": true,
    "adapter": "10de:2704",
    "tools_sha256": {
      "emitter": "8cc80b0c89f14448d8722b17cb2618a4a0b3ca92cf836d908f85fe5df184af0e",
      "provider": "8e682fef5b467cf1b2b77cad0f4bc65bdc6479faa2bd72263e6360fa18260ff8",
      "dxc": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
      "dxv": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3"
    },
    "runs": {
      "cold": {
        "draws": 4,
        "pixels": 1024,
        "compiled": 3,
        "cached_programs": 3,
        "guest_sha256": "0041cbb521d9e4625e535f09cdee3151227e229f28b5f9907c7a36cb868c3afe",
        "program_sha256": {
          "939f54dd0a56f6225f44c4895d27db083075073a783876ad56f92f1640b41380": "01f6cfbf43142655720e6d8bd52f36b398c29329018d8c58931902571d0b83b9",
          "939f54dd0a56f6225f44c4895d27db083075073a783876ad56f92f1640b41380_5c8b8f8668730b5c037ef03908ef55578f8e3834c966d8e0f6b5eefc6f4103ae_5c8b8f8668730b5c037ef03908ef55578f8e3834c966d8e0f6b5eefc6f4103ae": "70f148b0cc7b0ea73ff25e8b4d6522149ca3b5ab5b64871a4506459e90031cfd",
          "939f54dd0a56f6225f44c4895d27db083075073a783876ad56f92f1640b41380_5d1127142d06e7fb724e806b2438fd2ebb2b23243e6e8939a76381fb4841775f_5d1127142d06e7fb724e806b2438fd2ebb2b23243e6e8939a76381fb4841775f": "92d6d4873a6b723ef9bfb827eb73b7cda3bc7a87033037e2e106b11f0c8bed48"
        }
      },
      "warm": {
        "draws": 4,
        "pixels": 1024,
        "compiled": 0,
        "cached_programs": 3,
        "guest_sha256": "0041cbb521d9e4625e535f09cdee3151227e229f28b5f9907c7a36cb868c3afe",
        "program_sha256": {
          "939f54dd0a56f6225f44c4895d27db083075073a783876ad56f92f1640b41380": "01f6cfbf43142655720e6d8bd52f36b398c29329018d8c58931902571d0b83b9",
          "939f54dd0a56f6225f44c4895d27db083075073a783876ad56f92f1640b41380_5c8b8f8668730b5c037ef03908ef55578f8e3834c966d8e0f6b5eefc6f4103ae_5c8b8f8668730b5c037ef03908ef55578f8e3834c966d8e0f6b5eefc6f4103ae": "70f148b0cc7b0ea73ff25e8b4d6522149ca3b5ab5b64871a4506459e90031cfd",
          "939f54dd0a56f6225f44c4895d27db083075073a783876ad56f92f1640b41380_5d1127142d06e7fb724e806b2438fd2ebb2b23243e6e8939a76381fb4841775f_5d1127142d06e7fb724e806b2438fd2ebb2b23243e6e8939a76381fb4841775f": "92d6d4873a6b723ef9bfb827eb73b7cda3bc7a87033037e2e106b11f0c8bed48"
        }
      }
    },
    "scope": "synthetic native VS and portable PS, actual Scarlett slot 86 and worker, cold and warm processes"
  }
}
```
