# Xbox-facing D3D12

The app-local `d3d12_xs.dll` creates guest devices for two Xbox release profiles and routes the four root-signature serialization and deserialization exports to desktop D3D12. The SDK guests exercise both paths on a selected hardware adapter.

The Rust device factory validates the guest creation record. Each guest COM object holds its native D3D12 interface. The command queue translates guest objects to native handles and presents through DXGI. [Device and resource slots](src/device.rs) · [Queue and command list](src/commands.rs) · [Presentation](src/present.rs)

The packaged `D3D12CreateDevice` constructs a 72-byte Xbox record and calls `D3D12XboxCreateDevice`. TuxBox takes the same route, preserving the guest device’s queue layout. Games can set and read the process debug flags with the reference mask `0x83f00001`. Device creation currently supports release flags and AutoHDR off. [Packaged factory and flag routines](../../evidence/matrix-awakens/d3d12-process-debug-flags.json) · [Frosty device trial](../../evidence/matrix-awakens/tuxbox-device-factory.json)

## Library functions and identifiers

Games can import all 32 data symbols in the Scarlett DLL, including the Xbox DMA command-list and graphics-unknown interface IDs. The guest checks all 512 bytes and resolves each symbol by import, name and ordinal. [Rust exports](src/guids.rs) · [Console comparison](../../evidence/providers/d3d12-guids.json)

The Xbox debug interface can set process flags. Games can read them back through `D3D12XboxGetProcessDebugFlags`. `D3D10CreateBlob` provides writable shader storage. The four legacy `DXGIX` entry points return `DXGI_ERROR_UNSUPPORTED`, matching the packaged Series DLL. The `d3d12-exports` check exercises these calls on Linux. [Guest](exports_guest.cpp) · [Original and translated results](../../evidence/providers/d3d12-exports.json)

The Series export `D3D12ZeroMemoryX` clears aligned CPU memory ranges. Tests against the packaged DLL cover overlapping ranges, optional workspace and invalid requests that must leave memory intact. GPU-assisted clearing remains open. [CPU clear comparison](../../evidence/providers/d3d12-cpu-clear.json)

The default export ordinals match the reviewed 54-export Scarlett DLL. Other builds can assign different ordinals to the same names. Use `python build.py d3d12 --d3d12-reference /path/to/d3d12_xs.dll` to take the ordinal assignments from a supplied library, then run `python tuxbox.py verify --only d3d12-guids --output build/d3d12-identifiers.json`.

## What runs on the GPU

Frosty displays the menu and video preview through this provider on Linux. With checkout-built platform and media adapters, keyboard input opens Settings and starts the Wake Up sequence. The graphics trace records a 712-byte pipeline stream at Xbox device slot 86, direct queue submissions and a 3840 × 2160 display buffer. [Independent run](../../evidence/matrix-awakens/independent-menu.json) · [Graphics trace](../../evidence/matrix-awakens/tuxbox-menu-startup.json)

