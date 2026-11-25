# Offline synthetic provider regression suite

Build the provider and synthetic guests with the existing `build.py d3d12`.
Run:

```sh
python tools/check_d3d12_regressions.py --build-dir build/d3d12-smoke \
  --proton "$PROTON" --prefix "$PREFIX" --compiler-dir "$WINDOWS_DXC" \
  --adapter VVVV:DDDD --work /tmp/scarlett-regression-work \
  --output /tmp/scarlett-regression-receipt.json
```

The adapter must be explicit (NVIDIA or AMD). Every case gets a fresh
disposable offline prefix with read-only access outside its workspace.
Use fresh output paths. The original prefix is not modified. The runner
verifies the exact PCI adapter, successful exit, fixture receipt and
selected control-specific counts. SHA-256 fingerprints identify the
staged artifacts; the runner does not establish that a caller-supplied
binary really corresponds to a particular source revision. Keep logs local.

Optional `--case mesh` requires a mesh-capable device. The
`--case indirect-scarlett` mode requires the companion ABI fixture from
PR #87. An unsupported option or missing capability is a failure, not a pass.

## Forced null compute control

Two pipelines deliberately use an eight-byte malformed synthetic shader;
only one is recorded. Two normal affine dispatches write known results
and 64 substituted dispatches must leave their target words unchanged.
The test exercises initial/reset pipeline binding, SetPipelineState and
ClearState. In addition to 64/64 buffer words, the runner expects exactly
two checkpoint summaries (1 and 63 recorded null calls) and matching
stage digests, without presentation. No title code or data is used.
This verifies compute usage, not null raster attribution or title
rendering.

## Local validation

RTX 4080 / Proton Experimental, provider from 95af9d3 and companion
indirect fixture #87. Nine controls passed (the stream CLI argument
order was corrected and the stream test repeated). The AMD path is
portable by adapter selection but has not yet been exercised on AMD
hardware. This is synthetic functional coverage, not a retail launch.

```json
{
  "adapter": "10de:2704",
  "provider_base": "95af9d3",
  "artifacts_sha256": {
    "d3d12_xs.dll": "d5630a2fb1a0698a41cddeddea0d88dcb3de1c7db68be5124dd1d182090e9eaa",
    "d3d12.dll": "fc2bbef02509fb11c334cc96dd211f64def94bb7b4eb82082a4808e6e0549543",
    "d3d12core.dll": "9356501320114773be279506644aabd9a1717ff451d480ac802a4fd4e94b2576",
    "dxgi.dll": "2814edc3350cae6d34a75b7900df19752889c310ef34255c7316c4a95e1b183f",
    "dxcompiler.dll": "0689e2fee0a1c9df8662a59680469f24e2a3ad7ce6bbbcc9284f7a83172bfc59",
    "dxil.dll": "64be2368211d257a038a7d4ea6bb3e24c878eb8d588c7b8414fccf759b69c2ac",
    "d3d12-null-usage.exe": "a1adc6f5d2d790188e71d628afc5210e1289be4a748db261e9e31f3c9a2abdd8",
    "d3d12-indirect.exe": "ffc41c23a08d06b799bedd0d23b4ca0ee0488fac6b382a5a7e93b2c042386358",
    "d3d12-tables.exe": "9868dcd4a1ef417c6a17994c21500c20ffaf49dad5ea01da18a40824b20ae54f",
    "d3d12-heaps.exe": "bf79be902d100cdb6c65fbf3b07e887755387bac50475249d08521b721d63a43",
    "d3d12-placement.exe": "1c0f48f83c69ce53946a3d46dfd09affacaea437562815f00c1f658bb1203c92",
    "d3d12-draw.exe": "bce100ec65771512505e9bc58e96a7cfaf7b46eac16311fef2d6061b29c3ee5f",
    "xbox-frame.exe": "7905aee642b2a00eb419d44397072a6569d5e9b3c0bb55e63f9e2a11410e6086",
    "d3d12-mesh.exe": "eed9ca26ad0329335be2974ba7e7ae56a45e2dde9b7d49cc8a72be6a1d0c6f5c"
  },
  "cases": [
    {
      "case": "null-usage",
      "passed": true,
      "exit": 0,
      "seconds": 13.876,
      "receipt": "d3d12-null-usage PASS pci=10de:2704 words=64/64 recorded=64 valid=2"
    },
    {
      "case": "indirect",
      "passed": true,
      "exit": 0,
      "seconds": 13.744,
      "receipt": "d3d12-indirect PASS pci=10de:2704 sdk=0x20004 device=41 list=59 words=64/64 counts=direct,one,zero flush=global,range,wake-up(0x3000:0+0xa30000),zero-length,global-range budget=default,rejected2 mesh=0x00000000 rays=0x00000000"
    },
    {
      "case": "indirect-scarlett",
      "passed": true,
      "exit": 0,
      "seconds": 14.477,
      "receipt": "d3d12-indirect PASS pci=10de:2704 sdk=0x20004 device=41 list=61 words=64/64 counts=direct,one,zero flush=global,range,wake-up(0x3000:0+0xa30000),zero-length,global-range budget=default,rejected2 mesh=0x00000000 rays=0x00000000"
    },
    {
      "case": "tables",
      "passed": true,
      "exit": 0,
      "seconds": 24.228,
      "receipt": "d3d12-tables PASS pci=10de:2704 words=16/16 guards=4 heaps=2 rejected=3 placed-srv=16/16 range-copies=4 descriptor-mode=1"
    },
    {
      "case": "heaps",
      "passed": true,
      "exit": 0,
      "seconds": 19.99,
      "receipt": "d3d12-heaps PASS pci=10de:2704 heap=32MiB offsets=0,65536 words=256/256 invalid=4 display=3840x2160/RGB10A2 pixel=c00ffc00 volume=45/45 priorities=4/4 invalid_priority_calls=4"
    },
    {
      "case": "placement",
      "passed": true,
      "exit": 0,
      "seconds": 18.702,
      "receipt": "d3d12-placement PASS pci=10de:2704 slot=44 VA-offsets=0,4096,0 passes=2 BC1-blocks=522 cube-blocks=16404 R8-texels=262144 rejected=3 configured_vm=0"
    },
    {
      "case": "stream",
      "passed": true,
      "exit": 0,
      "seconds": 14.329,
      "receipt": "d3d12-stream PASS pci=10de:2704 slot=86 bytes=712 indices=16,32 half-red-blue=18 magenta=18 blue=220 rejected=4"
    },
    {
      "case": "frame",
      "passed": true,
      "exit": 0,
      "seconds": 17.573,
      "receipt": "xbox-frame PASS pci=10de:2704 submissions=2 snapshots=2 pixels=962/962 inset=208 guards=3 fence=2 compute_fence=7"
    },
    {
      "case": "mesh",
      "passed": true,
      "exit": 0,
      "seconds": 13.745,
      "receipt": "d3d12-mesh PASS pci=10de:2704 tier=1 stream=680 direct=6 amplified=12 indirect=2 zero=0 pixels=256/256 guards=32 rejected=5"
    }
  ]
}
```


