# Texture layout library (`xg_xs.dll`)

XG is the console's texture tiling library. The game imports two functions from it; the loader answers both
(`src/FiveStar.Loader/Shims/Xg`) with the same tiling the shipped `.ttpf` textures use ([textures.md](textures.md)).

## Call sites

Import slots 0x146793f60 (`XGCreateTextureComputer`) and 0x146793f68 (`XGSuggestSwizzleMode`), reached through the
thunks at 0x145f3ff40 and 0x145f3ff50.

| Call | Function | HRESULT tested | What the game does with the computer |
|---|---|---|---|
| 0x140344235 | 0x1403441f0 | no | slot 10 (tiling), then Release: builds the tile table of a reserved resource |
| 0x140979c75 | 0x140979c40 | yes | slot 2 (layout): keeps a mip's Size, Slice2DSize and PitchBytes; slot 9 later reads the mip back (0x140979e30) |
| 0x1416f7141 | 0x1416f7100 | no | slot 8 for every mip of every array slice, then Release: tiles row-major data into the resource |
| 0x145121a5e | 0x145121980 | yes | slot 2, then Release: caches one plane entry (0x6c8 bytes); 0x145121690 and 0x14555ea00 take a mip's address (slice x stride + Offset), Size, Slice2DSize and PitchBytes from it |
| 0x1455acdd0, inlined at 0x145151d98, 0x145151e7d, 0x1451a0ddb, 0x1451de676, 0x1451df086 | the computer of 0x140979c40 | return is an offset | slot 6: adds the offset to the tiled resource base to address one element (x and y divided by 4 for BC formats) |
| 0x144eccca2, 0x140c4b3ce | `XGSuggestSwizzleMode` | passed on | `(desc, 0, &desc->Layout)` when the asset asks for a suggested mode (mode byte 0xFF) |

## Description

`XGCreateTextureComputer(const desc*, computer**)` and `XGSuggestSwizzleMode(const desc*, UINT32, UINT32* layout)`
take the D3D12_RESOURCE_DESC layout, built at 0x144ecc940 (a copy at 0x140c4b000).

| Offset | Field | Values the game writes |
|---|---|---|
| 0x00 | Dimension | 2 (1D), 3 (2D), 4 (3D) |
| 0x08 | Alignment | 0 |
| 0x10 | Width (u64) | |
| 0x18 | Height | |
| 0x1C | DepthOrArraySize (u16) | |
| 0x1E | MipLevels (u16) | |
| 0x20 | Format | DXGI; D16 (0x37), D32 (0x28), D32S8 (0x14) become their typeless twins for some uses |
| 0x24 | SampleDesc | count, quality 0 |
| 0x2C | Layout | 0x100 + swizzle mode (0x100 for mode byte 0), 0x102, 0x10A, or the suggested value |
| 0x30 | Flags | D3D12 flags in bits 0-2, console flags 0x100000, 0x300000, 0x400000, 0x800000, 0x1000000, 0x4000000, 0x80000000 |

Swizzle modes are read with the RDNA address library numbering: 0 linear, 1-3 256 B, 4-7 4 KB, 8-15 64 KB,
16-19 64 KB, 20-23 4 KB, 24-31 64 KB blocks. `XGSuggestSwizzleMode` writes 0x100 + 27 (64 KB) when mip 0 holds 1 MiB
or more and 0x100 + 23 (4 KB) otherwise, the split the shipped textures show (every mip 0 of 1 MiB or more uses
64 KB blocks, every one under 128 KiB 4 KB blocks).

## Computer

A native object: vtable, reference count and the computed layout. Slots seen in the game:

| Slot | Method | Arguments after `this` |
|---|---|---|
| 1 | Release | |
| 2 | GetResourceLayout | `XG_RESOURCE_LAYOUT*` |
| 3 | UINT64 GetResourceSizeBytes | |
| 4 | UINT64 GetResourceBaseAlignmentBytes | |
| 5 | UINT64 GetMipLevelOffsetBytes | plane, mip |
| 6 | UINT64 GetTexelElementOffsetBytes | plane, mip, UINT64 x, y, z or slice, sample, `UINT*` bit offset (null) |
| 8 | CopyIntoSubresource | resource, plane, subresource, linear, row pitch, slice pitch |
| 9 | CopyFromSubresource | linear, plane, subresource, a UINT the game passes as 0, resource, row pitch, slice pitch |
| 10 | GetResourceTiling | as `ID3D12Device::GetResourceTiling` after the resource: tile count, packed mip info, tile shape, in/out count, first, tilings |

Slot 0 is taken as AddRef. The names follow the XDK method order (AddRef, Release, GetResourceLayout, GetResourceSizeBytes,
GetResourceBaseAlignmentBytes, GetMipLevelOffsetBytes, GetTexelElementOffsetBytes, GetTexelCoordinate, CopyIntoSubresource,
CopyFromSubresource, GetResourceTiling), which slots 2 and 6-10 match in arguments; the game was not seen calling 3-5.
Slot 6's mip is the wrapper's mip index (0x140979cce), its slice an override or the caller's z, its sample 0; offsets count
from the resource start, the base the game also passes to CopyFromSubresource. Offsets of a plane, mip or slice that
does not exist are 0. Slots 7 and 11-15 are stubs that log their first call and return E_NOTIMPL.
A subresource is mip + slice x mips; the plane is passed apart.

