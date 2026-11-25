# Differential testing of dependent packed integer chains

This synthetic campaign found wrong GPU results even though each isolated
instruction and the constant-folding controls passed. On the RTX 4080 /
Proton setup, the previous signed clamp representation yielded 15 failing
dispatches of 256 (4,096 input programs). The DXIL of a two-instruction
reproduction expresses the expected signed min/max, but its runtime
readback disagrees. We do not assign the root cause to a specific driver
or compiler pass without independent evidence.

Represent signed saturation using unsigned order: xor the sign bit of the
32-bit result, clamp between 0x7fff8000 and 0x80007fff, then xor back.
This preserves signed ordering, and therefore the exact 16-bit saturation
bounds. The signed add, subtract and multiply-add intermediates fit i32.
The change is local to signed packed clamp lowering.

## Reproduction

Build the existing `packed_integer` example for the host and use
`tools/check_packed_chain_gpu.py --help` for the provider, Proton, DXC,
DXV, LLVM and adapter options. Run with `--chains 256` and the default
seed, then seed 8391521. A failing synthetic report retains the instruction
list, input, expected and actual buffers. Pass it to
`--replay-prefixes REPORT` to test each nonempty prefix with exactly the
original inputs. This locates the shortest observed failing prefix, not
a globally minimal instruction subsequence; compiler optimizations can
change earlier values as the program grows. The report rejects partial
readbacks as semantic mismatch evidence for prefix selection.

Only the synthetic initial register moves and final stores are
instrumented. All 384 readback words per dispatch, including unchanged
inputs and guards, are checked against the existing CPU model. The
campaign uses LLVM-assembled instructions, six ordinary VGPRs, aliasing,
mixed signed/unsigned and saturating/non-saturating chains.

## Observed results

| Trial | Programs | Compared words | Failing dispatches |
|---|---:|---:|---:|
| Before, seed 0x43504841 | 4096 | 98304 | 15 |
| After, same seed | 4096 | 98304 | 0 |
| After, seed 8391521 | 4096 | 98304 | 0 |
| After, original failing prefixes | 2112 | 50688 | 0 |

CPU constant-folding/DXV campaign: 6528/6528 programs, 39168 result words.
Python 169, shader compiler 122, DXBC native bindings 52 tests passed.
No title files or claims of retail compatibility.

The local receipts below retain tool fingerprints and aggregate outcomes.
The provider is the previously built #83 test provider; the compiler
under test is based on upstream 8acef7c plus this signed clamp change.

```json
{
  "full": {
    "scope": "Synthetic dependent packed integer chains, CPU oracle vs dynamic GPU readback",
    "adapter": "10de:2704",
    "seed": 1129334849,
    "dispatches": 256,
    "programs": 4096,
    "compared_words": 98304,
    "result_words": 24576,
    "tools_sha256": {
      "emitter": "ed7c37a313cac004b040fc1afb03a811c03f96edd3313412f4c08ec20cd4c835",
      "llvm_mc": "b665b8ded52a50be68c3f332ac547a23472080ca6bb1a40ef87e63deba4ce9ef",
      "dxc": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
      "dxv": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3",
      "provider": "146d938f05693164d101cdc327fb5286ae33ac118cec0466e79a615c743e16c1"
    },
    "failure_dispatches": 15
  },
  "bias": {
    "scope": "Synthetic dependent packed integer chains, CPU oracle vs dynamic GPU readback",
    "adapter": "10de:2704",
    "seed": 1129334849,
    "dispatches": 256,
    "programs": 4096,
    "compared_words": 98304,
    "result_words": 24576,
    "tools_sha256": {
      "emitter": "2db0e5a405dca0ae22128374bd16d0b5f5b26bf941470a5de192f90dd59d1fbd",
      "llvm_mc": "b665b8ded52a50be68c3f332ac547a23472080ca6bb1a40ef87e63deba4ce9ef",
      "dxc": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
      "dxv": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3",
      "provider": "146d938f05693164d101cdc327fb5286ae33ac118cec0466e79a615c743e16c1"
    },
    "failure_dispatches": 0
  },
  "second": {
    "scope": "Synthetic dependent packed integer chains, CPU oracle vs dynamic GPU readback",
    "adapter": "10de:2704",
    "seed": 8391521,
    "dispatches": 256,
    "programs": 4096,
    "compared_words": 98304,
    "result_words": 24576,
    "tools_sha256": {
      "emitter": "2db0e5a405dca0ae22128374bd16d0b5f5b26bf941470a5de192f90dd59d1fbd",
      "llvm_mc": "b665b8ded52a50be68c3f332ac547a23472080ca6bb1a40ef87e63deba4ce9ef",
      "dxc": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
      "dxv": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3",
      "provider": "146d938f05693164d101cdc327fb5286ae33ac118cec0466e79a615c743e16c1"
    },
    "failure_dispatches": 0
  },
  "replay-fixed": {
    "scope": "Synthetic dependent packed integer chains, CPU oracle vs dynamic GPU readback",
    "adapter": "10de:2704",
    "seed": 1129334849,
    "dispatches": 132,
    "programs": 2112,
    "compared_words": 50688,
    "result_words": 12672,
    "tools_sha256": {
      "emitter": "2db0e5a405dca0ae22128374bd16d0b5f5b26bf941470a5de192f90dd59d1fbd",
      "llvm_mc": "b665b8ded52a50be68c3f332ac547a23472080ca6bb1a40ef87e63deba4ce9ef",
      "dxc": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
      "dxv": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3",
      "provider": "146d938f05693164d101cdc327fb5286ae33ac118cec0466e79a615c743e16c1"
    },
    "failure_dispatches": 0
  },
  "cpu": {
    "scope": "CPU only: independent packed integer model vs full translator, DXC constant folding and DXV",
    "seed": 8391521,
    "programs": 6528,
    "failure_dispatches": 0
  }
}
```
