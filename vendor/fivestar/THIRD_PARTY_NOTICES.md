# Third-party notices

Five Star Recomp is licensed under the GNU General Public License, version 3 or later (`LICENSE`). Parts of it are
based on the projects below, used under the licenses quoted with them.

## Leonida

https://github.com/ItsNotRexed/Leonida, GPL-3.0. Its notes on the console D3D12.X list (slots 146 and 148, the
device byte at +0x18), the create-device and present parameter names it collected from public sources, the
WaitFrameEventX return codes and the BVH header and instance-node fields were leads; each was checked against the
game's executable or shaders before use (docs/d3d12x-shim.md, docs/raytracing.md), and the instance node's 0x00 and
0x70 fields are taken from it unverified. Its BCPack notes (`docs/textures.md`) corroborated the lane widths of codecs
3 to 7 and the BC4 and BC5 distance and prediction rules, which were checked on the dump. Its note that an Intel host
needs the four Zen 2 instruction families emulated was the lead for docs/cpu.md; the census and the emulation are our
own. Its hardware measurement of the V# OOB_SELECT range checks (docs/buffer_bounds.md) defines the rules in
docs/shader-translation.md (Range checking). No code was copied.

## VIctory

https://github.com/marvisak/victory, revision `ffe310eb2098e52169f4c44db5ffdabf43a72622`, GPL-3.0. The BCPack block
layer of every codec in `src/FiveStar.Formats/Ttpf/BcPack` (`BcPackTables`, `BlockRecord`, `PlainBlockAssembler`,
`Bc6HBlockAssembler`, `Bc7BlockAssembler`) follows the mechanism of its decoder (`crates/victory-formats/src/bcpack.rs`):
the layout of its table file, the two back references and the dictionaries, the rank tables, the BC7 control words
and the lane meanings of BC3, BC6H and BC7. It is written in our own design on our lane decoder.

The tables themselves (`bcpack-tables.bin`) are not covered by VIctory's licence; they are format data of Microsoft's
BCPack. Five Star Recomp does not distribute them in any form. The user supplies the file (`fivestar setup
--bcpack-tables <file>`), and without it BC3, BC6H and BC7 do not decode. The rANS model frequencies in that file equal
our `mdu_models.json` (silicon-rans, below); its BC1 2x2 rank table gives the same codes as `bc1_quad_table.json`, and
its BC1 dictionary differs from `bc1_window.json` in 54 of 255 slots, by at most 3 in a field. With the tables, BC1 decodes through
their dictionary; our fitted tables remain the fallback without them.

Its pipeline handling (a host pipeline made on the first draw rather than at creation, and a list of the last run's
pipelines started during loading) was the idea behind building pipelines on first use and the pipeline journal
(docs/d3d12x-pipelines.md). The design and code are our own.

Its crash in the transient-pool walk (0x144e9b54a, from stale copy-backs of shader-written memory) and its command-list
object layout (state written up to +0x564) were leads for docs/d3d12x-shim.md, checked against our shim and the
executable. Its CopyPageMappingsBatchX (`crates/victory-gpu/src/queue.rs`) gave the name and argument layouts of
queue slot 31 (docs/physical-memory.md); the implementation is our own. No code was copied.

Its command-list table named the console copy slots (CopyMemoryToMemoryX, CopyTextureRegionX, CopyResourceX; docs/d3d12x-shim.md),
and its rule that a texture the host GPU copied into keeps the host's contents was the lead for `GpuTexture.CopiedInto`
(docs/gpu-resources.md). Its report that dead-code removal and constant folding cut its host instruction count by
about 43% led to the DXIL backend's folding and value numbering (docs/dxil-backend.md). The code is our own.

Its audio modules were leads for the loader's audio (docs/audio-runtime.md): the xapu.dll command and 0x50-byte result
layout (identity echo, samples written, the first-frame and frame-count fields), the note that the game's mixer sleeps
about 4 ms per 5.3 ms block and needs millisecond timers (and that Windows 11 drops them for a hidden window unless
the process opts out), and recording through process loopback from a child process. Each was checked against the
executable or a boot; the code is our own.

