# Correlated heap descriptor provenance through direct control flow

PR #108 validated descriptor origins only in straight-line programs.
Two native counterexamples on ade292d showed that adding a direct
branch, or mixing a descriptor word on only one predecessor, bypassed
that proof. Use must-alias equivalence classes for dynamic heap words
through the existing direct CFG. Each load has a unique local site
identity. At edges, rename classes in a disjoint destination-block
namespace; at joins intersect the equivalence relations (group by the
pair of origins, not the union of independent possible origins).
Canonical minimum register IDs keep the fixpoint deterministic.
A new load in a loop is distinct from any class carried into the block.
Different complete descriptors can therefore join, while mixing their
words is not accepted by this proof.

Also preserve heap offset alignment under AND with a constant, or
XOR/OR with a constant whose low alignment bits are zero. Do not
infer bounds. Unsupported/indirect control transfers retain the old
policy; the separate specialized descriptor paths and arbitrary EXEC
behavior are not claimed verified. Static descriptor optimistic joins
are unchanged.

Native tests: mixed words after an unconditional branch, mixed words
on only one predecessor, valid complete alternatives, old-generation
word graft after a loop reload, valid complete reload, aligned versus
unaligned bitwise updates. Unit tests cover relational intersection,
stable repeated joins and missing words. 160 shader compiler unit and
integration tests, 53 DXBC and 215 Python tests passed.

The existing physical pixel heap control now includes both branch
alternatives and a two-iteration loop that changes texture indices.
Two runs on RTX 4080 pass all 20 draws each, including the previous
scalar/copy/in-place controls, two textures, point/linear samplers,
scissor and untouched guards. 5120 RGBA pixels per run, exact.
DXC and DXV validate all generated shaders. Desktop D3D12 host harness,
provider staged from the earlier #104 build (not a fresh #107 provider);
this change is compiler-only. No retail corpus, AMD, FPS or global
coverage claim.

