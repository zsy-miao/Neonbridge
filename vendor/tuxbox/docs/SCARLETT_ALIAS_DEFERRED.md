# Verify deferred placed-image refresh

The maintainer's commit 524cd21 removes CPU waiting for publication of
a distinct overlapping texture. This supersedes PR #103's bounded wait.
Preserve the useful synthetic reproduction, now checking the chosen
deferred-refresh policy rather than a two-second delay.

The writer is blocked on a fence the CPU signals only after the reader
submission returns. Two distinct overlapping slices initially contain
old data. The reader submission must return without waiting for the
writer, preserve the old image for this batch, and refresh both images
on a subsequent batch after publication. Verify both readbacks and
the two deferral diagnostics. A one-second upper timing gate allows
scheduling variation but rejects the previous two-second wait.
No CPU-side timing improvement is extrapolated to retail FPS.

Build with build.py d3d12, then use tools/check_d3d12_regressions.py
--case alias-deferred --case copy-resource --case placement. The test
is opt-in. Local RTX 4080 / Proton Experimental: deferred submission
elapsed 0 ms at integer-millisecond resolution, both old and refreshed
bytes correct, and all three controls pass. The provider was rebuilt
from 7b815f2 in the #104 test worktree; that branch changes the compiler,
not the D3D12 provider source. The updated guest was rebuilt separately.
212 Python tests passed. No AMD or retail claim.

```json
{
  "schema": 1,
  "scope": "Synthetic D3D12 provider controls; not retail execution",
  "adapter": "10de:2704",
  "proton": "1794497090 experimental-11.0-20261112-x86_64",
  "artifacts_sha256": {
    "d3d12_xs.dll": "fc1bf7e2a44361d4e0ee624315f404a436f5c414660a637cbf959ee26bf02c70",
    "d3d12.dll": "fc2bbef02509fb11c334cc96dd211f64def94bb7b4eb82082a4808e6e0549543",
    "d3d12core.dll": "9356501320114773be279506644aabd9a1717ff451d480ac802a4fd4e94b2576",
    "dxgi.dll": "2814edc3350cae6d34a75b7900df19752889c310ef34255c7316c4a95e1b183f",
    "dxcompiler.dll": "0689e2fee0a1c9df8662a59680469f24e2a3ad7ce6bbbcc9284f7a83172bfc59",
    "dxil.dll": "64be2368211d257a038a7d4ea6bb3e24c878eb8d588c7b8414fccf759b69c2ac",
    "d3d12-copy-resource.exe": "ad0dabc9766cd206c376cc89939174263fddcad1f36772331c77daf7ae2e9dac",
    "d3d12-placement.exe": "2841d8c2e9ed1d863edccdf291cc39db43c768874eaee7da0503a52184d5ca28"
  },
  "unchanged": true,
  "cases": [
    {
      "case": "alias-deferred",
      "passed": true,
      "exit": 0,
      "seconds": 14.375,
      "receipt": "d3d12-alias-deferred PASS pci=10de:2704 overlaps=2 old=verified refreshed=verified no_cpu_wait=1"
    },
    {
      "case": "copy-resource",
      "passed": true,
      "exit": 0,
      "seconds": 17.079,
      "receipt": "d3d12-copy-resource PASS pci=10de:2704 slot=84 buffer_bytes=1028 texture_subresources=8 nonzero_option=E_NOTIMPL self_copy=E_INVALIDARG source_and_destination=verified placed_uploads=2 placed_dst=verified same_va_alias=verified cpu_writeback=verified partial_copy_texels=2 overlap_mips=4 pending_reader=verified"
    },
    {
      "case": "placement",
      "passed": true,
      "exit": 0,
      "seconds": 20.794,
      "receipt": "d3d12-placement PASS pci=10de:2704 slot=44 VA-offsets=0,4096,0 passes=2 BC1-blocks=522 cube-blocks=16404 R8-texels=262144 rejected=3 configured_vm=0"
    }
  ],
  "passed": true
}
```