Its DXIL emitter (`crates/victory-shader/src/emit/`, revision `ffe310eb2098e52169f4c44db5ffdabf43a72622`) showed that D3D12 accepts a container whose
digest the producer computes itself, with no dxil.dll signing. The DXIL backend (`src/FiveStar.Shaders/Dxil`,
docs/dxil-backend.md) learned these facts from it: the modified MD5 of the container digest (the bit count first in
the last block, `2 × bytes + 1` last), the unabbreviated LLVM 3.7 records DXIL accepts, the attribute groups and
metadata kinds a module declares, the PSV0 runtime-info layout of validator 1.6, and its note that the AMD driver
wants a loop's exiting block to be its latch (docs/dxil-backend.md lists it as open). The backend's design (a compiler for the translator's HLSL, SSA built while lowering) and its code are
our own.

For the graphics stages the backend also learned from it: the 32-byte ISG1/OSG1/PSG1 rows with the never-written
mask of outputs, the 16-byte PSV0 signature elements with the string and semantic index tables, the signature
metadata operands, the mesh state property, and declaring every output dependent on every input in the view-ID state
instead of tracing dependencies. Register packing, the order of signature rows and names, and the remaining layouts
were checked against dxc's containers (`fivestar shader dxil --compare`); the code is our own.

## victory-gta6

https://github.com/fxceless/victory-gta6, GPL-3.0-or-later, a fork of VIctory that reaches the prologue's gameplay. Its
changes were a source of mechanisms, each written anew in our design:

- Queue synchronisation (docs/d3d12x-shim.md, Memory waits): command lists cut at the Wait32/Wait64 slots and the
  parts run in order once the value is in memory (list.rs::cut, queue.rs::submit_cut / release_held), held waits
  released at a present, indirect arguments copied and clamped on the GPU before they run (indirect.rs, its
  bounds), and memory values read without VirtualQuery (its measurement of about 8 ms per query in the
  game's 1.1 GB allocation).
- Shader translation (docs/shader-translation.md): that `image_gather4_l` must read the named mip level (four loads in
  gather order), that an f16 operation reads an inline float constant as its f16 value, and per-stage loop caps
  against runaway loops.

Its `console_insn*.rs` files are GPL-2.0-only code from XCL and were not used. No code was copied.

VIctory's issues and pull requests were leads too (facts only, each checked against our translator): that
uninitialised or out-of-range mesh primitives hung NVIDIA's mesh pipeline (#587, which initialises them all and bounds
the outputs; our direct exports now do the same, docs/shader-translation.md), that wave64 mesh programs run as two
32-lane waves on NVIDIA (#778, #1216, #2176), and that the fog apply pass `CS_ApplyCombinedDeferred` multiplies the
lit HDR by a transmittance input another pass writes, so a missing writer erases the lighting (#2045).

## victory-fork

https://github.com/fxceless/victory-fork, revision `a26859fc5f512e0db0d7522d3da821c038378ab7`, GPL-3.0, a fork of VIctory
(its README credits upstream VIctory and its contributors). Facts from it, each written anew in our design
(docs/d3d12x-shim.md, Descriptor window; docs/shader-translation.md): the view and the sampler heap are separate
spaces each based at 0, a table's value is its handle minus the start of its own heap, and which heap a descriptor
load reads depends on the instruction's use (a sampler slot or a resource slot), not on the address; descriptor
indices clamped to the heaps (a view past the end to a null one, a sampler to the last), without which NVIDIA hung
and AMD faulted; and that a raw view's first byte must be a multiple of 16. No code was copied.

## ztrx

https://github.com/Zettrox/ztrx (`docs/fork-ray-tracing.md`), GPL-3.0, a fork in the VIctory family. Facts from its
notes, for docs/bvh-builder.md: the title's ray tracing is its own compute traversal with only the node test in
hardware, its dynamic structures come from the 52 builder programs of `XboxBvhBuilder.dll`, those programs reach title
memory by raw address, and running the builder with only some of its pipelines built wrote garbage into title memory
and crashed the title (hence an all-or-nothing gate). No code was copied.

## XCL

https://github.com/xcl-dev/xcl, GPL-2.0-only. Because that licence does not combine with ours, only facts from its
notes were used, never its code, structure or tables: that the newer D3D12.X list layout puts
Write32/64BitValueBottomOfPipeX at slots 71 and 72 and Wait32/64BitValueX at 73 and 74, that Wait64BitValueX's
argument after the value is flags and not a mask, the timestamp flags of the bottom-of-pipe writes, and that the
console GPU writes only to memory allocated as graphics memory (MEM_GRAPHICS, PAGE_GRAPHICS_*). Each was checked
against the game's executable (docs/d3d12x-shim.md); the implementation is our own.

## GTA6-Asset-Info

https://github.com/TrinitronClient/GTA6-Asset-Info, revision `0f649eba78cdd6e053d7aefd333fb6e3abe9739a`, GPL-3.0.
Its notes were leads, each checked on the dump before use; no text or code was copied.

- Textures (docs/textures.md): the resource-kind and array-size fields of the descriptor, the copies of a cube map
  and their padding, arrays stored level by level, no mip tail on a single-mip texture, the 3D brick order and the y
  bit order of the 1-byte 64 KB block. The stored mip count we use (mip field plus `d1` bits 0-2) is our own finding.
- Models (docs/models.md): the meshlet start wrap, the skin, tangent and extra texture-coordinate streams, the inverse
  bind matrices and the ped bone palette. Its order of the packed position low bytes was checked and not adopted.
- BCPack (docs/textures.md, `formats/06-bcpack.md`): the lane counts and symbol widths of codecs 3 to 7 and the lane
  meanings of the BC4, BC5 and BC7 block layers, checked by the convergence of every page, the anchor rule and the mip
  checks. None of its fitted tables were used.

## bcpack-rs

https://github.com/Ahmet-Dedeler/bcpack-rs, revision `0a887de1edc9c22a425605091f1a8365fdf74dbb`, licensed MIT OR
Apache-2.0 and used here under MIT.

- The BCPack BC1 decoder in `src/FiveStar.Formats/Ttpf/BcPack` is a C# implementation of its decoder (`src/rans.rs`,
  `src/bc1.rs`): the clocked lane model, the feeder and the BC1 block assembly.
- `src/FiveStar.Formats/Ttpf/BcPack/Tables/bc1_window.json` and `bc1_quad_table.json` are its
  `data/bc1_window.json` and `data/bc1_quad_table.json` with our corrections; each changed entry keeps the upstream
  value.
- `vendor/bcpack-rs` holds its synthetic test streams unchanged (`vendor/bcpack-rs/PROVENANCE.md`).

```
MIT License

Copyright (c) 2026 Ahmet Dedeler

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Microsoft silicon-rans

https://github.com/microsoft/silicon-rans, commit `2f6827d8a4e6a78029c372f20d8b61f7cd408fc5`.

- `src/FiveStar.Formats/Ttpf/BcPack/Tables/mdu_models.json` holds the rANS model tables (`base`, `freq`, `rf`, `rs`,
  `rn`) of its `mdu_minirans.v`, as tabulated by bcpack-rs.
- Each lane of the BC1 decoder follows that file's `decoder` module.

```
MIT License

Copyright (c) Microsoft Corporation.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE
```

## Concentus

https://github.com/lostromb/concentus, NuGet package `Concentus` 2.2.2, a C# port of the Opus reference library,
licensed under the BSD 3-Clause license below.

- The loader's `xapu.dll` queue (`src/FiveStar.Loader/Shims/Apu`) decodes the game's Opus packets with it; it is a
  package dependency, not copied code.

```
Copyright (c) by various holding parties, including (but not limited to):
Skype Limited, Xiph.Org Foundation, CSIRO, Microsoft Corporation,
Jean-Marc Valin, Gregory Maxwell, Mark Borgerding, Timothy B. Terriberry,
Logan Stromberg. All rights are reserved by their respective holders.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.

* Neither the name of Internet Society, IETF or IETF Trust, nor the
   names of specific contributors, may be used to endorse or promote
   products derived from this software without specific prior written
   permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.


This repository and its redistributable packages contain independently compiled
versions of the Opus C reference library, which is maintained by Xiph.org and the
Opus open-source contributors. The source code for these libraries is freely available
at https://gitlab.xiph.org/xiph/opus/-/tags/v1.5.2, and all binaries are being
redistributed to you under the same terms of the general Opus license dictated above.
```

## TraceEvent

https://github.com/microsoft/perfview, NuGet package `Microsoft.Diagnostics.Tracing.TraceEvent` 3.1.21 (with its
dependency `Microsoft.Diagnostics.NETCore.Client`), licensed under the MIT license below.

- `fivestar profile summary` (`src/FiveStar.Cli/Profile`) reads the loader's managed profiles with it, and the
  summary's test records one with the client; they are package dependencies, not copied code. The loader opens its
  own sessions through the documented diagnostics IPC protocol with code of its own.

```
Copyright (c) .NET Foundation and Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
## FFmpeg

https://github.com/FFmpeg/FFmpeg, file `libavcodec/wmaprodata.h` as of commit
`232e88928eee71afa55233d021eeda25eb3ed5b4` (its last change), licensed under the GNU Lesser General Public License,
version 2.1 or later. Section 3 of that license lets the tables be used under the GNU General Public License, which
this project does (`LICENSE`).

- `src/FiveStar.Formats/Audio/Xma/WmaProTables.cs` holds its WMA Pro tables unchanged as data: `critical_freq`,
  `scale_table`, `scale_rl_table`, `scale_rl_run`, `scale_rl_level`, `coef0_lens`, `coef0_syms`, `coef0_run`,
  `coef0_level`, `coef1_table`, `coef1_run`, `coef1_level`, `vec4_lens`, `vec4_syms`, `vec2_table` and `vec1_table`.
  The XMA2 decoder around them is our own code.

```
WMA 9/3/PRO compatible decoder
Copyright (c) 2007 Baptiste Coudurier, Benjamin Larsson, Ulion
Copyright (c) 2008 - 2009 Sascha Sommer

This file is part of FFmpeg.

FFmpeg is free software; you can redistribute it and/or
modify it under the terms of the GNU Lesser General Public
License as published by the Free Software Foundation; either
version 2.1 of the License, or (at your option) any later version.

FFmpeg is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
Lesser General Public License for more details.
```