## Strict no-fallback follow-up

Every ordinary control explicitly disables null PSO fallback; only the
forced-null case enables it. The new null-disabled case verifies that
invalid bytecode fails with a null output, then creates a valid pipeline
without dispatching it. The runner rejects conflicting adapter/case
receipts, duplicate fields and unexpected substitutions or null ranks.
The forced-null case must emit exactly two substitution messages.
Wine and wineserver executable hashes are also recorded.

Ten controls passed on RTX 4080 with the integrated provider aa71f753
and the updated synthetic guest. The provider and its DXBC/XG sources,
Cargo manifests and lockfile are unchanged at f2184c1; the tested DLL
was not rebuilt after that merge. No AMD or retail claim.

```json
{
  "artifacts_sha256": {
    "d3d12_xs.dll": "8e39569ded113f5b3c180b784ebce5afd5c9f7fdee42e97a26fb314171aa5c17",
    "d3d12.dll": "fc2bbef02509fb11c334cc96dd211f64def94bb7b4eb82082a4808e6e0549543",
    "d3d12core.dll": "9356501320114773be279506644aabd9a1717ff451d480ac802a4fd4e94b2576",
    "dxgi.dll": "2814edc3350cae6d34a75b7900df19752889c310ef34255c7316c4a95e1b183f",
    "dxcompiler.dll": "0689e2fee0a1c9df8662a59680469f24e2a3ad7ce6bbbcc9284f7a83172bfc59",
    "dxil.dll": "64be2368211d257a038a7d4ea6bb3e24c878eb8d588c7b8414fccf759b69c2ac",
    "wine": "7a6de49c00d8ed2ba55c6967d3643c5ef729f5e562fb51e47e4f41c6cdb5c92a",
    "wineserver": "4c9afbd3bc3fc4229de03acc87f1abc69a9c1ab42918ef95406107b396a57486",
    "d3d12-null-usage.exe": "b4c9475259d6b7c654a47c69b0cc56d5e2dd1386dc3ff50d0dfb63b95ae7dc16",
    "d3d12-indirect.exe": "ffc41c23a08d06b799bedd0d23b4ca0ee0488fac6b382a5a7e93b2c042386358",
    "d3d12-draw.exe": "bce100ec65771512505e9bc58e96a7cfaf7b46eac16311fef2d6061b29c3ee5f",
    "d3d12-tables.exe": "9868dcd4a1ef417c6a17994c21500c20ffaf49dad5ea01da18a40824b20ae54f",
    "d3d12-heaps.exe": "bf79be902d100cdb6c65fbf3b07e887755387bac50475249d08521b721d63a43",
    "d3d12-placement.exe": "1c0f48f83c69ce53946a3d46dfd09affacaea437562815f00c1f658bb1203c92",
    "xbox-frame.exe": "7905aee642b2a00eb419d44397072a6569d5e9b3c0bb55e63f9e2a11410e6086",
    "d3d12-mesh.exe": "eed9ca26ad0329335be2974ba7e7ae56a45e2dde9b7d49cc8a72be6a1d0c6f5c"
  },
  "cases": [
    {
      "case": "null-disabled",
      "passed": true,
      "exit": 0,
      "seconds": 13.217,
      "receipt": "d3d12-null-disabled PASS pci=10de:2704 rejected=1 valid=1",
      "null_fallback_enabled": false
    },
    {
      "case": "null-usage",
      "passed": true,
      "exit": 0,
      "seconds": 12.854,
      "receipt": "d3d12-null-usage PASS pci=10de:2704 words=64/64 recorded=64 valid=2",
      "null_fallback_enabled": true
    },
    {
      "case": "indirect-scarlett",
      "passed": true,
      "exit": 0,
      "seconds": 21.843,
      "receipt": "d3d12-indirect PASS pci=10de:2704 sdk=0x20004 device=41 list=61 words=64/64 counts=direct,one,zero flush=global,range,wake-up(0x3000:0+0xa30000),zero-length,global-range budget=default,rejected2 mesh=0x00000000 rays=0x00000000",
      "null_fallback_enabled": false
    },
    {
      "case": "stream",
      "passed": true,
      "exit": 0,
      "seconds": 12.692,
      "receipt": "d3d12-stream PASS pci=10de:2704 slot=86 bytes=712 indices=16,32 half-red-blue=18 magenta=18 blue=220 rejected=4",
      "null_fallback_enabled": false
    },
    {
      "case": "tables",
      "passed": true,
      "exit": 0,
      "seconds": 14.034,
      "receipt": "d3d12-tables PASS pci=10de:2704 words=16/16 guards=4 heaps=2 rejected=3 placed-srv=16/16 range-copies=4 descriptor-mode=1",
      "null_fallback_enabled": false
    },
    {
      "case": "heaps",
      "passed": true,
      "exit": 0,
      "seconds": 19.728,
      "receipt": "d3d12-heaps PASS pci=10de:2704 heap=32MiB offsets=0,65536 words=256/256 invalid=4 display=3840x2160/RGB10A2 pixel=c00ffc00 volume=45/45 priorities=4/4 invalid_priority_calls=4",
      "null_fallback_enabled": false
    },
    {
      "case": "placement",
      "passed": true,
      "exit": 0,
      "seconds": 18.856,
      "receipt": "d3d12-placement PASS pci=10de:2704 slot=44 VA-offsets=0,4096,0 passes=2 BC1-blocks=522 cube-blocks=16404 R8-texels=262144 rejected=3 configured_vm=0",
      "null_fallback_enabled": false
    },
    {
      "case": "frame",
      "passed": true,
      "exit": 0,
      "seconds": 18.507,
      "receipt": "xbox-frame PASS pci=10de:2704 submissions=2 snapshots=2 pixels=962/962 inset=208 guards=3 fence=2 compute_fence=7",
      "null_fallback_enabled": false
    },
    {
      "case": "indirect",
      "passed": true,
      "exit": 0,
      "seconds": 18.976,
      "receipt": "d3d12-indirect PASS pci=10de:2704 sdk=0x20004 device=41 list=59 words=64/64 counts=direct,one,zero flush=global,range,wake-up(0x3000:0+0xa30000),zero-length,global-range budget=default,rejected2 mesh=0x00000000 rays=0x00000000",
      "null_fallback_enabled": false
    },
    {
      "case": "mesh",
      "passed": true,
      "exit": 0,
      "seconds": 18.633,
      "receipt": "d3d12-mesh PASS pci=10de:2704 tier=1 stream=680 direct=6 amplified=12 indirect=2 zero=0 pixels=256/256 guards=32 rejected=5",
      "null_fallback_enabled": false
    }
  ]
}
```
