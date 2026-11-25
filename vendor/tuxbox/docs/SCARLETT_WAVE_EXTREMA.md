# Portable single-wave extrema

Extend the existing group-shared single-wave64 proof from unsigned add
to unsigned and floating min/max. Require the direct saved-EXEC predicate,
the exact DPP reduction shape and restore; include the floating identity
move in the protected range. Extra masks and escaping saved registers
still refuse this path. All existing single-group and control-flow
restrictions remain.

Build the host wave_extrema example and see tools/check_wave_extrema.py
--help. LLVM-assembled native opcodes enter the actual translator.
The GPU harness injects values and masks and stores the final scalar.
Four operations x sixteen masks x two widths: 128 dispatches, 49,152
guarded words, 6,144 result words, zero mismatches on RTX 4080.
Float controls cover finite values and infinities, not NaNs, signed-zero
ties or subnormal policy. No arbitrary divergence or retail claim.
The retest uses the compiler based on f2184c1 and provider aa71f753.
125 shader compiler tests and 183 Python tests passed.

```json
{
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
    "emitter_sha256": "83109dc61d0c88a4389939f4e40d273df0ec35946a5182581c9c08847022141d",
    "dxc_sha256": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
    "dxv_sha256": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3",
    "provider_sha256": "8e39569ded113f5b3c180b784ebce5afd5c9f7fdee42e97a26fb314171aa5c17",
    "guest_sha256": "cd298d454350738633fba7aef4cba0286f1323ad5be1491950dcbfccbe97cc08",
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
    "emitter_sha256": "83109dc61d0c88a4389939f4e40d273df0ec35946a5182581c9c08847022141d",
    "dxc_sha256": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
    "dxv_sha256": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3",
    "provider_sha256": "8e39569ded113f5b3c180b784ebce5afd5c9f7fdee42e97a26fb314171aa5c17",
    "guest_sha256": "cd298d454350738633fba7aef4cba0286f1323ad5be1491950dcbfccbe97cc08",
    "shaders": {
      "umin": "d05d4019e19047f95a565273ee04fa4c094ea77d58af5392c4a8a376a0650748",
      "umax": "ad0b350c20d899e4b04add735ded53ee8102f00ac4aab810bda23fcaa24e5d38",
      "fmin": "a6e0eab7ac913b24e42633ddae4968349b72414863393ff608117da3133cf88c",
      "fmax": "9bf5814ea3e7fb442e6cc8a8f6bd59014608c196bbfa930d6c9f93dea21e350b"
    }
  }
}
```
