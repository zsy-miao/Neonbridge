# Heap offset and straight-line descriptor provenance

Synthetic native pixel programs after #101 exposed stale sampler-offset
facts: an image sample overwrote the register (or its second component)
yet the emitter accepted those result bits as an aligned sampler offset.
Conversely a genuine vector copy of a scaled sampler offset was refused.

Compute reaching alignment facts before invalidating vector result words.
Track ordinary copies and selects of the same offset kind, including
in-place copies, but not DPP/SDWA or modified vector encodings. Invalidate
offset facts across the supported image/buffer result widths, with
conservative upper bounds for special multiword results. These are
alignment facts, not a proof of index bounds or arbitrary EXEC behavior.

Two additional native controls showed that a texture or sampler descriptor
could mix words from distinct loads: ordinal-only tags did not identify
the load. In a single straight-line region, tag dynamic heap descriptor
words with their load site and require matching origins. Full descriptor
copies remain supported. Complex control flow retains the previous join
policy; correlated descriptor joins and loop generations are not solved.
The patch does not validate arbitrary descriptor reinterpretation or
prove that any such program exists in the title.

## Validation

Four newly rejected malformed provenance cases; valid sampler offset copy
now accepted; complete texture descriptor copy remains accepted. Baseline
tests reproduced the acceptance/refusal issues, then all 13 sampler_heap
tests passed. 215 Python, 152 shader compiler unit/integration and 53
DXBC tests passed (420 total).

Build the pixel_heap_offsets Rust example and run
tools/check_pixel_heap_offsets.py --help for the physical campaign.
Actual translated native pixel HLSL, DXC ps_6_6 and DXV, real directly
indexed texture/sampler heaps and two root constants: scalar, copied and
in-place copied sampler offsets, two distinct 2x2 textures and point
versus linear sampling. Compare all 256 RGBA pixels per case, including
81 scissored pixels and 175 guards. Two runs on RTX 4080 passed all 12
draws (3072 pixels / 12288 color components per run). The desktop D3D12
host harness stages the provider built for the previous #104 campaign;
this patch changes the compiler, not the provider. No retail replay,
AMD validation, coverage percentage or FPS claim.

```json
{
  "schema": 1,
  "scope": "Synthetic native pixel HLSL, desktop D3D12 direct resource/sampler heap, no retail replay",
  "passed": true,
  "adapter": "10de:2704",
  "guest_exit": 0,
  "cases": 12,
  "pixels": 3072,
  "color_components": 12288,
  "tools_sha256": {
    "emitter": "008fa8cd4f23fb4efe28a2af51fd8bd9d9588d7cac80f062b8068b756f1df0b7",
    "provider": "fc1bf7e2a44361d4e0ee624315f404a436f5c414660a637cbf959ee26bf02c70",
    "dxc": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
    "dxv": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3"
  },
  "guest_sha256": "23d4568a65a6378f709814efcbf9a6a624cbd0775a302df53eeaad281ca0310e",
  "shaders": {
    "vertex": "aa0342a47690e20c6d2069655f685f049093f2d83f91b9164359a02e16a181d4",
    "scalar": "d11729c782c334762d928530c934c7d3738ffe7bc9b7dcc610cc3763f8239431",
    "copy": "d11729c782c334762d928530c934c7d3738ffe7bc9b7dcc610cc3763f8239431",
    "inplace": "d11729c782c334762d928530c934c7d3738ffe7bc9b7dcc610cc3763f8239431"
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
    }
  ]
}
```
