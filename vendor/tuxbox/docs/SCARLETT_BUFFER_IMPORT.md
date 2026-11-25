# Preserve placed buffer identity after late commitment

The Scarlett layout placement entry point (slot 133) first tries to import a
possibly widened resource. On failure the old retry could successfully
commit and import the original guest pages, then unconditionally register
the result as a device-local stand-in with base zero and no imported heap.
The resource's GetGPUVirtualAddress returned zero, losing guest descriptor
identity and remap tracking.

Retry the exact widened import after committing the checked host allocation
span, keeping its heap Arc and the original guest address and offset. If
the original direct import is used instead, keep its actual heap and zero
offset. Preserve the existing unrelated-device-buffer fallback by default;
it remains a compatibility approximation, not proof of shared page data.
When native alignment cannot be determined, the non-strict path retains
the prior commitment attempt. Checked arithmetic avoids wraparound.

Set TUXBOX_TRACE_BUFFER_BACKING=1 with TUXBOX_LOG=stderr to see imported,
commit_retry, pool_open, pool_hit, device_standin and strict_rejected
counts. First eight and powers of two only; these sampled counts are lower
bounds, not full per-resource traces. No guest addresses in these new
records. TUXBOX_REQUIRE_BUFFER_IMPORT=1 refuses the unrelated stand-in
rather than concealing a failed import; it is opt-in and may stop a title.
Normal compatibility mode remains the default. A complete stderr line
is now assembled before writing to reduce interleaving with driver logs.

## Reproducer

Use the existing build.py d3d12 and check_d3d12_regressions.py, adding
--case buffer-layout --case buffer-import --case buffer-import-reject.
The tests are opt-in, because external host memory import is driver-specific.
The slot 133 guest uses the Scarlett 0x20004 profile, Device11 and queue
Signal slot 16. The test reserves two pages, creates the resources before
commitment, checks both original VAs, writes independent data and reads
back 512 words from aligned and four-byte-displaced buffers. Before the
fix, aligned placement succeeded but returned VA zero (failed identity
check at index 0). After the fix, both imports and retries succeed and
the readback is exact.

The separate slot 44 test registers two pages, imports A/B/A, releases
and recreates A after GPU completion, and checks 1536 words. Expected
native opens 3, cache hits 2, imports 5. The negative test releases a
reservation and proves strict mode rejects the now unbacked address.
These initial controls do not exercise retail descriptor/pipeline failures. See the queue-ordered XMem follow-up below.

182 Python tests and Windows-target D3D12 typecheck passed; full provider
build succeeded. Nine physical controls passed on RTX 4080, including
six existing table/heap/placement/raster/indirect/mesh controls.
AMD and retail not tested.

```json
{
  "schema": 1,
  "scope": "Synthetic D3D12 provider controls; not retail execution",
  "adapter": "10de:2704",
  "proton": "1794497090 experimental-11.0-20261112-x86_64",
  "artifacts_sha256": {
    "d3d12_xs.dll": "9fba726ecfb79c3d0af58666c098c2d158f6bc5b1cfbeff4b4bd14226ca6c858",
    "d3d12.dll": "fc2bbef02509fb11c334cc96dd211f64def94bb7b4eb82082a4808e6e0549543",
    "d3d12core.dll": "9356501320114773be279506644aabd9a1717ff451d480ac802a4fd4e94b2576",
    "dxgi.dll": "2814edc3350cae6d34a75b7900df19752889c310ef34255c7316c4a95e1b183f",
    "dxcompiler.dll": "0689e2fee0a1c9df8662a59680469f24e2a3ad7ce6bbbcc9284f7a83172bfc59",
    "dxil.dll": "64be2368211d257a038a7d4ea6bb3e24c878eb8d588c7b8414fccf759b69c2ac",
    "d3d12-buffer-import.exe": "b6a8ad5f97cea21f0037496bf3d0140f1fffc9075eefc1e973ed30e58f8195e3",
    "d3d12-tables.exe": "9868dcd4a1ef417c6a17994c21500c20ffaf49dad5ea01da18a40824b20ae54f",
    "d3d12-heaps.exe": "bf79be902d100cdb6c65fbf3b07e887755387bac50475249d08521b721d63a43",
    "d3d12-placement.exe": "1c0f48f83c69ce53946a3d46dfd09affacaea437562815f00c1f658bb1203c92",
    "d3d12-draw.exe": "bce100ec65771512505e9bc58e96a7cfaf7b46eac16311fef2d6061b29c3ee5f",
    "d3d12-indirect.exe": "ffc41c23a08d06b799bedd0d23b4ca0ee0488fac6b382a5a7e93b2c042386358",
    "d3d12-mesh.exe": "eed9ca26ad0329335be2974ba7e7ae56a45e2dde9b7d49cc8a72be6a1d0c6f5c"
  },
  "unchanged": true,
  "cases": [
    {
      "case": "buffer-layout",
      "passed": true,
      "exit": 0,
      "seconds": 12.77,
      "receipt": "d3d12-buffer-layout PASS pci=10de:2704 slot=133 words=512/512 VA-offsets=0,4"
    },
    {
      "case": "buffer-import",
      "passed": true,
      "exit": 0,
      "seconds": 21.29,
      "receipt": "d3d12-buffer-import PASS pci=10de:2704 pages=2 words=1536/1536 passes=2 aliases=2"
    },
    {
      "case": "buffer-import-reject",
      "passed": true,
      "exit": 0,
      "seconds": 18.875,
      "receipt": "d3d12-buffer-import-reject PASS pci=10de:2704 rejected=1"
    },
    {
      "case": "tables",
      "passed": true,
      "exit": 0,
      "seconds": 17.652,
      "receipt": "d3d12-tables PASS pci=10de:2704 words=16/16 guards=4 heaps=2 rejected=3 placed-srv=16/16 range-copies=4 descriptor-mode=1"
    },
    {
      "case": "heaps",
      "passed": true,
      "exit": 0,
      "seconds": 16.129,
      "receipt": "d3d12-heaps PASS pci=10de:2704 heap=32MiB offsets=0,65536 words=256/256 invalid=4 display=3840x2160/RGB10A2 pixel=c00ffc00 volume=45/45 priorities=4/4 invalid_priority_calls=4"
    },
    {
      "case": "placement",
      "passed": true,
      "exit": 0,
      "seconds": 16.464,
      "receipt": "d3d12-placement PASS pci=10de:2704 slot=44 VA-offsets=0,4096,0 passes=2 BC1-blocks=522 cube-blocks=16404 R8-texels=262144 rejected=3 configured_vm=0"
    },
    {
      "case": "stream",
      "passed": true,
      "exit": 0,
      "seconds": 18.803,
      "receipt": "d3d12-stream PASS pci=10de:2704 slot=86 bytes=712 indices=16,32 half-red-blue=18 magenta=18 blue=220 rejected=4"
    },
    {
      "case": "indirect-scarlett",
      "passed": true,
      "exit": 0,
      "seconds": 18.954,
      "receipt": "d3d12-indirect PASS pci=10de:2704 sdk=0x20004 device=41 list=61 words=64/64 counts=direct,one,zero flush=global,range,wake-up(0x3000:0+0xa30000),zero-length,global-range budget=default,rejected2 mesh=0x00000000 rays=0x00000000"
    },
    {
      "case": "mesh",
      "passed": true,
      "exit": 0,
      "seconds": 16.594,
      "receipt": "d3d12-mesh PASS pci=10de:2704 tier=1 stream=680 direct=6 amplified=12 indirect=2 zero=0 pixels=256/256 guards=32 rejected=5"
    }
  ],
  "passed": true
}
```


