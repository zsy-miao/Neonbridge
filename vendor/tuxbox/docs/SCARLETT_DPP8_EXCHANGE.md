# Exact straight-line DPP8 exchange after #101

The lone-lane compute fallback accepts a DPP8 unsigned maximum followed
by a constant readlane, but replaces the exchange with a local copy and
readlane with readfirstlane. A minimal synthetic 32-thread program
(maximum of adjacent lane indices, then readlane 0) emitted legal HLSL
and validated DXIL, yet returned lane indices 0..31 instead of 32
copies of 1. NVIDIA RTX 4080 readback: 31/32 wrong, then 0/32 with the
exact exchange lowering. This original probe instrumented the scalar
output with a UAV store; it was not a title shader.

Keep the real DPP8 selector (eight 3-bit lanes) and both operands.
Use a two-barrier group-shared read, independent of host subgroup width.
The accepted subset is a straight-line 32- or 64-thread X group with
initial full participation, ordinary operands, unsigned add/AND/OR/min/max,
DPP8 selector encoding 233 and no EXEC change, branch, LDS or early exit.
Constant readlane 0..31 is permitted only in 32-thread groups, where
wave32 and wave64 agree for the active lanes. Static descriptor loads
and a single terminal buffer store are supported through the existing
resource/address resolver and require root metadata. Unknown encodings
are not admitted by this proof. A recognized exact subset that later
fails binding/lowering cannot fall back to lone-lane copies. Other
previous lone-lane approximations are unchanged.

## Native GPU campaign

Build the lone_lane_probe host example. See tools/check_dpp8_exchange.py
--help. Five operations, four selector patterns, distinct operands,
32 and 64 threads, unsigned boundary and overflow values, plus lane-31
broadcast for 32 threads. LLVM-verified native encodings, actual
translator, real reflected raw UAV and native terminal store; no HLSL
output injection in the 60-case campaign. Check all 384 words per
dispatch including untouched guards; fallback disabled. Two builds
pass the same 60 cases. Existing wave32/64 min/max controls also pass
128 dispatches on the final compiler. The final GPU harness uses
desktop D3D12 through Proton with the newly built provider staged;
this is not a retail guest pipeline replay.

213 Python, 146 shader compiler unit/integration and 53 DXBC tests pass.
Full provider build succeeds. No arbitrary divergent control flow,
AMD run, title shader acceptance percentage or FPS claim.

