# FSSM programs

An FSSM blob is the native RDNA program of one shader (the XDAT section 0x0D of its DXBC container, see
[shader-containers.md](shader-containers.md)): a header with the hardware shader configuration, the machine code and
metadata after it. Decoder: `src/FiveStar.Shaders/Hardware`.

Offsets below are file offsets (magic `FSSM` at 0). Offset *values* stored in the header are relative to file
offset 0x10. All integers are little-endian.

## Generic block

| Offset | Size | Field |
|---|---|---|
| 0x00 | 4 | `FSSM` |
| 0xC0 | 8 | code block offset (file offset = value + 0x10) |
| 0xC8 | 8 | code block size |
| 0xD4 | 4 | flags: bit 5 wave32; bits 16..23 user SGPR count; bit 2 VS calls a fetch shader; bits 24..31 SGPR pair holding the fetch-shader PC |
| 0xD8 | 4 | first VGPR written by the fetch shader (4 when used) |
| 0xF8 | 4 | number of resource metadata records after the code |
| 0x100 | 1 | hardware stage: 3 GS, 4 VS, 5 PS, 6 CS, 7 NGG (mesh) |
| 0x104 | 4 | low byte: RSRC1.FLOAT_MODE (0xC0) |
| 0x10C | 4 | LDS bytes requested (compute) |
| 0x110 | 4 | low 16 bits VGPR count; bits 20..31 SGPR count including VCC |

VS, NGG and GS programs always run as wave64; PS and CS use the wave32 flag.

**Code block.** 24 bytes carrying the shader hash, then the program, then `s_code_end` (0xBF9F0000) padding. The
program is disassembled from block + 0x18 with the trailing padding removed.

## Pixel shader block (stage 5)

| Offset | Size | Field |
|---|---|---|
| 0x138 | 32 × 8 | interpolant table: (u32 a, u32 b) pairs, `a` = index << 24 \| mask << 16 \| semantic << 8 \| 6 |
| 0x238 | 4 | SPI_PS_IN_CONTROL.NUM_INTERP (at least the highest attrN + 1) |
| 0x23C | 4 | SPI_SHADER_PGM_RSRC2_PS: bits 1..5 user SGPRs, bit 25 load collision wave id (POPS) |
| 0x248 | 4 | SPI_PS_INPUT_ENA (equal to ADDR) |
| 0x24C | 4 | DB_SHADER_CONTROL: bit 0 Z export, bits 4..5 Z order, bit 6 kill enable, bit 16 primitive-ordered shading (POPS) |
| 0x250 | 4 | CB_SHADER_MASK: 4 bits per render target |

Input VGPRs are packed from v0 in SPI_PS_INPUT_ENA bit order, skipping disabled inputs:

| Bit | Input | VGPRs |
|---|---|---|
| 0, 1, 2 | persp_sample, persp_center, persp_centroid | 2 each |
| 3 | persp_pull_model | 3 |
| 4, 5, 6 | linear_sample, linear_center, linear_centroid | 2 each |
| 7 | line_stipple | 1 |
| 8..11 | pos_x, pos_y, pos_z, pos_w | 1 each |
| 12 | front_face | 1 |
| 13 | ancillary | 1 |
| 14 | sample_coverage | 1 |
| 15 | pos_fixed_pt | 1 |

## Vertex shader block (stage 4)

| Offset | Size | Field |
|---|---|---|
| 0x2BC | 4 | VS_VGPR_COMP_CNT: 0 = v0 only, 3 = v3 (InstanceID) loaded too |
| 0x29C | 4 | number of param exports (likely) |
| 0x138 | 160 | fetch table (with the fetch flag): 5-byte entries {type (6), attribute, mask, first VGPR, components − 1}; type 0 ends it |

A fetch-shader program starts with `s_swappc_b64 s[4:5], s[4:5]`, a call into a driver-generated subroutine that
loads each table entry's attribute into its VGPRs (from v4, 4-aligned).

## Compute block (stage 6)

| Offset | Size | Field |
|---|---|---|
| 0x138 | 4 | COMPUTE_PGM_RSRC2: bit 0 scratch, bits 1..5 user SGPRs, bits 7/8/9 group id x/y/z enable, bit 10 group size enable, bits 11..12 thread id component count − 1, bits 15..23 LDS size in 512-byte granules |
| 0x13C / 0x140 / 0x144 | 3 × 4 | numthreads x, y, z |

## NGG block (stage 7)

| Offset | Size | Field |
|---|---|---|
| 0x55C | 4 | 0x00800080 in every program |
| 0x560 | 4 | low 16 bits 128; bits 16..23 user SGPR count |

## Geometry block (stage 3)

| Offset | Size | Field |
|---|---|---|
| 0x148 | 8 | offset of the second program block, the driver's VS copy shader (file offset = value + 0x10) |
| 0x150 | 8 | its size |

The copy-shader block is disassembled whole, as wave64.

## Preloaded registers

A register read before any write is a hardware input:

| Stage | SGPRs | VGPRs |
|---|---|---|
| PS | s0.. user data; s[user] PRIM_MASK; s[user + 1] POPS collision wave id when POPS is on | input layout above |
| VS | s0.. user data (for the fetch shader: s[0:1] vertex-buffer table, s2 base vertex, s[4:5] its PC; [pipeline-cache.md](pipeline-cache.md#shaders)) | v0 VertexID, v2 PrimitiveID, v3 InstanceID, fetched attributes |
| CS | s0.. user data, then group id x, y, z (enabled ones, in order), then group size | v0 / v1 / v2 thread id x / y / z (separate) |
| NGG | s0..s7 system (s3 bits 24..27 wave index in the group), user data from s8 | v5 ES vertex id |
| GS | s0..s7 system (s3 wave info), s[8:9] ring table, user data from s8 | v0, v1, v3 ES vertex offset pairs, v2 primitive id, v4 instance id |
| Copy shader | s[0:1] ring table | v0 vertex offset |

## Command

```
fivestar shader disasm <program.fssm> [--count N] [--llvm-disassembler]
```

Prints `address size text` for each instruction (branch targets as `@T0x<address>`). The code is decoded in-process
(`RdnaDecoder`, see [shader-translation.md](shader-translation.md)). With `--llvm-disassembler` the bytes are instead
assembled into an object file with `llvm-mc` and listed with `llvm-objdump`, CPU `gfx1013`; wave64 programs add
`--mattr=+wavefrontsize64` (otherwise lane masks decode as wave32). The LLVM bin folder comes from `--llvm <folder>`,
then `FIVESTAR_LLVM`, then a default install folder when it exists, then `PATH`.

## Unknown

- The byte at 0x101 (an API-level stage id: PS 1, VS 0, CS 3, NGG 6, GS 2).
- 0x29C as the VS param export count (matches most programs, not all).
- The meaning of the PS interpolant table flags and the VS output table at 0x1D8.
- The scratch size; programs that spill set up flat scratch themselves.
- The NGG fields at 0x55C and 0x560 beyond the user SGPR count.
- The rest of the GS header after the copy-shader block.
