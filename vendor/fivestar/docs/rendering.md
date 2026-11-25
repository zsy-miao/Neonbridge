# Rendering

`src/FiveStar.Graphics/Rendering` holds the small D3D12 pieces that draw with translated programs:
`CommandContext` (queue, list, fence), `ResourceArena` (upload buffers, textures), `BindlessHeaps` (the two
shader-visible heaps), `RenderTarget` (offscreen target with readback), `DepthTarget` (D32_FLOAT) and
`HardwareSampler` (RDNA S# to D3D12 sampler). `PipelineBuilder.Graphics(program, state)` takes blend, rasterizer and
depth-stencil state.

## Depth

The game's main passes use reverse-Z: DB_DEPTH_CONTROL tests GREATER_EQUAL (the vehicle passes) or GREATER, and only
shadow and linear-depth passes use LESS or LESS_EQUAL ([pipeline-cache.md](pipeline-cache.md)). `DepthConvention`
holds the direction: the clear value (the far plane: 0 reversed, 1 forward), the test and the depth of a point.

`CacheStateMapping.DepthTested` keeps a pipeline's own depth state when it tests depth and takes its convention from
the test. The immediate-mode passes test nothing, so the renderer turns the test on with write and GREATER_EQUAL, the
main passes' convention. `UnlitRenderer` clears a `DepthTarget` to the convention's far plane and draws with it.

## First frame

```
fivestar graphics draw-sample <out.png> [--dump <folder>] [--programs <fssm folder>] [--texture <file.ttpf>]
                              [--index N] [--adapter <index|name|warp>] [--work <dir>] [--debug-layer]
```

Draws a game texture with the game's own immediate-mode unlit pass, offscreen, and writes a PNG:

1. Loads the pipeline cache from `shaders.rpf` ([pipeline-cache.md](pipeline-cache.md)) and takes the first pipeline
   running `VS_TransformUnlit_IM` + `PS_TexturedUnlit_IM` (pipeline 0 in the reference dump).
2. Translates both programs to DXIL (as `shader translate`, into `--work`, default `<out>-shaders`).
3. Builds the pipeline with the bindless root signature, the cache's blend and rasterizer state, a depth test (see
   Depth), the cache's input layout and render-target format (R8G8B8A8_UNORM).
