> Français : [xbox2windows.md](../fr/recherche/xbox2windows.md)

# Xbox2Windows: what we take

[Xbox2Windows](https://github.com/R0Y00/Xbox2Windows) (R0Y00, MIT licence) studies static recompilation of Xbox Series X|S software to Windows x64: CPU functions translated to C++ and then to Windows DLLs, gfx1013 pixel shaders translated to HLSL, BC7 textures recovered and displayed, script state replayed. Each result is checked on hardware GPUs and on WARP, and a community GPU bench covers more than 40 cards. State recorded on 21 November 2026 (commit `18d57d9`).

## Taken into AnyXboxX (in Rust, `rust/anyxboxx-texture`)

| Item | Their source | Our port | Verification |
| --- | --- | --- | --- |
| BC7 decoding (8 modes, partitions, rotation, index selection) | `tools/bc7_blocks.py` | `src/bc7.rs` | their hand-computed blocks, their field offsets, and their two SHA-256 fingerprints of the 512-block set reproduced identically |
| GFX10 `SW_4K_S` layout (4 KiB tiles, 16 × 16 blocks) | `tools/bc7_tiles.py` | `src/tiles.rs` | bit order y0 y1 x0 x1 y2 x2 y3 x3 tested position by position |
| Mip tail shared inside a 4 KiB tile | `linearize_4k_s_tail` | `src/tiles.rs` | level origins and bounds |

The command line `anyxboxx_texture bc7 <blocs.bin> <largeur> <hauteur> <sortie.png> [--4k-s]` writes a PNG; a Python test reads it back independently (zlib, CRC). These building blocks serve issue #14 (`xg_xs`, console texture layout).

## Not taken, and why

- **BCPack**: their `tools/bcpack_probe.py` tests a rANS coding hypothesis on the 48-byte header of a stream; it is not a decoder, and the model of the variable lanes is still open (their issue #15). The "no zlib" blocks in Codex's notes on ETPF ([ETPF_ANALYSIS.md](package/ETPF_ANALYSIS.md)) may be of this kind, without proof.
- **CPU recompilation and FidelityFX**: a different approach from AnyXboxX's native execution; their fixtures remain a useful reference for future tests.