```json
{
  "minimal_before": {
    "status": 0,
    "adapter": "10de:2704",
    "expected": [
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1
    ],
    "actual": [
      0,
      1,
      2,
      3,
      4,
      5,
      6,
      7,
      8,
      9,
      10,
      11,
      12,
      13,
      14,
      15,
      16,
      17,
      18,
      19,
      20,
      21,
      22,
      23,
      24,
      25,
      26,
      27,
      28,
      29,
      30,
      31
    ],
    "mismatches": 31,
    "scope": "synthetic native DPP8 adjacent maximum and lane-zero read, observed scalar instrumented to UAV"
  },
  "minimal_after": {
    "status": 0,
    "adapter": "10de:2704",
    "expected": [
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1
    ],
    "actual": [
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1,
      1
    ],
    "mismatches": 0,
    "scope": "synthetic native DPP8 adjacent maximum and lane-zero read, observed scalar instrumented to UAV"
  },
  "campaign": {
    "schema": 1,
    "scope": "Native synthetic instructions, actual emitted HLSL and reflected raw UAV store; no output instrumentation",
    "adapter": "10de:2704",
    "guest_exit": 0,
    "passed": true,
    "cases": 60,
    "compared_words": 23040,
    "result_words": 2560,
    "tools_sha256": {
      "emitter": "b8ca6973a6841da61285285bbd2390bd4149ae68f36ad9cdc1586fb38300f6e5",
      "provider": "fc1bf7e2a44361d4e0ee624315f404a436f5c414660a637cbf959ee26bf02c70",
      "dxc": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
      "dxv": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3"
    },
    "guest_sha256": "a1d529d66d6c8857495339fee08bbcdcdc67859c76f6dda40a0617c342337220"
  },
  "extrema": {
    "32": {
      "schema": 1,
      "scope": "Native masked wave extrema translated to HLSL; finite values and infinities; no NaN or signed-zero tie claims",
      "passed": true,
      "adapter": "10de:2704",
      "width": 32,
      "guest_exit": 0,
      "cases": 64,
      "compared_words": 24576,
      "result_words": 2048,
      "emitter_sha256": "588f54bbd81e584e3c10f96a3edfec3bc3c9449ccdb8481d034c64d78afd62ec",
      "dxc_sha256": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
      "dxv_sha256": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3",
      "provider_sha256": "fc1bf7e2a44361d4e0ee624315f404a436f5c414660a637cbf959ee26bf02c70",
      "guest_sha256": "a1d529d66d6c8857495339fee08bbcdcdc67859c76f6dda40a0617c342337220",
      "shaders": {
        "umin": "661e0b82415534105f9618b137cdcb826088e75cc3f110007aafd631bdc9b685",
        "umax": "ce6b9f9733d5d9c407a20c43025b1b8b19caf7c5ecce555704a0e49cfd6738c6",
        "fmin": "20bdd9ac8cbca038fdf0b61a85c3a833b6baff881e5a935053ef532c3dfc326a",
        "fmax": "2a4f6a1a99da11f2b9d23e5567806a3170ce15cc82d46ab7c2871d0349c51c3b"
      }
    },
    "64": {
      "schema": 1,
      "scope": "Native masked wave extrema translated to HLSL; finite values and infinities; no NaN or signed-zero tie claims",
      "passed": true,
      "adapter": "10de:2704",
      "width": 64,
      "guest_exit": 0,
      "cases": 64,
      "compared_words": 24576,
      "result_words": 4096,
      "emitter_sha256": "588f54bbd81e584e3c10f96a3edfec3bc3c9449ccdb8481d034c64d78afd62ec",
      "dxc_sha256": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
      "dxv_sha256": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3",
      "provider_sha256": "fc1bf7e2a44361d4e0ee624315f404a436f5c414660a637cbf959ee26bf02c70",
      "guest_sha256": "a1d529d66d6c8857495339fee08bbcdcdc67859c76f6dda40a0617c342337220",
      "shaders": {
        "umin": "d05d4019e19047f95a565273ee04fa4c094ea77d58af5392c4a8a376a0650748",
        "umax": "ad0b350c20d899e4b04add735ded53ee8102f00ac4aab810bda23fcaa24e5d38",
        "fmin": "a6e0eab7ac913b24e42633ddae4968349b72414863393ff608117da3133cf88c",
        "fmax": "9bf5814ea3e7fb442e6cc8a8f6bd59014608c196bbfa930d6c9f93dea21e350b"
      }
    }
  }
}
```


## Signed extrema, XOR and FI variants

Extend the exact full-participation DPP8 subset to signed min/max
and XOR, and the alternate DPP8 FI=1 encoding. Signed comparisons
reinterpret operands as int32 and return the original 32-bit result.
FI does not change the result when all source lanes in the octet
participate, as required by the existing straight-line proof.
No branch, partial EXEC or additional memory side effects are admitted.
This does not solve divergent collectives or establish retail coverage.

The extended native GPU campaign checks eight operations, four
selector patterns, three group/readlane combinations and both FI
encodings (192 dispatches). LLVM gfx1030 encodings verified, actual
emitted HLSL compiled/validated and native terminal UAV store exercised.
RTX 4080: 73,728 guarded words, all exact. 164 shader compiler and
232 Python tests pass. The staged provider is from 465318f; compiler
source base dbc357b. No AMD or retail run.

```json
{
  "schema": 1,
  "scope": "Native synthetic instructions, actual emitted HLSL and reflected raw UAV store; no output instrumentation",
  "adapter": "10de:2704",
  "guest_exit": 0,
  "passed": true,
  "cases": 192,
  "compared_words": 73728,
  "result_words": 8192,
  "tools_sha256": {
    "emitter": "aea4ccb8a2f752eec6eee26dddea6ed69c401c4def21b6af85f7f7fb639f00ff",
    "provider": "8e682fef5b467cf1b2b77cad0f4bc65bdc6479faa2bd72263e6360fa18260ff8",
    "dxc": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
    "dxv": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3"
  },
  "guest_sha256": "a1d529d66d6c8857495339fee08bbcdcdc67859c76f6dda40a0617c342337220"
}
```