4. Reads mip 0 of a texture (default `data/ttdb/ui/platform.ttpf` #0, BC7) and draws it twice: once at its own size,
   once as a strip with a red-to-blue vertex colour at half alpha, alpha-blended over dark grey.

`--debug-layer` reports D3D12 validation errors (exit code 1 when there are any). The test
`DrawSampleTests` renders an uncompressed texture on WARP and checks every pixel of the quad against texel over
clear colour, and draws a near quad before an overlapping far one to check that the near one stays in front.

Pipeline 0's state: blend SRC_ALPHA / INV_SRC_ALPHA on colour and alpha, back faces culled with
FrontCounterClockwise (the cache's low-confidence guess; the sample's quads are wound to suit it), depth off in the
cache.

## Binding contract

What the runtime must provide for a translated program, as found for this pass:

| Item | Content |
|---|---|
| Root parameter 0 | CBV `b0` with the 128 user-data dwords (`g_user4`); user SGPR n is dword n |
| Heap slot 0 | `ByteAddressBuffer` over the GPU-memory window; a 32-bit pointer in user data is a byte address in it |
| Heap slot 1 | writable view of the window (only programs that store through pointers use it) |
| Any other slot | whatever a descriptor's dword 0 names |
| Sampler slot 1024 to 2047 | the program's literal S#s, each at the slot its words give (`// static sampler <slot>: S# = ...[ compare]` lines in the HLSL) |
| Vertex input | translated vertex shaders read `ATTR0..n`; element n of the cache's input layout becomes `ATTR` n with the cache's format, slot and offset |

Both programs of the pass read only user SGPR s8: the low half of a pointer whose high half the code sets to 0. It
points at a table of descriptors:

| Table offset | Read by | Descriptor | Contents of the resource |
|---|---|---|---|
| 0x00 | PS | V# (4 dwords) | dword 0 scales alpha (squared by the code), dword 3 scales colour |
| 0x20 | VS | V# | 4x4 object matrix, row vectors: rows 0-2 multiply x, y, z, row 3 is added |
| 0x40 | PS | T# (8 dwords) | the texture |
| 0x60 | VS | V# | view constants; the 4x4 view-projection matrix (row vectors) at byte 0x160 |

The pixel shader samples with a literal S# `00000400 00c00000 18f00000 00000000`: wrap on all axes, 4x anisotropic,
linear mips, LOD 0 to 12. Output: `colour = texture.rgb * vertex.rgb * c[3]`, `alpha = texture.a * vertex.a * c[0]^2`.

The draw uses one table for both stages, since one root CBV serves every stage; their offsets do not overlap here.
On the console each stage has its own user-data registers, so another pass may need a table (or user data) per
stage. The values the sample puts in the tables are its own: constants that make the texture appear texel for texel.
A draw with several textures gets one table per texture, 0x80 bytes apart, and sets s8 per batch; the buffers are
shared.

The cache's shader metadata (tag 0x41 entries) lists the same bindings: root parameter 0 is the table at s8 with
t13 at dword 0 and t0 at dword 16 (PS), t7 at dword 8 and b0 at dword 24 (VS); s14 is static sampler 2
([pipeline-cache.md](pipeline-cache.md#bindings)).

## Shader fixes this needed

- Compressed colour exports (`exp mrt0 v0, v0, v1, v1 compr`): LLVM prints the two packed registers as
  `src0, src0, src1, src1`; the translator read the first register twice, so blue and alpha were wrong in every
  pixel shader that exports 16-bit colour.
- VS / PS linkage: D3D12 links stages by semantic and register. Vertex shaders now declare their parameters in
  number order before the unpacked f16 copies, and pixel shaders declare every input register of their signature,
  read or not, with the f16 halves last.

## Models

```
fivestar graphics draw-model <out.png> --archive <file.rpf> --from 0xOFFSET [--name TEXT] [--lod N] [--yaw DEG]
                             [--pitch DEG] [--size N] [--plain] [--paint RRGGBB] [--rim 0xOFFSET --tyre 0xOFFSET]
                             [--dump <folder>] [--programs <fssm folder>] [--adapter <index|name|warp>] [--work <dir>]
                             [--debug-layer]
```

Draws a drawable ([models.md](models.md)) with the same unlit pass:

1. Carves the archive from `--from` and takes the first drawable whose name contains `--name`.
2. Takes the geometries of level of detail `--lod`. The shader with the most triangles gets its `DiffuseTex`, found by
   hash in the dump's `.ttpf` files; sRGB formats are read as their UNORM twins, so the stored values reach the UNORM
   target unchanged. The other geometries, and all of them without a texture or with `--plain`, are drawn in the
   `--paint` colour (default white).
3. With `--rim` and `--tyre` (offsets of a `rim_*` and a `tyre_*` drawable), a vehicle fragment gets the pair on every
   wheel bone, scaled to its tyre cylinder and mirrored on the right ([models.md](models.md#fragments)). Rims are
   drawn light grey and tyres dark grey: like the paint, their `DiffuseTex` is a placeholder.
4. Turns the model by `--yaw` (about z, default 35) and `--pitch` (default 20, looking down) and fits it into a
   square target (`--size`, default 768) with an orthographic view.
5. Each vertex carries its depth between the nearest and farthest point of the model in z, in the pass's convention,
   so the depth test hides what is behind. The CPU drops back faces (winding flipped for mirrored parts) and winds the
   rest to suit the cache's front face. The vertex colour is a Lambert term from the decoded normals.

Before the depth target, triangles were sorted by their centres; on `dominator9_hi` underbody parts showed through
the body.

## Vehicle material pass

How far the game's own vehicle material got. The pipeline cache has 25 pipelines per variant pairing a
`PS_Vehicle*Deferred` pixel shader (`LiveryOffOnly`, `LiveryOffComplexDirtOff`, `...DetailMapsOff...`, `_NoDeform`)
with a mesh shader, `MS_VS_TransformD` (rigid) or `MS_VS_TransformSkinD` / `MSR_VS_TransformSkinD` (skinned; the
vehicle bodies are skinned). All of them write six targets (a G-buffer), test GREATER_EQUAL and cull back faces.
`shader translate` turns `PS_VehicleLiveryOffOnlyDeferred_7095f810`, `MS_VS_TransformD_7095f810` and
`MS_VS_TransformSkinD_34f92402` into DXIL without errors.

What the programs read:

| User SGPR | Pixel shader | Mesh shaders |
|---|---|---|
| 8 | table of 4-dword descriptors at 0x1C0 to 0xB20 and T# at 0xE20, 0xE60, 0xFE0, 0x1580 | descriptors at 0xA0 to 0xB20 |
| 9 | table at 0x20 to 0x120 | table at 0x00 to 0x120 |
| 10 | one V# at 0 | one V# at 0 |
| 11, 12, 13 | | read, not traced |
| 14 | the material's byte offset into the buffer whose V# is at s8 + 0xAC0 | |
| 15 | address of an array of T# | read, not traced |
| 16 | 4-dword descriptors at 0x400, 0x430, 0x470, 0x4B0 | |

The pixel shader fetches a material texture in three steps: a u32 from the buffer at s8 + 0xAC0, at byte s14 + k,
is the T#'s byte offset in the array at s15; dword 0 of that T# indexes the heap. The k read (0x0C, 0x10, 0x20, 0x28,
0x30) are the `slot * 4` values of the drawable's texture parameters ([models.md](models.md#vehicle-textures)), so the
runtime keeps one u32 per texture parameter per material.

Not done: the meshlet and vertex buffers the mesh shaders expect at s11 to s13, the contents of the s8, s9 and s16
tables, and a lighting pass to turn the G-buffer into colour. `draw-model` therefore still draws with the unlit pass.

## Unknown

- What s0..s7 hold for these programs (they are not read).
- The meaning of the PS buffer's dwords 1 and 2 and of the view buffer before byte 0x160.
- Whether the console binds one table for both stages.