| Guest check | Measured result |
|---|---|
| Graphics pipeline and draw | A vertex shader and pixel shader draw a triangle through Xbox slots. A 6 × 6 scissor changes 36 pixels and leaves the other 220 blue. |
| Device2 graphics pipeline stream | A 712-byte Xbox stream creates a pipeline for indexed triangles. Two draws bind 16-bit and 32-bit index views and fetch positions from a vertex buffer. The blend factor changes the red channel from 128 to 255 between draws. Each covers 18 pixels and the other 220 stay blue. [NVIDIA](../../evidence/providers/d3d12-stream-valve.json) · [Radeon](../../evidence/providers/d3d12-stream-radeon.json) |
| Mesh pipelines | Device2 slot 86 accepts Xbox mesh and amplification bytecode. Command-list slot 127 dispatches groups directly. Slot 59 executes indirect mesh records of type 107. The controls check group counts, payloads, output pixels and untouched guards on both adapters. [NVIDIA](../../evidence/matrix-awakens/mesh-command-list-nvidia.json) · [Radeon](../../evidence/matrix-awakens/mesh-command-list-radeon.json) |
| Linked retail shaders | Two geometry/pixel pairs and three vertex/pixel pairs create Xbox pipelines on NVIDIA and AMD. The checks cover Frosty’s five-target material pipeline and reject missing, misfiled and wrong-stage translations. [Shader pairs and controls](../../evidence/matrix-awakens/tuxbox-linked-materials.json) |
| Indirect compute | A GPU argument buffer controls dispatch. Counts of one and zero produce the expected writes and untouched words. For both texture-cache flush forms, dependent dispatches preserve the expected values across a 64-word readback. |
| Descriptor tables | A compute shader reads a constant buffer, buffer view and sampled texture, then writes a storage buffer. Both a null texture and a placed texture whose view was copied after command-list recording produced 16 correct outputs and four intact guard words. |
| Memory and copies | Two placed buffers share a 32 MiB heap at distinct offsets. The guest checks all 256 words after a GPU copy and a 3840 × 2160 RGB10A2 display allocation returns the expected green pixel. |
| Whole-resource and region copies | Xbox command-list slot 84 copies a buffer and all eight subresources of a two-layer texture. Slot 83 updates a two-texel region. The guest checks both GPU readback and the original tiled pages, including guards and padding. [NVIDIA](../../evidence/matrix-awakens/command-list-copy.json) · [Radeon](../../evidence/matrix-awakens/command-list-copy-amd.json) |
| Guest-backed textures | Slot 44 reads the Xbox mip and slice layout at the title’s virtual address. GPU readbacks cover neighboring textures, aliases, a six-face BC1 cube and 262,144 R8 texels. Replacing the backing after recording changes the next submission’s result. |
| Depth and stencil | A compute shader changes the depth plane through an R32 UAV. The committed and placed tests verify every depth and stencil value. [Depth readbacks](../../evidence/matrix-awakens/tuxbox-depth-stencil.json) |
| Movie surface copies | The compact Xbox copy list splits native NV12 into guest luma and chroma textures. Tests check both planes and reject stale frames and foreign resources. [NV12 readbacks](../../evidence/matrix-awakens/tuxbox-media-copy.json) |
| Frame pacing and presentation | Frame origins issue tokens. A completed host present signals a duplicated caller event. Two RGB10A2 frames passed readback and the display color check. Frosty's 10% late-frame threshold uses synchronized presentation. |
| Timing and queues | Timestamp and occlusion results resolve to a guarded readback buffer. A direct queue releases a fence that permits a separate copy queue to transfer 64 bytes. |
| Device queries | Seven Xbox feature records, batched allocation sizes and mip upload footprints are checked against the selected adapter and the packaged driver's record layouts. |

A byte at offset `0x18` in the guest device tells the title to copy descriptors through the device API. TuxBox copies valid ranges using single-descriptor host calls, including mixed range sizes. Frosty uses this path to bind UAVs during its compute clears. [Descriptor GPU checks](../../evidence/providers/d3d12-placement-valve.json)

The slot 25 allocation call passes its result buffer before the guest device. TuxBox translates that argument order and asks the host adapter to calculate sizes and alignments. Texture uploads use separate arrays for subresource footprints, row counts and unpadded row sizes. [Allocation ABI](src/device/allocation.rs) · [Desktop allocation contract](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nf-d3d12-id3d12device-getresourceallocationinfo%28uint_uint_constd3d12_resource_desc%29)

Frosty supplies some compute, vertex and pixel shaders with the portable bitcode stripped. At first boot, the launcher prepares known translations from local game files. The provider looks up the incoming shader by stage and SHA-256 when it creates a pipeline. Three Frosty compute clears passed GPU readback with distinct cache keys. Missing or broken translations preserve the host’s failure result. [Clear shader controls](../../evidence/matrix-awakens/tuxbox-compute-cache.json) · [Graphics pipeline creation](../../evidence/matrix-awakens/tuxbox-menu-startup.json)

