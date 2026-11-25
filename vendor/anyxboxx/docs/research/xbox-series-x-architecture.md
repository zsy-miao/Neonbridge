> Français : [architecture-xbox-series-x.md](../fr/recherche/architecture-xbox-series-x.md)

# Xbox Series X architecture

Research done on 21 November 2026 to guide the development of AnyXboxX. Every fact carries its source. **Verified** means the page was read. **To confirm** means the fact comes from a search snippet or a secondary source.

## Hardware

| Item | Xbox Series X ("Anaconda") | Xbox Series S ("Lockhart") |
|---|---|---|
| SoC | Enhanced TSMC N7, 360.4 mm², 15.3 billion transistors [CB] | to confirm |
| CPU | 8 AMD Zen 2 cores: 3.8 GHz, or 3.66 GHz with SMT [W] | 3.6 GHz, or 3.4 GHz with SMT [W] |
| CPU caches | 512 KiB L2 per core; 4 MiB L3 per CCX, 2 CCX of 4 cores [CB] | to confirm |
| GPU | AMD RDNA 2, 52 active CUs out of 56, 3,328 shaders, fixed 1.825 GHz, 12.155 TFLOPS [W] | RDNA 2, 20 CUs, 4.006 TFLOPS [W] |
| GPU cache, ray tracing | 5 MiB L2; 380 billion ray-box intersections per second [CB] | to confirm |
| Memory | 16 GiB GDDR6, 320-bit bus: 10 GiB at 560 GB/s and 6 GiB at 336 GB/s [W] | 10 GiB GDDR6, of which 8 GiB at 224 GB/s [W] |
| Memory for games | about 13.5 GiB, the rest is reserved for the system [W] | to confirm |
| Storage | 1 TB NVMe SSD (802 GB usable), 2.4 GB/s raw, 4.8 GB/s with 2:1 compression [W][XV] | 512 GB SSD (364 GB usable) [W] |
| Decompression | hardware block: general LZ (zlib) and BCPack for textures [W][XV] | to confirm |

Differences found between sources: 3.6 or 3.66 GHz with SMT; 15.3 or 15.4 billion transistors; ComputeBase cites a PCIe 4.0 ×2 ceiling of 4 GB/s, which differs from the 2.4 GB/s announced by Microsoft.

## Graphics features

- **Xbox Velocity Architecture**: SSD, hardware decompression, DirectStorage and Sampler Feedback Streaming. Microsoft presents SFS as an innovation built on the rest of the Velocity Architecture [XV, verified].
- **DirectX 12 Ultimate**: DXR ray tracing, Variable Rate Shading, mesh shaders, Sampler Feedback [XR, to confirm].
- On the software side, games use a console variant of Direct3D 12 ("D3D12.X") shipped with the GDK. It exposes the hardware more directly than the PC version [to confirm].

## Security and system

- **Physical attack model**: since the Xbox One, Microsoft assumes that flash memory, the disk and even DRAM can be read or modified. The SoC, designed with AMD, encrypts and verifies what it loads [TX, verified; Xbox One source].
- **Root of trust**: the Hot Chips 32 presentation cites an "HSP/Pluton RoT" and a "SPLEAP" protection against privilege escalation [HC32, to confirm].
- **Hypervisor**: an adapted Hyper-V separates a host system, a "system" partition (interface, applications) and an exclusive partition for the game [VT, to confirm].
- **XVD/XVC containers**: signed header, data encrypted with AES-XTS; the content key (CIK) is itself protected by a distribution key (ODK) [XVD, verified].

## What this means for AnyXboxX

1. **CPU**: the game runs as native x86-64. Unicorn is enough for a prototype; a playable emulator would run the code directly on the PC processor, like Wine.
2. **GPU**: this is most of the work. D3D12.X calls must be translated to Direct3D 12 or Vulkan, without public documentation to rely on.
3. **Memory**: the two bandwidth zones (10 + 6 GiB) do not need to be reproduced; a single heap is enough.
4. **Storage**: DirectStorage and BCPack can be emulated in software, at the cost of longer load times.
5. **Content**: game packages are encrypted with keys that only the console holds. AnyXboxX therefore only runs unencrypted programs, such as its demo.

## Sources

- [W] Wikipedia, *Xbox Series X and Series S*: https://en.wikipedia.org/wiki/Xbox_Series_X_and_Series_S (verified)
- [CB] ComputeBase, Hot Chips 2020 summary: https://www.computerbase.de/2020-08/xbox-series-x-hot-chips/ (verified)
- [XV] Xbox Wire, *A closer look at Xbox Velocity Architecture*: https://news.xbox.com/en-us/2020/07/14/a-closer-look-at-xbox-velocity-architecture/ (verified)
- [XR] Xbox Wire, RDNA 2 integration: https://news.xbox.com/en-us/2020/10/28/a-closer-look-at-how-xbox-series-xs-integrates-full-amd-rdna-2-architecture/ (to confirm)
- [HC32] Hot Chips 32, Microsoft presentation: https://hc32.hotchips.org/assets/program/conference/day1/HotChips2020_GPU_Microsoft_Jeff_Andrews_v2.pdf (to confirm)
- [TX] Tony Chen, BlueHat 2019, *The Xbox One story*: https://xboxoneresearch.github.io/wiki/_files/bluehat-2019-the-xbox-one-story/source.txt (verified)
- [VT] Vectree, Xbox system software: https://vectree.io/c/xbox-one-and-xbox-series-xs-system-software (to confirm)
- [XVD] Xbox One Research wiki, *Xbox Virtual Drive*: https://xboxoneresearch.github.io/wiki/operating-system/xbox-virtual-drive (verified)
