# Preserve component types when linking portable pixel consumers

The existing mixed-stage retry reads the portable pixel input
interpolation and ordering, but previously only reconciled component
types for native Xbox pixel consumers. A native vertex producer with
a float4 TEXCOORD0 export linked to a portable nointerpolation uint4
TEXCOORD0 still declared a float4, despite retaining raw export bits.
On RTX 4080 through Proton, the generated VS and PS individually
compiled, but CreateGraphicsPipelineState returned E_INVALIDARG
(0x80070057); VKD3D reported incompatible TEXCOORD0 signatures with
the same register and mask.

Return the signature already checked against PSV rows alongside the
interpolation metadata. For a uniquely matched non-system semantic and
index, use the consumer's 32-bit uint, sint or float component type,
preserving the native export bits. Unsupported or ambiguous matching
portable types are refused rather than guessed. Native consumer logic
and the bound-consumer bytecode identity check are unchanged.
This does not repair missing semantics, arbitrary packed register
mismatches, or 16-bit native interfaces.

## Synthetic physical campaign

Build the host xshader, then run tools/check_portable_pixel_link.py
--help for arguments. The synthetic native gfx1030 VS uses VertexID,
a fullscreen triangle, and an unmodified raw parameter export containing
0x89abcdef, 0x3e800000, 0x3f000000 and 0x3f800000. The production CLI
extracts and links the generated cooked container to actual DXC pixel
bytecode. Check the output type in HLSL and validate both stages with
DXV, then draw using desktop D3D12. All nine combinations of uint,
sint, float producer and consumer declarations are tested with
nointerpolation. Compare 81 scissored and 175 untouched pixels per
draw, four color components plus depth, including row padding.
A mismatched bound consumer is refused with exit 65 and no HLSL.

Two runs of nine draws passed on RTX 4080 / Proton Experimental:
2304 pixels and 11520 color/depth components per run, exact.
The baseline uses the same fixture for float-to-uint and failed at
pipeline creation. The provider was staged from the #114 validation
build (9139f2f runtime; f111f5c changes the compiler only).
The desktop harness does not itself exercise the retail slot 86
on-demand cache/session transport. No claim about retail coverage,
AMD or FPS. 164 shader compiler, 53 DXBC and 225 Python tests passed
(442 local tests); the optional real DXC metadata test was enabled.