For single-sample 2D textures and arrays placed at a guest virtual address, TuxBox uploads CPU changes when the game submits work. When the GPU copies into one of these textures, TuxBox writes the texels back in Xbox tile order before the guest fence completes. Two resources with the same address and layout share the GPU image. Overlapping resources with different layouts can read the updated pages after the fence. Simultaneous writes through conflicting layouts and rendering into guest-backed targets remain open. Run `python tuxbox.py verify --only d3d12-copy-resource --output build/copy-results.json` to repeat the copy checks. [Copy results](../../evidence/matrix-awakens/command-list-copy.json) · [Placement on NVIDIA](../../evidence/providers/d3d12-placement-valve.json) · [Placement on Radeon](../../evidence/providers/d3d12-placement-radeon.json)

For the two texture-cache flushes observed in Frosty, command-list slot 67 issues a global memory barrier on hosts with enhanced barrier support.

To repeat the graphics checks on one adapter, run the following from the repository root. Add `--adapter VVVV:DDDD` to select a specific GPU.

```sh
python build.py lanes
python tuxbox.py verify --only d3d12-features --only d3d12-queries --only d3d12-queues \
  --only d3d12-tables --only d3d12-placement --only d3d12-heaps --only d3d12-indirect --only d3d12-draw --only d3d12-stream \
  --only d3d12-guids --only d3d12-exports --only d3d12-frame-events --only guest-frame --only lanes
```

## Screen rectangles