## Queue-ordered XMem follow-up

Build both targets (build.py xmem and build.py d3d12), then invoke the
existing regression runner with --xmem build/xmem-smoke/xmem.dll
--case buffer-remap. The runner stages and fingerprints XMem alongside
the GPU artifacts.

Two physical pages contain independent patterns. Map A into the guest
GPU aperture, create an imported buffer, repeat the same mapping and
create a second view. Submit both copies before replacing A with B on
queue slot 31, then immediately import the same VA again without a CPU
fence wait. Read back the two pre-replacement copies and the new copy
after queue completion. Keep the old resource references alive until
the fence completes. Finally unmap the destination and release the
physical pages.

The control requires 768 exact words, one identical-map cache hit,
two native opens, three imports, exactly one stale-import retirement
and no device-local stand-ins. It exercises queue ordering, but does
not force the driver to use the asynchronous worker path if the queue
has already drained.

On RTX 4080, driver 615.78.8, Proton Experimental, the control passed
on the original fix and again after rebuilding from main d94e4e5.
The other three buffer controls also passed on the latter build.
192 Python tests passed. No retail or AMD run of this follow-up.

```json
{
  "schema": 1,
  "scope": "Synthetic D3D12 provider controls; not retail execution",
  "adapter": "10de:2704",
  "proton": "1794497090 experimental-11.0-20261112-x86_64",
  "artifacts_sha256": {
    "d3d12_xs.dll": "bd72f5dae4d24a2f08997a63b52144bd74065912436d786cba48f56e0a05eeb8",
    "d3d12.dll": "fc2bbef02509fb11c334cc96dd211f64def94bb7b4eb82082a4808e6e0549543",
    "d3d12core.dll": "9356501320114773be279506644aabd9a1717ff451d480ac802a4fd4e94b2576",
    "dxgi.dll": "2814edc3350cae6d34a75b7900df19752889c310ef34255c7316c4a95e1b183f",
    "dxcompiler.dll": "0689e2fee0a1c9df8662a59680469f24e2a3ad7ce6bbbcc9284f7a83172bfc59",
    "dxil.dll": "64be2368211d257a038a7d4ea6bb3e24c878eb8d588c7b8414fccf759b69c2ac",
    "d3d12-buffer-import.exe": "b3134947831647e7a962517d6ad8b8c0553c4994c13ba452c4a52e4d96419596",
    "xmem.dll": "e892155317c6b46660b0c9b675acc09253dd0994b0eeed46e23b6bd2d6c96b60"
  },
  "unchanged": true,
  "cases": [
    {
      "case": "buffer-remap",
      "passed": true,
      "exit": 0,
      "seconds": 13.445,
      "receipt": "d3d12-buffer-remap PASS pci=10de:2704 words=768/768 identical=1 replacement=1"
    },
    {
      "case": "buffer-layout",
      "passed": true,
      "exit": 0,
      "seconds": 13.458,
      "receipt": "d3d12-buffer-layout PASS pci=10de:2704 slot=133 words=512/512 VA-offsets=0,4"
    },
    {
      "case": "buffer-import",
      "passed": true,
      "exit": 0,
      "seconds": 12.647,
      "receipt": "d3d12-buffer-import PASS pci=10de:2704 pages=2 words=1536/1536 passes=2 aliases=2"
    },
    {
      "case": "buffer-import-reject",
      "passed": true,
      "exit": 0,
      "seconds": 22.59,
      "receipt": "d3d12-buffer-import-reject PASS pci=10de:2704 rejected=1"
    }
  ],
  "passed": true
}
```
