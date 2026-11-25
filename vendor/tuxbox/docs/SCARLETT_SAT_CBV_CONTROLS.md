# Packed saturation and descriptor negative controls

The new v_sat_pk_u8_i16 translation from #63 is independently checked in
dependent chains with an optional test campaign. Original seeds and
default generation remain unchanged. In the opt-in campaign each chain
contains one VOP1 or VOP3 saturating pack consuming a previous result;
later dependencies retain the original destination. Python models the two
signed 16-bit inputs independently, clamping each to [0,255] and clearing
the upper destination word. Edge inputs include -32768, -1, 0, 1, 255,
256 and 32767, plus random words. Prefix replay accepts the two new
unmodified forms and rejects unsupported modifiers.

Run tools/check_packed_chain_gpu.py --help with --include-saturate,
--chains 256 --seed 20261124 --optimization O0 (then O3).
On RTX 4080 each optimization mode passed 4096 programs, 98,304 guarded
words; all 256 full readback hashes were identical between modes.
No title data, arbitrary VOP3 modifier, or retail compatibility claim.

The CBV proof now has nine extra negative controls: incompatible resource
class, table length, unbounded range, physical offset, register overflow,
reflection space, overlapping reflection, wrong reflection kind and
32 possible descriptors beyond the 16-candidate cap. These tests do not
expand production descriptor acceptance.

125 shader compiler tests and 182 Python tests passed on f2184c1.

```json
{
  "O0": {
    "schema": 1,
    "scope": "Synthetic dependent packed integer chains, CPU oracle vs dynamic GPU readback",
    "passed": true,
    "mode": "generated",
    "optimization": "O0",
    "includes_saturate": true,
    "adapter": "10de:2704",
    "guest_exit": 0,
    "seed": 20261124,
    "dispatches": 256,
    "programs": 4096,
    "compared_words": 98304,
    "result_words": 24576,
    "tools_sha256": {
      "emitter": "2243ef2f0c172b1b55050f405f57d1b057b787bd3aa540655904bfee4023b95a",
      "llvm_mc": "b665b8ded52a50be68c3f332ac547a23472080ca6bb1a40ef87e63deba4ce9ef",
      "dxc": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
      "dxv": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3",
      "provider": "8e39569ded113f5b3c180b784ebce5afd5c9f7fdee42e97a26fb314171aa5c17"
    },
    "proton": "1794497090 experimental-11.0-20261112-x86_64",
    "guest_sha256": "cd298d454350738633fba7aef4cba0286f1323ad5be1491950dcbfccbe97cc08"
  },
  "O3": {
    "schema": 1,
    "scope": "Synthetic dependent packed integer chains, CPU oracle vs dynamic GPU readback",
    "passed": true,
    "mode": "generated",
    "optimization": "O3",
    "includes_saturate": true,
    "adapter": "10de:2704",
    "guest_exit": 0,
    "seed": 20261124,
    "dispatches": 256,
    "programs": 4096,
    "compared_words": 98304,
    "result_words": 24576,
    "tools_sha256": {
      "emitter": "2243ef2f0c172b1b55050f405f57d1b057b787bd3aa540655904bfee4023b95a",
      "llvm_mc": "b665b8ded52a50be68c3f332ac547a23472080ca6bb1a40ef87e63deba4ce9ef",
      "dxc": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
      "dxv": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3",
      "provider": "8e39569ded113f5b3c180b784ebce5afd5c9f7fdee42e97a26fb314171aa5c17"
    },
    "proton": "1794497090 experimental-11.0-20261112-x86_64",
    "guest_sha256": "cd298d454350738633fba7aef4cba0286f1323ad5be1491950dcbfccbe97cc08"
  }
}
```