`XG_RESOURCE_LAYOUT` is 0x1B40 bytes (the game's stack buffers). Fields marked read are the ones the game reads; the
rest follow the XDK order and are filled for completeness.

| Offset | Field | |
|---|---|---|
| 0x00 | SizeBytes (u64) | |
| 0x08 | BaseAlignmentBytes (u64) | |
| 0x10 | MipLevels, 0x14 Planes | |
| 0x18 + 0x6C8 x plane | Plane entries (4) | read |
| plane + 0x08 | SizeBytes | |
| plane + 0x10 | Array slice stride | read |
| plane + 0x18, 0x20, 0x28 | Base offset, alignment, bytes per element | |
| plane + 0x38 + 0x70 x mip | Mip entries (15) | read |
| mip + 0x00 | SizeBytes | read |
| mip + 0x08 | OffsetBytes from the resource start (plane base included) | read |
| mip + 0x10 | Slice2DSizeBytes | read |
| mip + 0x18 | PitchPixels | |
| mip + 0x1C | PitchBytes | read |
| mip + 0x20 to 0x3B | Alignment, padded width, height, depth, width, height, depth (elements) | |
| 0x1B38 | Dimension | |

## Layout

Per plane: array slices one after another, each holding one mip chain. An element is a 4x4 block for BC formats and
a pixel otherwise; the block swizzles are those of [textures.md](textures.md) (a 256-byte block uses the first bits
of the 4 KB pattern).

| Rule | |
|---|---|
| Order in a slice | the mip tail block first, then the other mips smallest first, so mip 0 ends the slice |
| Non-tail mip | padded to whole blocks, blocks row-major |
| First tail mip | the first mip no larger than the block halved in width, with at most log2(block bytes) - 4 mips from it to the end (8 for 4 KB, 12 for 64 KB); a chain of one mip has no tail and starts at offset 0 |
| Tail slot of tail mip k | m = log2(block bytes) - 5 - k; it starts at 16 << m bytes when m > 6, else at m x 256 |
| Tail mip addressing | the whole block's swizzle, from the element whose index is the slot start |
| Linear (mode 0, or a 12-byte element) | mips in order from mip 0, rows padded to 256 bytes, mips on 512 bytes |
| Depth-stencil formats | a second plane of one-byte stencil after the depth plane, aligned to the block |
| 3D in a D or R mode, or with 256 B blocks | depth slices are laid out as array slices of the full chain |
| 3D in a Z or S mode with 4 KB or 64 KB blocks (thick) | one chain of bricks: each mip padded to whole bricks in x, y and z, bricks in x, then y, then z, smallest mip first, no tail. 4 KB bricks per element size 16x16x16, 8x16x16, 8x16x8, 8x8x8, 4x8x8 (the RDNA 1 KB brick doubled in y and z); 64 KB bricks four times the 1 KB brick on each axis. The 2- and 8-byte 4 KB brick swizzles are those of the shipped volumes ([textures.md](textures.md)); other sizes and 64 KB bricks take the missing x, z and y bits in turn |

GetResourceTiling uses 64 KB tiles: in a 64 KB mode the mips outside the tail are standard mips and the tail block
is the one packed tile; in other modes every mip is packed.

Checked on the dump: with these rules and the stored mip count of [textures.md](textures.md) (mip field plus `d1`
bits 0-2) the computed size equals the span of every 2D texture. Each mip read back through the layout correlates with the previous mip shrunk by half (BC1 endpoint brightness:
mean 0.90 for 4 KB and 0.97 for 64 KB standard mips, 0.67 to 0.94 for the first four tail mips; about 0 when the
tail slots are moved by 512 bytes). `dotnet test` re-tiles two shipped textures byte for byte.

## Contract with the D3D12 shim

Textures the game fills through the computer, and those it streams from `.ttpf` pages, sit in this console layout:
mip Offset and the slice stride above, block swizzles from [textures.md](textures.md). Memory handed to the D3D12
shim for a texture (a placed resource, an upload) must be detiled with the same layout (`XgAddress.Element` gives
each element's byte offset) before it goes to a PC resource, and a PC resource read back for the game must be tiled
the same way. Tile indices from GetResourceTiling count 64 KB tiles of this layout, not the PC's standard tiling.

## Unknown

- Slot 7 (GetTexelCoordinate by the XDK order) and the meaning of the fourth CopyFromSubresource argument; the
  `XGSuggestSwizzleMode` second argument.
- The plane and mip fields the game does not read; whether SizeBytes of a tail mip is the tail's or the mip's.
- The swizzle modes behind the `.ttpf` layout bases and the exact XG suggestion rule.
- Array order: the shipped arrays are stored level by level ([textures.md](textures.md)), while this layout puts
  slices one after another, as the single slice stride the game reads implies.
- Thick (3D) layouts: the brick swizzles of 1-, 4- and 16-byte elements and of 64 KB bricks, and the mip order and
  tail of volumes with mips. The size of a one-mip 2-byte volume is checked: a 256x32x8 R16_FLOAT texture in mode 5
  (0x105) streamed in story mode needs 0x40000 bytes; laid out as 2D slices it got 0x20000, the game read half its
  archive chunk and broke at 0x144ffd879.
- MSAA layouts and the console's pipe/bank XOR, which the shipped textures do not show.