```json
{
  "schema": 1,
  "scope": "Synthetic native pixel HLSL, desktop D3D12 direct resource/sampler heap, no retail replay",
  "passed": true,
  "adapter": "10de:2704",
  "guest_exit": 0,
  "cases": 20,
  "pixels": 5120,
  "color_components": 20480,
  "tools_sha256": {
    "emitter": "b955ff0492188d8a2da63f4a9f2db3b856813fdabf7777131089cdfb9f987c05",
    "provider": "fc1bf7e2a44361d4e0ee624315f404a436f5c414660a637cbf959ee26bf02c70",
    "dxc": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
    "dxv": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3"
  },
  "guest_sha256": "23d4568a65a6378f709814efcbf9a6a624cbd0775a302df53eeaad281ca0310e",
  "shaders": {
    "vertex": "aa0342a47690e20c6d2069655f685f049093f2d83f91b9164359a02e16a181d4",
    "scalar": "d11729c782c334762d928530c934c7d3738ffe7bc9b7dcc610cc3763f8239431",
    "copy": "d11729c782c334762d928530c934c7d3738ffe7bc9b7dcc610cc3763f8239431",
    "inplace": "d11729c782c334762d928530c934c7d3738ffe7bc9b7dcc610cc3763f8239431",
    "branch": "e3e98931ad1fe55438f814c082ae642d98a0ee44a5e05721a62bb64a30b5a514",
    "loop": "de3661d5f8b44f1b77fb4545422371b7a81bb8e3b677ae5360a9fe3bc609a9d2"
  },
  "rows": [
    {
      "case": "scalar_0_0",
      "texture": 0,
      "sampler": 0,
      "matched": true,
      "readback_sha256": "7e9153ca2ea553a55233a53fae56fea9a8384f3009ae80ae3ab055cd45dcd30a"
    },
    {
      "case": "scalar_0_1",
      "texture": 0,
      "sampler": 1,
      "matched": true,
      "readback_sha256": "9a19cc88281b0bd038a3335bb73252b14071773a90e84f95e09bed01c2eb5f37"
    },
    {
      "case": "scalar_1_0",
      "texture": 1,
      "sampler": 0,
      "matched": true,
      "readback_sha256": "1b84bc33604143d36e4c4937cf362a01f81f1800eaf429f96aa18f9d17da1508"
    },
    {
      "case": "scalar_1_1",
      "texture": 1,
      "sampler": 1,
      "matched": true,
      "readback_sha256": "a80e8cf4ba0ed1bd6c4541958611c8b83aee9e2a56c1693d8387b10789eebe06"
    },
    {
      "case": "copy_0_0",
      "texture": 0,
      "sampler": 0,
      "matched": true,
      "readback_sha256": "7e9153ca2ea553a55233a53fae56fea9a8384f3009ae80ae3ab055cd45dcd30a"
    },
    {
      "case": "copy_0_1",
      "texture": 0,
      "sampler": 1,
      "matched": true,
      "readback_sha256": "9a19cc88281b0bd038a3335bb73252b14071773a90e84f95e09bed01c2eb5f37"
    },
    {
      "case": "copy_1_0",
      "texture": 1,
      "sampler": 0,
      "matched": true,
      "readback_sha256": "1b84bc33604143d36e4c4937cf362a01f81f1800eaf429f96aa18f9d17da1508"
    },
    {
      "case": "copy_1_1",
      "texture": 1,
      "sampler": 1,
      "matched": true,
      "readback_sha256": "a80e8cf4ba0ed1bd6c4541958611c8b83aee9e2a56c1693d8387b10789eebe06"
    },
    {
      "case": "inplace_0_0",
      "texture": 0,
      "sampler": 0,
      "matched": true,
      "readback_sha256": "7e9153ca2ea553a55233a53fae56fea9a8384f3009ae80ae3ab055cd45dcd30a"
    },
    {
      "case": "inplace_0_1",
      "texture": 0,
      "sampler": 1,
      "matched": true,
      "readback_sha256": "9a19cc88281b0bd038a3335bb73252b14071773a90e84f95e09bed01c2eb5f37"
    },
    {
      "case": "inplace_1_0",
      "texture": 1,
      "sampler": 0,
      "matched": true,
      "readback_sha256": "1b84bc33604143d36e4c4937cf362a01f81f1800eaf429f96aa18f9d17da1508"
    },
    {
      "case": "inplace_1_1",
      "texture": 1,
      "sampler": 1,
      "matched": true,
      "readback_sha256": "a80e8cf4ba0ed1bd6c4541958611c8b83aee9e2a56c1693d8387b10789eebe06"
    },
    {
      "case": "branch_0_0",
      "texture": 0,
      "sampler": 0,
      "matched": true,
      "readback_sha256": "7e9153ca2ea553a55233a53fae56fea9a8384f3009ae80ae3ab055cd45dcd30a"
    },
    {
      "case": "branch_0_1",
      "texture": 0,
      "sampler": 1,
      "matched": true,
      "readback_sha256": "a80e8cf4ba0ed1bd6c4541958611c8b83aee9e2a56c1693d8387b10789eebe06"
    },
    {
      "case": "branch_1_0",
      "texture": 1,
      "sampler": 0,
      "matched": true,
      "readback_sha256": "1b84bc33604143d36e4c4937cf362a01f81f1800eaf429f96aa18f9d17da1508"
    },
    {
      "case": "branch_1_1",
      "texture": 1,
      "sampler": 1,
      "matched": true,
      "readback_sha256": "a80e8cf4ba0ed1bd6c4541958611c8b83aee9e2a56c1693d8387b10789eebe06"
    },
    {
      "case": "loop_0_0",
      "texture": 0,
      "sampler": 0,
      "matched": true,
      "readback_sha256": "1b84bc33604143d36e4c4937cf362a01f81f1800eaf429f96aa18f9d17da1508"
    },
    {
      "case": "loop_0_1",
      "texture": 0,
      "sampler": 1,
      "matched": true,
      "readback_sha256": "a80e8cf4ba0ed1bd6c4541958611c8b83aee9e2a56c1693d8387b10789eebe06"
    },
    {
      "case": "loop_1_0",
      "texture": 1,
      "sampler": 0,
      "matched": true,
      "readback_sha256": "7e9153ca2ea553a55233a53fae56fea9a8384f3009ae80ae3ab055cd45dcd30a"
    },
    {
      "case": "loop_1_1",
      "texture": 1,
      "sampler": 1,
      "matched": true,
      "readback_sha256": "9a19cc88281b0bd038a3335bb73252b14071773a90e84f95e09bed01c2eb5f37"
    }
  ]
}
```