Xbox screen rectangles supply three corners per primitive. TuxBox reconstructs the fourth corner before rasterization. The draw guest checks two rectangles with different corner orders, interpolated texture coordinates, primitive and instance IDs, and the pixels outside both rectangles. It exercises a descriptor pipeline with a direct draw and a stream pipeline with an indexed indirect draw. [Radeon and NVIDIA readbacks](../../evidence/providers/screen-rectangles.json) · [Xbox GDK rectangle example](https://github.com/microsoft/Xbox-GDK-Samples/blob/main/Samples/Graphics/PointSprites/Shaders/RenderRectListQuadVS.hlsl)

Build the guest with `python build.py d3d12`. Place the Windows x64 `dxcompiler.dll` and `dxil.dll` from the DXC distribution beside `build/d3d12-smoke/d3d12-draw.exe`. On Linux, select the Proton installation and test prefix with `PROTONPATH` and `WINEPREFIX`, then run both modes with the adapter's PCI ID.

```sh
umu-run build/d3d12-smoke/d3d12-draw.exe --rect --adapter 10de:2705
umu-run build/d3d12-smoke/d3d12-draw.exe --rect-stream --adapter 10de:2705
```

## Guest devices and frames

A retail executable imports `D3D12XboxCreateDevice` and passes a 64-byte creation record with SDK value `0x20002`. Its call sites select device slot 8 for a non-compute queue, slot 48 for a compute queue and slot 73 for a graphics command list. The fixture sets the two title-dependent creation fields to zero and supplies a DXGI adapter explicitly so each run can verify the selected GPU. The recorded retail factory call supplies a null adapter. Other SDK versions and option combinations need their own evidence. [Static call sites and executable hash](../../evidence/fortnite-device.json)

Frosty passes a 72-byte `0x20004` record and the default adapter. The factory accepts both this release record and the default used by the standard-named factory in the packaged DLL. A guest checks the Xbox compute queue at slot 48, the device returned by that queue, GPU fence completion and four debug-flag transitions. It also obtains the adapter from the guest’s DXGI interface, checks its identity and retrieves its parent factory. [Microsoft’s DXGI adapter contract](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/nf-dxgi-idxgidevice-getadapter) · [Frosty call sites](../../evidence/frosty-device.json) · [Proton Experimental](../../evidence/providers/d3d12-factories-valve.json) · [GE-Proton](../../evidence/providers/d3d12-factories-ge.json) · [Radeon 610M with RADV](../../evidence/providers/d3d12-factories-radeon.json)

| Suite | What the guest checks |
|---|---|
| `guest-frame` | `d3d12-factory.exe` exercises the default and explicit `0x20004` factories, process debug flags and compute queue fences. |
| `guest-frame` | `xbox-frame.exe` requests a direct queue at slot 8 and a compute queue at slot 48. It signals and waits on a separate compute fence. On the direct queue it clears one 37 × 13 RGBA8 texture, snapshots it, then clears a 208-pixel rectangle and snapshots it again. After the second submission completes, it compares every channel of all 962 pixels and checks three guard regions. |
| `guest-frame` | `xbox-frame-negative.exe` creates two guest devices on one adapter. It checks rejection of foreign allocators and fences, invalid copies, a mixed-device batch and an open list. A valid copy and completed fence after a failed recording verify list recovery. |
| `present` | `xbox-frame.exe --present` also submits the texture via queue slot 30 (`PresentX`) after each clear. A single-plane RGBA8 copy feeds a double-buffered DXGI swap chain. The guest samples all four corners and the center of the displayed window on both frames. The 4K RGB10 test repeats this with a host-owned window, an oversized title window and a 960 × 540 title window. The client area fits the monitor and the GPU still renders all 3840 × 2160 pixels. [Window sizes and screen samples](../../evidence/matrix-awakens/tuxbox-present-sizing.json) |

The guest-frame and presentation suites inspect the guest executables' PE imports before launch. Each must request the Xbox factory from the adjacent DLL and leave desktop D3D12 device creation to the provider. Passing results apply to the homebrew calls exercised on that adapter and provider. [Run results](../../docs/RESULTS.md)

The same guest binaries exercised the Rust provider and its C++ predecessor on Linux. Both passed the 962-pixel readback, command-error checks and Lanes replay on NVIDIA and AMD. Both displayed two frames on NVIDIA with Proton Experimental. On the isolated Xvfb display, AMD PresentX returned `0x80070057`, and GE-Proton on NVIDIA sampled incorrect colors intermittently with both providers. [Provider hashes, exports and paired results](../../evidence/d3d12-rust-comparison.json)

Lanes exercises the Xbox compute pipeline, root signature and buffer bindings to generate a course on the GPU. It uploads its board through the guest command list and presents through queue slot 30. When a title supplies no window, the provider creates one and runs its message loop. Closing that window ends presentation with `DXGI_ERROR_ACCESS_LOST`. [Lanes guest and checks](../../examples/lanes/README.md)

## Root-signature serialization

The `graphics-abi` suite runs `d3d12-root-smoke.exe`. Its `D3D12SerializeRootSignature` import resolves through a PE forwarder in `d3d12_xs.dll`. The fixture supplies four root constants at `b5, space2` and a root UAV at `u3, space4`. The Xbox-facing deserializer checks the version 1.0 descriptor layout and bindings. The guest uses desktop device and command-list interfaces to dispatch an original DXIL compute shader twice. It checks 48 output words and 16 guards. [Shader](affine.hlsl) · [Microsoft serialization contract](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nf-d3d12-d3d12serializerootsignature) · [Reviewed library exports](../../docs/LIBRARIES.md)

The `d3d12-signatures` suite calls the versioned serializer with 1.0 root constants and a 1.1 descriptor table. It checks both deserializers after releasing their source blobs, including shader registers, descriptor flags and truncated-input errors. The original console DLL on Proton returns `E_NOINTERFACE` at the first serialization call in this standalone guest. [Signature results](../../evidence/providers/d3d12-signatures.json)

## Build and verify

Use an x64 Visual Studio developer prompt on Windows, or clang-cl, lld-link and the xwin SDK on Linux. Put the host's `dxc` executable on PATH, set `DXC`, or pass `--dxc PATH` to the build. Run from the repository root. [Desktop build setup](../../graphics/README.md) · [DXC releases](https://github.com/microsoft/DirectXShaderCompiler/releases)

```sh
python build.py d3d12
python tuxbox.py verify --only graphics-abi --only guest-frame --output build/d3d12-results.json
```

The presentation check needs a visible, unobscured window. On Linux, use an isolated Xvfb display with a hardware Vulkan driver that supports presentation to it. Use a dedicated Proton prefix for the display so an existing Wine session cannot attach the guest to another desktop.

```sh
xvfb-run -a -s "-screen 0 1280x720x24" python tuxbox.py verify --only present --prefix build/present-prefix --output build/present-results.json
```

On Windows, run the same suite on the interactive desktop with `python tuxbox.py verify --backend native --only present --output build/present-results.json`. Add `--adapter VVVV:DDDD` to any verification command to select a GPU by hexadecimal PCI vendor and device IDs. The JSON records the provider, binary hashes, exit status and receipt for each gate.
