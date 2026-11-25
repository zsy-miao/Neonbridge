# Physical root CBV window regression control

Test-only follow-up to #107 (5c73bee). A Scarlett Device11 compute guest
uses slot 41 directly on CPU-backed committed guest pages, without
placing a buffer first. Three 256-byte-aligned starts (0, 256 and
65280 bytes into a 64 KiB page) expose one full 64 KiB constant buffer.
Read uint4 values at indices 0, 1, 2048 and 4095; compare all 24 output
words per dispatch, including eight untouched guards. The crossing
cases require a 128 KiB host import from the preceding page boundary.

Build with build.py d3d12; run check_d3d12_regressions.py with
--case root-cbv-window. The opt-in test requires three actual view
imports of 65536, 131072 and 131072 bytes, no short-range diagnostic
and no null substitution. It is capability-specific and not a claim
that the old code reproduces an NVIDIA Xid on every driver. No
production changes; no retail or AMD run.

RTX 4080, Proton Experimental, provider built from ade292d:
two executions of the new control pass, 72/72 words each. The second
run includes import tracing and is checked by the final validator.
Existing tables, stream and Scarlett indirect controls also pass
on the same provider. 216 Python tests pass.

```json
{
  "traced": {
    "schema": 1,
    "scope": "Synthetic D3D12 provider controls; not retail execution",
    "adapter": "10de:2704",
    "proton": "1794497090 experimental-11.0-20261112-x86_64",
    "artifacts_sha256": {
      "d3d12_xs.dll": "0efe7d65f21a7b58bea17f67f46dfc8e4cc5eecbf91663d3109b446ffe13fa4b",
      "d3d12.dll": "fc2bbef02509fb11c334cc96dd211f64def94bb7b4eb82082a4808e6e0549543",
      "d3d12core.dll": "9356501320114773be279506644aabd9a1717ff451d480ac802a4fd4e94b2576",
      "dxgi.dll": "2814edc3350cae6d34a75b7900df19752889c310ef34255c7316c4a95e1b183f",
      "dxcompiler.dll": "0689e2fee0a1c9df8662a59680469f24e2a3ad7ce6bbbcc9284f7a83172bfc59",
      "dxil.dll": "64be2368211d257a038a7d4ea6bb3e24c878eb8d588c7b8414fccf759b69c2ac",
      "d3d12-root-cbv-window.exe": "cb33b8c8ce83070aeea950bf535f95e62cdf3eef3964a1aa44413d5905d1b675"
    },
    "unchanged": true,
    "cases": [
      {
        "case": "root-cbv-window",
        "passed": true,
        "exit": 0,
        "seconds": 37.202,
        "receipt": "d3d12-root-cbv-window PASS pci=10de:2704 words=72/72 window=65536 offsets=0,256,65280"
      }
    ],
    "passed": true
  },
  "other_controls": {
    "schema": 1,
    "scope": "Synthetic D3D12 provider controls; not retail execution",
    "adapter": "10de:2704",
    "proton": "1794497090 experimental-11.0-20261112-x86_64",
    "artifacts_sha256": {
      "d3d12_xs.dll": "0efe7d65f21a7b58bea17f67f46dfc8e4cc5eecbf91663d3109b446ffe13fa4b",
      "d3d12.dll": "fc2bbef02509fb11c334cc96dd211f64def94bb7b4eb82082a4808e6e0549543",
      "d3d12core.dll": "9356501320114773be279506644aabd9a1717ff451d480ac802a4fd4e94b2576",
      "dxgi.dll": "2814edc3350cae6d34a75b7900df19752889c310ef34255c7316c4a95e1b183f",
      "dxcompiler.dll": "0689e2fee0a1c9df8662a59680469f24e2a3ad7ce6bbbcc9284f7a83172bfc59",
      "dxil.dll": "64be2368211d257a038a7d4ea6bb3e24c878eb8d588c7b8414fccf759b69c2ac",
      "d3d12-root-cbv-window.exe": "cb33b8c8ce83070aeea950bf535f95e62cdf3eef3964a1aa44413d5905d1b675",
      "d3d12-tables.exe": "3eca7b39d11f3d4a03cb2255348341ca55510fc9a5ecfe67b0135cadcbfa2866",
      "d3d12-draw.exe": "cbcee1dbc081b5a73150bfa62ae33a962c37410b87e8976a51b2ef8ddd551a45",
      "d3d12-indirect.exe": "9824674c7a74e34647a8492aa0843581d5d99936aad052d1dfa0dd3fbeef8d4e"
    },
    "unchanged": true,
    "cases": [
      {
        "case": "root-cbv-window",
        "passed": true,
        "exit": 0,
        "seconds": 14.95,
        "receipt": "d3d12-root-cbv-window PASS pci=10de:2704 words=72/72 window=65536 offsets=0,256,65280"
      },
      {
        "case": "tables",
        "passed": true,
        "exit": 0,
        "seconds": 20.222,
        "receipt": "d3d12-tables PASS pci=10de:2704 words=16/16 guards=4 heaps=2 rejected=3 placed-srv=16/16 range-copies=4 descriptor-mode=1"
      },
      {
        "case": "stream",
        "passed": true,
        "exit": 0,
        "seconds": 13.267,
        "receipt": "d3d12-stream PASS pci=10de:2704 slot=86 bytes=712 indices=16,32 half-red-blue=18 magenta=18 blue=220 rejected=4"
      },
      {
        "case": "indirect-scarlett",
        "passed": true,
        "exit": 0,
        "seconds": 22.504,
        "receipt": "d3d12-indirect PASS pci=10de:2704 sdk=0x20004 device=41 list=61 words=64/64 counts=direct,one,zero flush=global,range,wake-up(0x3000:0+0xa30000),zero-length,global-range budget=default,rejected2 mesh=0x00000000 rays=0x00000000"
      }
    ],
    "passed": true
  }
}
```