```json
{
  "baseline": {
    "adapter": "10de:2704",
    "exit": 1,
    "error": "graphics pipeline failed: 0x80070057",
    "matched": false
  },
  "final": {
    "schema": 1,
    "scope": "Synthetic native vertex via production CLI, DXC portable pixel, desktop D3D12; not retail replay",
    "wrong_consumer_rejected": true,
    "passed": true,
    "adapter": "10de:2704",
    "guest_exit": 0,
    "cases": 9,
    "pixels": 2304,
    "compared_components": 11520,
    "tools_sha256": {
      "emitter": "f2d1ac91fab617eedb3231048bfaee069b120cb8ca56ecf1c0818db55fe05323",
      "provider": "0efe7d65f21a7b58bea17f67f46dfc8e4cc5eecbf91663d3109b446ffe13fa4b",
      "dxc": "3dfc856b76291ffc7e45e21fd37ab80e85b34101de76a03253df6608785c92a0",
      "dxv": "33778294d67143d9f81526f4759c66d5a67a95ea9873448ef3595650a643cbd3"
    },
    "guest_sha256": "606370663491acbccb8f279752239e026ff159dfb6081c9c00815778e0ffd955",
    "rows": [
      {
        "case": "p1c1",
        "producer": "uint",
        "consumer": "uint",
        "vertex_sha256": "70f148b0cc7b0ea73ff25e8b4d6522149ca3b5ab5b64871a4506459e90031cfd",
        "pixel_sha256": "5c8b8f8668730b5c037ef03908ef55578f8e3834c966d8e0f6b5eefc6f4103ae",
        "matched": true,
        "readback_sha256": "7fd1ff85bfe50c897891d43566c5f82764b66c47645de9f7bf36ba67527e0c2a"
      },
      {
        "case": "p1c2",
        "producer": "uint",
        "consumer": "int",
        "vertex_sha256": "92d6d4873a6b723ef9bfb827eb73b7cda3bc7a87033037e2e106b11f0c8bed48",
        "pixel_sha256": "f72a1eeb15671dea4507ff80c472319b5be42911e4f5349618028563b844f798",
        "matched": true,
        "readback_sha256": "7fd1ff85bfe50c897891d43566c5f82764b66c47645de9f7bf36ba67527e0c2a"
      },
      {
        "case": "p1c3",
        "producer": "uint",
        "consumer": "float",
        "vertex_sha256": "e16270f35c672e6cc3b74385f8078150e81a4dc25679e051ba41ed01dee98ead",
        "pixel_sha256": "929259a22720c63a708990c78e3d5d5202c047dda65b7f05aa98ca0e2684556a",
        "matched": true,
        "readback_sha256": "7fd1ff85bfe50c897891d43566c5f82764b66c47645de9f7bf36ba67527e0c2a"
      },
      {
        "case": "p2c1",
        "producer": "int",
        "consumer": "uint",
        "vertex_sha256": "70f148b0cc7b0ea73ff25e8b4d6522149ca3b5ab5b64871a4506459e90031cfd",
        "pixel_sha256": "5c8b8f8668730b5c037ef03908ef55578f8e3834c966d8e0f6b5eefc6f4103ae",
        "matched": true,
        "readback_sha256": "7fd1ff85bfe50c897891d43566c5f82764b66c47645de9f7bf36ba67527e0c2a"
      },
      {
        "case": "p2c2",
        "producer": "int",
        "consumer": "int",
        "vertex_sha256": "92d6d4873a6b723ef9bfb827eb73b7cda3bc7a87033037e2e106b11f0c8bed48",
        "pixel_sha256": "f72a1eeb15671dea4507ff80c472319b5be42911e4f5349618028563b844f798",
        "matched": true,
        "readback_sha256": "7fd1ff85bfe50c897891d43566c5f82764b66c47645de9f7bf36ba67527e0c2a"
      },
      {
        "case": "p2c3",
        "producer": "int",
        "consumer": "float",
        "vertex_sha256": "e16270f35c672e6cc3b74385f8078150e81a4dc25679e051ba41ed01dee98ead",
        "pixel_sha256": "929259a22720c63a708990c78e3d5d5202c047dda65b7f05aa98ca0e2684556a",
        "matched": true,
        "readback_sha256": "7fd1ff85bfe50c897891d43566c5f82764b66c47645de9f7bf36ba67527e0c2a"
      },
      {
        "case": "p3c1",
        "producer": "float",
        "consumer": "uint",
        "vertex_sha256": "70f148b0cc7b0ea73ff25e8b4d6522149ca3b5ab5b64871a4506459e90031cfd",
        "pixel_sha256": "5c8b8f8668730b5c037ef03908ef55578f8e3834c966d8e0f6b5eefc6f4103ae",
        "matched": true,
        "readback_sha256": "7fd1ff85bfe50c897891d43566c5f82764b66c47645de9f7bf36ba67527e0c2a"
      },
      {
        "case": "p3c2",
        "producer": "float",
        "consumer": "int",
        "vertex_sha256": "92d6d4873a6b723ef9bfb827eb73b7cda3bc7a87033037e2e106b11f0c8bed48",
        "pixel_sha256": "f72a1eeb15671dea4507ff80c472319b5be42911e4f5349618028563b844f798",
        "matched": true,
        "readback_sha256": "7fd1ff85bfe50c897891d43566c5f82764b66c47645de9f7bf36ba67527e0c2a"
      },
      {
        "case": "p3c3",
        "producer": "float",
        "consumer": "float",
        "vertex_sha256": "e16270f35c672e6cc3b74385f8078150e81a4dc25679e051ba41ed01dee98ead",
        "pixel_sha256": "929259a22720c63a708990c78e3d5d5202c047dda65b7f05aa98ca0e2684556a",
        "matched": true,
        "readback_sha256": "7fd1ff85bfe50c897891d43566c5f82764b66c47645de9f7bf36ba67527e0c2a"
      }
    ]
  }
}
```


## Actual slot 86 on-demand follow-up

Run tools/check_slot86_portable_link.py with a freshly built host emitter
and provider (including the import library). The synthetic Scarlett
Device11 guest submits the native vertex container and two distinct
portable pixel consumers to slot 86. Both initially require incompatible
signatures to be repaired. The actual provider and launcher worker
compile the ordinary producer and each exact link. The guest draws
four times, alternating consumers, and compares all RGBA pixels against
independent expected values, including 175 guards per draw.
A second process starts from a separate disposable prefix with only
the verified persistent compiler cache copied from the first. The
optional cache reset in the helper touches only the disposable prefix.

Eight draws, 2048 pixels, 3 cold compilations and 0 warm compilations.
Both cache receipts match exactly. Null substitutions are disabled.
Repeated successfully after rebuilding the provider and emitter from
465318f. 232 Python tests pass. This exercises the VS/PS slot 86 request,
worker, retry, draw and warm cache path; not a native GS/MS consumer or
retail replay. No AMD, retail coverage, VRAM or FPS claim.

```json
{
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
```