## Consolidated coverage after #92

The follow-up consolidates the duplicate #116 test into this PR.
Extend the original 24-word readback to five starts (0, 256, 65280,
65536, 65792) across three committed pages. Require all five physical
view imports: 64/128/128/64/128 KiB, respectively. The stricter
upstream runner now disables null substitution in positive cases.
Both the expanded root CBV control (120/120 words including guards)
and the null-disabled negative control pass on RTX 4080 with a fresh
provider built from the merged 9139f2f runtime. 224 Python tests pass.
No old GPU-fault reproduction, graphics CBV or retail claim.

```json
{
  "schema": 1,
  "scope": "Synthetic D3D12 provider controls; not retail execution",
  "adapter": "10de:2704",
  "proton": "1794497090 experimental-11.0-20261112-x86_64",
  "artifacts_sha256": {
    "d3d12_xs.dll": "0efe7d65f21a7b58bea17f67f46dfc8e4cc5eecbf91663d3109b446ffe13fa4b",
    "d3d12.dll": "fc2bbef02509fb11c334cc96dd211f64def94bb7b4eb82082a4808e6e0549543",
    "d3d12core.dll": "9356501320114773be279506644aabd9a1717ff451d480ac802a4fd4e94b2576",
    "dxgi.dll": "2814edc3350cae6d34a75b7900df19752889c310ef34255c7316c4a95e1b183f",
    "dxcompiler.dll": "0689e2fee0a1c9df8662a59680469f24e2a3ad7ce6bbbcc9284f7a83172bfc59",
    "dxil.dll": "64be2368211d257a038a7d4ea6bb3e24c878eb8d588c7b8414fccf759b69c2ac",
    "wine": "7a6de49c00d8ed2ba55c6967d3643c5ef729f5e562fb51e47e4f41c6cdb5c92a",
    "wineserver": "4c9afbd3bc3fc4229de03acc87f1abc69a9c1ab42918ef95406107b396a57486",
    "d3d12-root-cbv-window.exe": "daffa6d0555dd5878174bfab925ff0d5547507aec808b71920999a9451f7cfdc",
    "d3d12-null-usage.exe": "901884dd1dbdd45ac710f158b89cd792f5f7f9b7f895643897ff3a43bfdc8892"
  },
  "unchanged": true,
  "cases": [
    {
      "case": "root-cbv-window",
      "passed": true,
      "exit": 0,
      "seconds": 13.513,
      "receipt": "d3d12-root-cbv-window PASS pci=10de:2704 words=120/120 window=65536 offsets=0,256,65280,65536,65792",
      "null_fallback_enabled": false
    },
    {
      "case": "null-disabled",
      "passed": true,
      "exit": 0,
      "seconds": 15.603,
      "receipt": "d3d12-null-disabled PASS pci=10de:2704 rejected=1 valid=1",
      "null_fallback_enabled": false
    }
  ],
  "passed": true
}
```
