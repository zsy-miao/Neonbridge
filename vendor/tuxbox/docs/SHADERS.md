# RDNA2 shader translation

TuxBox translates the Xbox shader program a game supplies. Implementing an RDNA2 instruction or binding pattern helps every game that uses it. Compiled shaders are specific to their input bytes, stage and compiler configuration. On Linux the usual path is Xbox shader → HLSL → desktop DXIL → Vulkan SPIR-V. TuxBox and DXC produce the DXIL. Proton’s graphics backend handles the last step. [TuxBox shader worker](../project_tuxbox/runtime_shaders.py) · [vkd3d-proton shader cache](https://github.com/HansKristian-Work/vkd3d-proton#shader-cache)

## Measured on The Matrix Awakens

Frosty supplies a real collection of programs for testing the translator. Percentages here use the sampled archive entries as their denominator. A percentage of the RDNA2 instruction set would need a separate survey covering instructions, operand forms and execution modes. Frosty's reviewed archives contain 4,576 global entries and 38,831 material entries. Some refer to the same program. [Progress map](progress.svg) · [Shader inventory](../evidence/matrix-awakens/shader-inventory.json)

Frosty’s screen-probe irradiance shader (global 481) shares lighting coefficients across 64 lanes. TuxBox now waits for all producers before the next set of lanes reads those coefficients. On GeForce, the old shader produced invalid colours at 5,376 of 65,536 pixels with an 8 × 8 gather. The corrected shader produced none. The bright coloured patches disappeared in the matched on-foot street capture. [GPU and street measurements](../evidence/matrix-awakens/street-lighting-inputs.json) · [Before and after](../evidence/matrix-awakens/sandbox-lighting-comparison.webp)

**37,026 of 37,031 entries (99.99%) translate with ray tracing set aside.** All 32,122 material raster entries, all 734 material compute entries and all 192 mesh entries produce HLSL. All 2,412 global compute entries outside inline ray tracing also translate. The five remaining pixel entries handle hair-strand lighting (2409 and 2414) and distance-field object culling (726, 727 and 730). [Remaining entries](../evidence/matrix-awakens/nonray-shader-validation.json) · [Raster survey](../evidence/matrix-awakens/raster-archive-survey.json)

Across both packaged archives, **43,229 of 43,407 entries (99.59%) produced compiler output**. This includes 8 of 96 compute shaders that traverse ray-tracing geometry and 6,195 of 6,280 serialized ray-tracing state objects. Raster, mesh and compute entries count when TuxBox emits HLSL. Ray state objects count when DXC compiles their DXIL. In-game rendering and frame times are tracked separately. [Compute survey](../evidence/matrix-awakens/compute-archive-survey.json) · [Material compute survey](../evidence/matrix-awakens/material-compute-survey.json) · [Ray-state survey](../evidence/matrix-awakens/ray-state-census.json)

Since the last checkpoint, 123 global compute entries and the final material compute entry, 6308, passed DXIL validation. All 124 created pipelines using their packaged root signatures on Radeon and GeForce. GPU readbacks also checked the conservative depth buffer, particle keys, the nearest 16 and 32 hair samples, and reflections across wave boundaries. Game image quality still needs a retail run. [Pipeline results](../evidence/matrix-awakens/nonray-shader-validation.json) · [Depth](../evidence/matrix-awakens/conservative-depth-gather.json) · [Particle keys](../evidence/matrix-awakens/particle-sort-full-width-nvidia.json) · [Hair](../evidence/matrix-awakens/hair-visibility-linked-gpus.json) · [Reflections](../evidence/matrix-awakens/compute-wave64-portability.json)

On Radeon and GeForce, compute entries 681 and 1279 produced the expected probe tiles and offsets for six output planes. Six physics-field shaders also reproduced the expected debug glyphs, counters and clipping. Those checks used three inputs per shader. The field-volume image and line geometry still need an active rendering case. [Probe readbacks](../evidence/matrix-awakens/probe-tiles-prefix-gpus.json) · [Physics-field readbacks](../evidence/matrix-awakens/physics-fields-gpus.json)

| Column | What it measures |
| --- | --- |
| Packaged | Entries in the two shader archives shipped with this build. |
| HLSL / unique DXBC | Distinct non-ray programs for which the checkout compiler emitted HLSL. Desktop compilation, GPU results and visible rendering each need their own check. |
| TuxBox only | Distinct programs that the independent compiler emitted with legacy translation disabled. The survey compares its HLSL and profile with the regular build. |
| In profile | Distinct native programs listed in the launcher's menu and opening cinematic preparation profile. The linked vertex/geometry variants are counted separately. |
| In profile / requested | Profiled programs among the distinct hashes recorded by TuxBox during the sampled runs. Other scenes and graphics settings can request different shaders. |

The green code on the camera plane uses material shader map **136**. The verified main-menu trace includes its vertex shader at material entry 2 and pixel shader at entry 3530. The post-process filter uses map **66**, including pixel shader 1461. The inventory lists each map member, its archive index, stage and the runs that requested it. A request identifies a program used to create a pipeline. Checking a draw or a captured frame establishes whether that pipeline produced visible output. The packaged ray-tracing entries use Xbox serialized state objects and have their own translation path. [Rendered menu](../evidence/matrix-awakens/green-code-menu.webp) · [Cooked material references](../evidence/matrix-awakens/menu-materials.json) · [Ray-state samples](../evidence/matrix-awakens/retail-shaders.json)

The falling code renders green with packaged graphics settings. Frosty computes its bloom from a 2048×2048 texture. Some translated compute shaders read shared memory before other GPU lanes had finished writing it. This gave the tonemapper a zero scene multiplier and a blue-only bloom multiplier. After the synchronization fix, the tonemapper received all three channels. The new capture has 20,554 green-dominant background pixels and 1,303 blue, compared with 19 green and 48,261 blue before. [Corrected menu](../evidence/matrix-awakens/menu-green-code.jpg) · [Earlier blue menu](../evidence/matrix-awakens/menu-blue-code.jpg) · [Shader readbacks and measurements](../evidence/matrix-awakens/menu-materials.json)

### Inline Lumen ray tracing

Frosty uses global compute shader 219 to fill the two-bit shadow mask for each texel of a Lumen card. TuxBox translates the packaged program’s tile queue, ray origins, two distance intervals and shadow writes. The package declares the scene at `t5` as an acceleration structure, and its Xbox instructions read the nodes directly. At dispatch the provider resolves a native scene address from either a typed scene view or a raw buffer view, including the offset into the guest allocation. It supplies the native scene to a private root parameter and restores the game’s root bindings afterward. [Shader implementation](../tools/shader-compiler/src/compute_hlsl/card_direct_shadow.hlsl) · [Scene binding](../runtime/d3d12/src/device/blas/scene_view.rs)

On Radeon and GeForce, entry 219 reproduced the empty, hit and miss masks of the neighboring ray-generation shader 218 in the same test scene. The translation uses the host GPU’s triangle intersections. Driver precision, triangles at exactly zero distance and the Xbox traversal iteration limit can differ. If a volatile scene descriptor points to a different structure before submission, TuxBox rejects the recorded list. The survey has 88 untranslated compute programs that traverse Xbox BVH nodes. [Shadow-mask replay](../evidence/matrix-awakens/inline219-shadow-gpus.json) · [Endpoint controls](../evidence/matrix-awakens/direct-light-inline-boundaries.json)

The radiance-cache programs 318 and 306 use the scene at `t13`. Their closest-hit queries recover face normals from the cooked triangles or supported upload-backed vertex and index buffers. On Radeon and GeForce, reversing the winding of the same 18 hit triangles darkened the test card and suppressed its page feedback. GPU-only vertex data still lacks a source for these normals. [318 GPU readbacks](../evidence/matrix-awakens/inline318-radiance-gpus.json)

Entry 306 also writes a five-word record for a later lighting pass. TuxBox uses the packaged ray-generation shader’s no-bookmark representation. Entry 313 reads the queued ray and traces it again using instance mask `0x20`. On both GPUs the queued rays reproduced the reference hit distances and sky colors. Two reversed-winding records differed from the reference by one step in the packed distance. [306 queue results](../evidence/matrix-awakens/inline306-radiance-gpus.json) · [306 and 313 GPU replay](../evidence/matrix-awakens/inline313-radiance-gpus.json)

Screen-probe gather shaders 114 and 58 and reflection shaders 426 and 434 use native triangle queries. On Radeon and GeForce, the near- and far-field passes reproduced the packaged ray-generation shaders’ sky colors, hit distances and instance-mask selection in the tested scenes. The retail game recorded all four passes in Construct. [Screen gather](../evidence/matrix-awakens/inline114-screen-gpus.json) · [Far-field screen gather](../evidence/matrix-awakens/inline58-screen-gpus.json) · [Reflections](../evidence/matrix-awakens/inline426-reflection-gpus.json) · [Far-field reflections](../evidence/matrix-awakens/inline434-reflection-gpus.json)

TuxBox emits HLSL for all 5,102 MaterialCHS hit groups in Frosty’s material archive. The 2,551 opaque groups emit a closest-hit shader. The other 2,551 emit both closest-hit and any-hit shaders. All 628 newly translated pairs passed DXC and DXV. The progress map counts ray entries in the overall percentage at DXIL compilation. [Material hit-group survey](../evidence/matrix-awakens/material-hit-hlsl-survey.json)

Wake Up material 5384 computes opacity from the hit position and shader-record constants. Changing one constant from −0.67 to −0.66 changed both triangle faces from rejected to accepted. TuxBox and desktop DXR agreed on all 112 payload words on each GPU. [Material 5384 readbacks](../evidence/matrix-awakens/material5384-anyhit-gpus.json)

The material translator follows payload locations through branches and private stack saves. For transparent surfaces it handles the verified always-accept, always-ignore and payload-flag-dependent any-hit programs. A paired GPU replay checked these choices alongside each material’s closest-hit export. Separate tests exercised texture array layers, cube faces and explicit-gradient mip selection. [Hit and miss tests](../evidence/matrix-awakens/material-anyhit-gpus.json) · [Image instructions](../evidence/matrix-awakens/material-image-samples-gpus.json) · [Packed material arithmetic](../evidence/matrix-awakens/material-ray-arithmetic.json)

### Other measured shader paths

The ray translator now preserves the seven-bit field width in the scalar RDNA2 bit-field extract. A native Radeon sweep found 1,480 incorrect values in the previous translation across 4,096 offset and width pairs. After the fix, Radeon and GeForce matched every native result and condition bit. [Native instruction and desktop GPU comparison](../evidence/matrix-awakens/ray-scalar-bitfield.json)

DXC compiled all 5,975 material ray-tracing state objects and 220 of the 305 global ones. The 85 remaining entries are global shaders. Each packaged state contains three traversal programs and a fourth program with the shader exports. TuxBox translates the exports and resource bindings for desktop DXR and uses the host GPU for traversal. [Archive census](../evidence/matrix-awakens/ray-state-census.json)

On GeForce and Radeon, TuxBox accepted four raw retail states through the Xbox device, compiled them on demand and linked them into one pipeline. The Xbox command list ran the occlusion and intersection ray generators against two instances. Each processed six rays and matched a desktop D3D12 control. Occlusion returned four hits and two misses. Intersection returned the expected distances, primitive and instance indices, and barycentric coordinates. One miss passed beside the scene and one stopped short of the triangle. [GeForce readback](../evidence/matrix-awakens/ray-library-dispatch.json) · [Radeon readback](../evidence/matrix-awakens/ray-library-dispatch-amd.json)

Acceleration-structure builds and copies now reject an address when multiple registered buffers cover it. A Radeon and GeForce control creates overlapping placed buffers at the same GPU address, verifies the rejection, and confirms a build using the remaining buffer can be recorded after the other is released. Ordinary ray queries and late BLAS relocation still pass. [Address ownership checks](../evidence/matrix-awakens/ambiguous-as-addresses.json)

Frosty requests screen-space shadow shaders at global entries 3801 and 3824. Both read six G-buffer textures and resume shading after a ray hits or misses. In a 4 × 2 dispatch, GeForce and Radeon returned the expected colors for an occluded pixel, a visible pixel and a two-sided material that traced back through a surface. The zero-depth and out-of-bounds pixels issued no rays. Entry 3824 also wrote a hit distance that matched the GPU’s triangle intersection on each adapter. [Shadow textures and callback counts](../evidence/matrix-awakens/occlusion-ray-dispatch.json)

Entry 3810 also traces hair shadows through a virtual voxel volume. On Radeon, a ray that missed every triangle stayed lit with a missing page or a page with zero density. Giving the voxel a density of 1,024 shadowed the same pixel. The two-sided material then fired its second ray. The test also enabled a hair surface at one pixel. When its lighting-channel mask matched the light, the shader traced one additional ray and wrote white to its separate hair-lighting texture. A disjoint mask left that texture black. [Voxel shadow readback](../evidence/matrix-awakens/occlusion-ray-dispatch.json)

TuxBox also translates both Lumen translucency volume shaders at global entries 619 and 620. On Radeon, each traced 32 rays into a sky cube with six different colors. The returned light matched the expected face for every ray. Depth testing excluded 32 other cells, and a dispatch coordinate outside the volume left its output untouched. [Volume lighting and hit distances](../evidence/matrix-awakens/lumen-translucency-volume.json)

Radeon ran the five Lumen reflection shaders used by Frosty’s first two ray pipelines. Two jobs in reverse pixel order returned distinct sky colors and a signed distance of −17. A second test placed a triangle 0.175 units along one ray. Shader 456 wrote the measured distance to its reflection pixel and trace record while the other ray returned the sky. The unused pixel and queue record kept their original values. [Reflection pixels, trace records and callback counts](../evidence/matrix-awakens/lumen-reflections.json)

GeForce and Radeon ran the retail lighting miss shader at global entry 2752. Adjacent rays used different shader records to select an unlit or colored directional light. Across 24 rays, the output preserved the incoming radiance in darkness and added the expected RGB contribution on a front-facing white surface. The back-facing surface stayed dark. Two more rays exercised the hidden material at entry 2670. Its any-hit shader ignored the triangle, and its closest-hit shader preserved the payload when the ray forced the triangle opaque. [Two-GPU lighting and hidden material readback](../evidence/matrix-awakens/lighting-and-hidden-rays.json)

The radiance-cache shader at global entry 323 traces an 8 × 8 tile and packs RGB and distance into two words per ray. GeForce and Radeon matched all six sky colors and the miss distance on the empty scene rays. Moving the probe beside a triangle gave 18 surface hits. Each packed distance matched the GPU intersection rounded to half precision. An empty work queue left every output untouched. Radeon also ran the opening scene’s record-writing and far-field variants at entries 299 and 311. Each reproduced all 18 intersections. Shader 264 traced two rays per cell and shader 266 packed all 64 retrace records with no traversal. [Radiance-cache tile readbacks](../evidence/matrix-awakens/lumen-radiance-cache.json)

All 64 packaged Lumen screen-probe gather ray-generation variants compile and validate through DXIL and Vulkan SPIR-V. Frosty requests entries 64 and 71 in its second scene pipeline. On GeForce and Radeon, both left the output untouched for an empty queue and hit a triangle at 0.49 units when a probe pointed toward it. Facing empty space, entry 64 issued one miss and entry 71 issued two, then wrote the expected 20-byte retrace record. [Screen-probe dispatch results](../evidence/matrix-awakens/screen-probe-gather.json)

The virtual-shadow cache shader at entry 1228 reuses eligible pages from a previous frame and updates their age. A GeForce and Radeon control checked four combinations of cached data, page invalidation and missing mappings. Each dispatch matched all 88,436 words of the CPU reference, including five statistics counters and both layers of the page tables. [Cached shadow page results](../evidence/matrix-awakens/cached-shadow-pages.json)

The opening pipeline also uses 12 closest-hit material shaders. On Radeon, TuxBox ran all 12 from their packaged state objects and matched the desktop control on every word returned by 48 triangle hits and 12 misses. Reversing the ray changed the surface-facing flag. Two materials returned the exact RGB values supplied through the shader record. [Material payloads and shader inputs](../evidence/matrix-awakens/material-hit-dispatch.json)

Frosty’s second ray pipeline uses a smaller Lumen material shader at global entry 1238. It returns the hit distance, geometry data, a packed surface normal and two instance identifiers. On Radeon and GeForce, TuxBox matched the desktop results for four triangle hits and the paired miss shader at entry 1237. A scaled, sheared instance produced the expected change in its surface normal. [Lumen geometry and miss readbacks](../evidence/matrix-awakens/lumen-material-rays.json)

Frosty compiles many shadow shaders when it first builds its ray-tracing pipeline. At global entry 3770, TuxBox now merges branches with a single entry and shares one traversal call across eight shader continuations. On an RTX 4070 Ti SUPER, cold compilation of that collection fell from 38.67 to 7.26 seconds. Peak process memory fell from 2.81 GB to 0.86 GB. [Compiler timings and GPU controls](../evidence/matrix-awakens/shadow-ray-compilation.json)

To repeat the archive count or GPU check with a mounted copy of Frosty, use the following commands.

```sh
python tools/check_frosty_shaders.py "$MOUNT" --compile-ray-census --output build/ray-census.json
python tools/check_frosty_blas.py --ray-archive "$MOUNT" --lighting-hits --output build/lighting-rays.json
python tools/check_frosty_blas.py --ray-archive "$MOUNT" \
    --provider build/d3d12-smoke/d3d12_xs.dll --voxel-occlusion --lumen-volume --lumen-reflections --lumen-material --material-hits --radiance-cache --output build/ray-dispatch.json
```

The menu pixel shader uses 28 Xbox instructions to share values across groups of four pixels. Each instruction reads from one fixed lane. TuxBox now emits one broadcast per instruction. On the RTX 4070 Ti SUPER, the captured menu draw took a median of 0.941 ms, down from 1.022 ms, with identical pixels across the entire HDR target. [GPU timings and shader identities](../evidence/matrix-awakens/pixel-quad-broadcast.json)

Pixel shaders now check the Xbox execution mask once for consecutive register writes that use only arithmetic. Stock Proton translated Frosty’s material shader 18160 to SPIR-V in 336 ms on the test PC, down from 10.94 seconds. Two other chase materials took 158 ms and 48 ms, down from 2.15 seconds and 1.18 seconds. On the RTX 4070 Ti SUPER, nine test draws with those shaders retained the same coverage. The largest difference in any color channel was 0.00000012. In the captured green-code draw, five of 3,686,400 pixels changed by one step in the half-precision target. [Shader identities, CPU timings and GPU readbacks](../evidence/matrix-awakens/pixel-exec-groups.json)

One recorded archive survey translated all 19,634 distinct vertex shaders to HLSL. GPU readbacks check the cubemap shadow shader’s vertex outputs and the compute instructions used by 94 newly translated programs. Across 30 traces of startup, the menu, Nanite, the chase and city free roam, all 2,550 observed programs passed standalone DXIL validation. Normal gameplay preparation covers every shader observed in the full chase into the city. [Cubemap shadow readback](../evidence/matrix-awakens/cubemap-shadow-vertex.json) · [Clip-distance outputs](../evidence/matrix-awakens/clip-distance-vertex.json) · [Compute instruction checks](../evidence/matrix-awakens/compute-instructions.json)

Four street materials in Frosty’s city read packed page-table entries from the same `R32_UINT` texture. TuxBox now preserves those integer loads when the Xbox shader range-checks a value from the first active lane and extracts a bit field. This changed 147 packaged pixel shaders, all of which passed DXIL validation. In a replay of an asphalt draw, the black pavement patch appeared with both texture declarations. [Street texture and draw measurements](../evidence/matrix-awakens/street-page-table.json)

Global shader 475 converts screen probes to irradiance using wave64 reductions. TuxBox maps its 8×8 workgroup to 64 logical lanes on the host. Full-shader readbacks on Radeon and GeForce matched a Radeon native-wave64 translation for probe widths one through five and eight when the sampled depth texture was constant. With a gradient point-sampled texture at width three, a direct capture showed the GPUs selecting adjacent texels at a coordinate boundary. [Probe irradiance GPU results](../evidence/matrix-awakens/probe-irradiance-wave64.json) A second 8×8 variant writes two textures; both match the native-wave64 version on Radeon and the portable version on Radeon and GeForce. The reference shares the remaining shader translation. [Two-output GPU readbacks](../evidence/matrix-awakens/probe-irradiance-485.json)

## ProjectScarlett upscaler

TuxBox compiles 28 of the 29 compute shaders in the supplied FidelityFX upscaler to validated DXIL. Nine newly supported tensor passes ran on Radeon and GeForce using the package’s root signatures. For each pass, all 32 MiB of output matched the original RDNA2 instructions running on Radeon. Two large passes compiled from HLSL to DXIL in 0.93 s and 2.17 s using packed signed-byte dot products. [Native and translated tensor results](../evidence/providers/fidelityfx-tensors.json) · [Repeat the check](../tools/check_fidelityfx_tensors.py)

With an empty shader cache, the upscaler created 26 pipelines on Radeon before shader `0x9fe90` read lane 31 of register `v1`, a case the translator cannot yet resolve. On GeForce, the driver rejected an unordered-access texture with format `R9G9B9E5_SHAREDEXP` during resource creation. The standalone checks and runtime results are separate from Frosty’s archive counts. An upscaled frame still needs verification. [Packaged upscaler run](../evidence/providers/scarlett-fidelityfx-runtime.json)

## Independent translation

An earlier survey of TuxBox’s independent Rust compiler translated 99 of Frosty’s 12,271 distinct pixel shaders and seven of its 3,233 compute shaders. These are results for that compiler snapshot, not a current coverage count. The pixel shaders write constant colors to one or more targets, preserve rasterized depth, or discard every sample. Three compute programs initialize specific words in structured buffers. The other four perform empty dispatches. All 106 passed readback checks on GeForce and Radeon, including untouched pixels and buffer words. The gameplay compiler still uses TomBox-derived HLSL generation for many other supported programs. [GPU results](../evidence/matrix-awakens/independent-shader-gpu.json) · [Compute instructions and strides](../evidence/matrix-awakens/independent-compute.json) · [Source attribution](REFERENCES.md#shader-compiler-attribution)

A cold rebuild validated all 223 prepared entries and linked variants. Four pixel programs and two linked vertex programs changed. The two updated pairs created depth-only pipelines with their original root signatures on both GPUs. [Prepared shader comparison](../evidence/matrix-awakens/independent-shader-profile.json)

Build the independent compiler into a separate directory to test a raw Xbox shader. It reports an error when a program needs an unsupported operation. This command compiles the supplied bytes directly and does not read the game’s shader cache.

```sh
cargo build --locked --release --no-default-features \
    --manifest-path tools/shader-compiler/Cargo.toml --target x86_64-unknown-linux-gnu \
    --target-dir build/independent-compiler
build/independent-compiler/x86_64-unknown-linux-gnu/release/xshader \
    shader.dxbc --hlsl-out shader.hlsl --profile-out shader.profile
```

## Vertex fetch

Three of Frosty’s vertex shaders give us expected positions and texture coordinates directly from their Xbox instructions. Global entry 1007 copies the input position and sets W to one. Entry 2450 passes through the full position and a pair of texture coordinates. Entry 2751 scales and flips coordinates into clip space. On stock Proton, the current gameplay compiler produced the expected bytes for all three with separate buffers and with interleaved attributes, on GeForce and Radeon. Deliberately swapping the two texture coordinates failed both layouts for entry 2450. This is a baseline for replacing the legacy vertex emitter. [Readbacks and reproduction](../evidence/matrix-awakens/vertex-fetch.json)

Frosty’s menu material at entry 2 illustrates the overlap with AnyPS5’s instruction profile. LLVM 23.1.5 decodes the shader’s call at byte offset `0x18` as `s_swappc_b64 s[4:5], s[4:5]` on both `gfx1013` and `gfx1030`. At `0x1cc`, the former recognizes `v_mad_f32` and the latter rejects the instruction encoding. TuxBox uses AMD’s GFX10.1 and GFX10.3 tables together. The Xbox shader signature and input layout supply the evidence for how a game binds its vertex data. [Menu material inventory](../evidence/matrix-awakens/menu-materials.json) · [Opcode table](../runtime/dxbc/src/rdna2/opcodes.rs) · [AnyPS5 research notes](REFERENCES.md)

## ProjectScarlett refusals

On November 25, 2026, preparation compiled 20,583 of 25,079 recorded requests and refused 4,496. None failed in DXC or the validator. [Preparation record](../evidence/projectscarlett-shader-preparation.json)

| Refusal reason | Requests |
| --- | --- |
| Mesh request recorded without a root signature | 1,551 |
| Wave reduction that may execute in divergent control flow | 1,502 |
| Image store to a table that isn't an unordered-access table | 432 |
| Buffer store to a table that isn't an unordered-access table | 145 |
| Raw buffer `IDXEN` access that needs the console descriptor stride | 129 |
| Lane operation with a non-constant lane | 115 |

The counts are per recorded request, so they overstate what a live run loses: a request recorded without its root signature can't be translated offline. Rank failures by the pipelines a run lost with the [pipeline failure record](PROJECTSCARLETT.md#finding-why-pipelines-fail).

### Whole-wave branches

The console branches once per wave, but the translator normally branches once per thread. Where its targeted checks can't show that this is safe around a wave operation, it now branches with the whole wave: branches on EXEC and VCC vote across the wave, as do the condition codes set by mask operations ([analysis](../tools/shader-compiler/src/hlsl/lockstep.rs)). It refuses programs that use a lane mask as a number, or a number as a lane mask. Ray tracing resolve programs, which already keep whole masks, also translate now when their loops go through the block dispatcher.

A [GPU check](../tools/check_lockstep_gpu.py) compares both paths with a model of the console on an RTX 4070 Ti SUPER: 245 cases match. Programs that translate through the targeted checks still branch once per thread. With the final branch that routes them to whole-wave translation removed, 160 of 210 cases of the same programs give wrong results, so some shaders that translate today are likely to compute wrong values. This hasn't been traced to anything visible in a game.

Most wave reduction refusals come from programs over 6,000 instructions, where the translator doesn't try its one-lane-per-wave form. In a trial on an earlier set of 1,442 such compute shaders, removing that limit translated 1,211. Six sampled outputs compiled in 2 to 12 seconds with DXC optimization off, and mostly produced 1 to 2 MB of DXIL. Large compiles in a recorded run took the driver 6 minutes or more, so the limit stays until the generated code compiles faster. The outputs weren't tested in a game.

### Earlier survey

Shader preparation reports only how many recorded requests xshader refused. `tools/triage_scarlett_shaders.py` runs the HLSL emitter on each recorded request in the prefix's `projectscarlett/<exe sha256>/requests` directory. It groups the refusal messages with program counters and register numbers masked. It does not run DXC and does not write to the cache. On 9,102 requests recorded through player selection, xshader translated 4,383 and refused 4,719. The largest groups were `s_load` with a dynamic SOFFSET (29%), sampler descriptors resolved from an inline word (19%), and wave reductions in divergent control flow (13%).

## Reusing compiled shaders during play

The graphics provider can read a cached translation directly after the host identifies the compiler for the session. It checks the shader stage and the SHA-256 of the bytes it passes to the pipeline. On a warm cache, one of Frosty’s clear pipelines took 0.82 ms on GeForce and 1.62 ms on Radeon. The previous polling worker took 52.13 ms and 26.14 ms respectively. Both GPUs returned all 1,400 expected buffer words. For a new translation, Linux now wakes the worker when the guest publishes its request. In 20 cold trials with the same clear shader, the median time from request to validated DXIL fell from 46.94 ms to 18.16 ms. [GPU readbacks and worker timings](../evidence/providers/shader-cache-readback.json)

## Street materials and scene instances

Earlier reduced-effects runs using stock Proton and the Proton fork showed missing character surfaces in the rooftop scene. The standing character’s head and torso are absent in both captures. [Earlier rooftop frame](../evidence/matrix-awakens/rooftop-before.jpg) · [Later rooftop frame](../evidence/matrix-awakens/rooftop-independent.jpg)

Frosty's pixel shaders read triangle facing and fragment position from registers selected by the Xbox shader. TuxBox now supplies the front-face flag and packs the enabled position components in hardware order. On Radeon and NVIDIA, the same two-sided material produced the expected light front and dark back. A second material produced the expected depth values at both test distances. All 667 affected pixel programs requested during the recorded chase and city run passed DXIL validation. [Pixel shader GPU readbacks](../evidence/matrix-awakens/pixel-raster-inputs.json)

Material and compute shaders also need the byte distance between records in a structured buffer. TuxBox reads that distance from the Xbox binding metadata where available. In a GPU test, Frosty's scene-update shader previously wrote 64 of 128 primitive IDs and overwrote 64 adjacent IDs. It now writes all 128 requested IDs and preserves all 128 adjacent IDs. [Scene-update GPU readback](../evidence/matrix-awakens/scene-primitive-ids.json)

## Mesh shaders

All 192 mesh programs in Frosty’s global archive are permutations of `HWRasterizeMS`. TuxBox translates their Xbox instructions and output declarations to Shader Model 6.5. The original workgroup has 256 logical lanes. Each desktop workgroup uses 128 threads and processes the extra vertices in a second pass. TuxBox also reconstructs the packed triangle indices, per-primitive attributes and culling decisions. [Mesh translator](../tools/shader-compiler/src/hlsl/mesh.rs) · [DirectX mesh shader specification](https://microsoft.github.io/DirectX-Specs/d3d/MeshShader.html)

Both the RTX 4070 Ti SUPER and the Radeon integrated GPU created pipelines for all 192 validated programs. Separate paired draws exercise rasterization with original pixel shaders, including a triangle that references vertices 192–194. The graphics tests also cover direct and indirect Xbox dispatch and a portable amplification shader. Frosty’s archive contains no amplification programs. [NVIDIA results](../evidence/matrix-awakens/nanite-mesh-nvidia.json) · [Radeon results](../evidence/matrix-awakens/nanite-mesh-amd.json) · [Command-list checks](../evidence/matrix-awakens/mesh-command-list-nvidia.json)

To repeat the retail shader tests on a mesh-capable adapter, use its PCI vendor and device IDs.

```sh
python build.py d3d12
python tools/check_frosty_nanite_mesh.py /path/to/Mount --adapter 10de:2705 \
    --all-mesh --output build/nanite-mesh.json
```

## Virtual shadow maps

The packaged `CompactFeedbackHashTableCS` shader compacts nonzero feedback into a bounded output list. Radeon and GeForce matched a CPU model across six dispatches, including lanes on both sides of the wave64 boundary, a partial input, exhausted capacity and invalid record indices. [GPU readbacks](../evidence/matrix-awakens/compact-feedback.json)

The packaged `CullToVoxelClipmapCS` shader builds a list of objects intersecting a voxel clipmap. Radeon and GeForce matched a CPU model for intersecting, distant and undersized objects across sparse, full and partial waves. [GPU readbacks](../evidence/matrix-awakens/voxel-clipmap-cull.json)

Lumen’s visibility-shading indirect setup also translates and passes provider-level checks on both GPUs. The control checks the dispatch dimensions and buffer guards for five allocator values; it does not validate the complete lighting pass. [GPU readbacks and title view-format evidence](../evidence/matrix-awakens/voxel-indirect-setup-gpus.json)

The Lumen update-grid tile shader also translates independently. Provider controls on Radeon and NVIDIA check the tile list, counter and 3D mask with finite bounds; the full lighting pass remains unverified. [Radeon](../evidence/matrix-awakens/lumen-update-tiles-amd.json) · [NVIDIA](../evidence/matrix-awakens/lumen-update-tiles-nvidia.json)

Frosty uses compute shaders to allocate physical shadow pages, choose pages for initialization and merging, and compact the views that need rendering. GeForce and Radeon ran ten packaged shaders and matched every one of the 137,694 output words checked on each GPU. The cases include an exhausted free-page pool, requests in both halves of a 64-lane wave, and selected mip levels from multiple views. Raising the depth texture from zero to one culled all six shadow instances that previously reached the work queue. [Shadow page and view readbacks](../evidence/matrix-awakens/virtual-shadow-pages.json)

The first-boot hair cluster-culling shader also passes five CPU-modeled readbacks on Radeon and GeForce through TuxBox’s D3D12 provider. The controls cover visible and hidden clusters, LOD and frustum selection. [GPU readbacks](../evidence/matrix-awakens/hair-cluster-provider-gpus.json)

A second hair cluster-culling shader now passes six provider-level readbacks on both GPUs, including its additional per-cluster LOD metadata. [GPU readbacks](../evidence/matrix-awakens/hair-cluster-503-gpus.json)

Two hair cluster prefix-sum shaders also match CPU-modeled results on Radeon and GeForce through the provider, including empty, partial and multi-block inputs and output guard words. These checks cover the prefix stage only. [GPU readbacks](../evidence/matrix-awakens/hair-prefix-provider-gpus.json)

The hair-mesh deformation shader passes CPU-modeled checks on Radeon and GeForce for multiple control-point counts, partial output and two groups. This verifies one stage; hair rendering still needs end-to-end validation. [GPU readbacks](../evidence/matrix-awakens/hair-mesh-deformation.json)

Ten reflection trace compaction variants translate their wave32 reductions and prefix scans. Real texture inputs and CPU-modeled outputs pass on Radeon and GeForce for empty, full, mixed and wave-boundary cases across groups of 64 to 1,024 threads. [GPU readbacks](../evidence/matrix-awakens/reflection-compact-wave-sizes.json)

The visualization tile shader compacts viewport-intersecting 8×8 groups. GPU controls cover viewport edges, fractional offsets and floating-point conversion limits. [GPU readbacks](../evidence/matrix-awakens/visualize-create-tiles.json) · [AMD RDNA 2 instruction reference](https://docs.amd.com/api/khub/documents/Et~wpu9g~Ffl7d9q0QZ~Og/content)

Three visualization ray-compaction variants preserve matching data and ray records while selecting different flags. GPU controls cover empty, full, mixed and two-group dispatches. [GPU readbacks](../evidence/matrix-awakens/visualize-compact-rays.json)

Four hardware ray-compaction variants select five-word ray records, including variants with different flag masks. Tests on Radeon and GeForce check sparse and full waves, per-lane limits and two groups. [GPU readbacks](../evidence/matrix-awakens/hwrt-compact-rays.json)

The mesh distance-field offset shader builds object ranges and indirect dispatch arguments. Radeon and GeForce controls cover wave boundaries, multiple groups and integer wrapping. [GPU readbacks](../evidence/matrix-awakens/mesh-sdf-offsets.json)

The distance-field streaming request shader compacts changed nonzero mip requests into a bounded queue. Radeon and GeForce match CPU controls for full, sparse, partial and capacity-limited groups. [GPU readbacks](../evidence/matrix-awakens/distance-field-requests.json)

## Nanite instance culling

Frosty calls `InstanceCull` to decide which scene instances enter Nanite’s work queue. Its 64-lane Xbox workgroup counts visible instances, reserves space in the queue, and writes each instance ID in lane order. TuxBox also handles the path that writes small projected instances directly into the visibility texture. Frosty’s first-boot profile includes the variant requested by the title, global entry 3444. [Retail comparison and list of variants](../evidence/matrix-awakens/instance-cull-retail.json)

Another packaged instance-culling variant uses a returning atomic depth maximum to select a visibility pixel. Radeon and GeForce controls check individual lanes on both sides of the wave boundary, simultaneous equal-depth candidates, and a protected pixel. Equal-depth candidates can finish in either order. [First pass](../evidence/matrix-awakens/instance-cull-returning-atomic.json) · [Post pass](../evidence/matrix-awakens/instance-cull-post-returning-atomic.json) · [Statistics variants](../evidence/matrix-awakens/instance-cull-statistics-returning-atomic.json) · [More variants](../evidence/matrix-awakens/instance-cull-atomic-additional.json) · [Variants without depth pyramid](../evidence/matrix-awakens/instance-cull-atomic-no-hzb.json)

Twelve of the 76 packaged `InstanceCull` variants pass GPU readback on both NVIDIA and Radeon. Each GPU ran 149 dispatches and matched 89,838 buffer words and 12,069 visibility texels. The checks cover ordered per-view queues, capacity limits, impostor depth and occlusion against a four-level depth pyramid. Paired controls confirm that selecting mip 1 changes which instances survive. Two second-pass variants recheck previously hidden instances against the current depth and append the survivors to the three-word node queue. [NVIDIA results](../evidence/matrix-awakens/instance-cull-nvidia.json) · [Radeon results](../evidence/matrix-awakens/instance-cull-amd.json)

```sh
python build.py d3d12
python tools/check_frosty_instance_cull.py /path/to/Mount --adapter 10de:2705 \
    --output build/instance-cull.json
```

## Nanite cluster culling

After instance culling, `PersistentClusterCull` walks the scene hierarchy, requests missing mesh data and builds the lists of visible clusters for hardware and software rasterization. TuxBox keeps the Xbox workgroup’s 64 lanes together during queue updates, including on GPUs that execute 32 lanes at a time. The translator handles the lane shuffles, shared memory and atomic updates used by these kernels. [AMD RDNA 2 instruction reference](https://docs.amd.com/api/khub/documents/Et~wpu9g~Ffl7d9q0QZ~Og/content)

Five of the 32 packaged variants pass DXIL validation and GPU readback on both Radeon and NVIDIA. Each GPU ran 86 dispatches and matched all 45,062 buffer words. The checks cover nested hierarchy traversal, streaming requests, camera selection and draw counts for both rasterizers. Entry 1827 uses the previous frame’s instance transforms and depth pyramid to defer hidden geometry. Its controls check which clusters survive at two mip levels, the records written across a 64-cluster batch boundary and the child masks saved for the next pass. [Retail run and family census](../evidence/matrix-awakens/persistent-cluster-retail.json) · [Radeon results](../evidence/matrix-awakens/persistent-cull-amd.json) · [NVIDIA results](../evidence/matrix-awakens/persistent-cull-nvidia.json)

```sh
python tools/check_frosty_persistent_cull.py /path/to/Mount --adapter 10de:2705 \
    --output build/persistent-cull.json
```

## Nanite micropolygon rasterization

All 120 packaged `HWRasterizePS` variants now translate and pass DXIL validation. Three variants were dispatched on GeForce and Radeon, including the clipped variant requested by Frosty. Each matched all 247 texels in the test, including competing depth values and untouched pixels outside the clip rectangle. [Shader census and GPU results](../evidence/matrix-awakens/hwraster-visibility.json)

`MicropolyRasterize` draws small triangles from Nanite’s visible-cluster list. The variant Frosty requests, global entry 3246, stores depth and triangle ID together in a 64-bit visibility value. When triangles overlap, one atomic maximum selects the winning pair. TuxBox preserves the Xbox shader’s choice of scanline loop for each 64-lane wave. [Shader Model 6.6 atomic operations](https://microsoft.github.io/DirectX-Specs/d3d/HLSL_SM_6_6_Int64_and_Float_Atomics.html)

All 96 packaged variants passed DXIL validation and created an Xbox compute pipeline on Radeon and NVIDIA. Entry 3246 also matched all 9,269 texels in 13 GPU tests per adapter. These cover narrow and wide triangles, interpolated depth, overlap, and pixels outside the viewport. The retail Radeon run passed this pipeline and reached graphics command-list slot 96. [Retail traces](../evidence/matrix-awakens/micropoly-retail.json) · [Radeon results](../evidence/matrix-awakens/micropoly-amd.json) · [NVIDIA results](../evidence/matrix-awakens/micropoly-nvidia.json)

```sh
python tools/check_frosty_micropoly.py /path/to/Mount --adapter 10de:2705 \
    --all-pipelines --output build/micropoly.json
```

## Subsurface setup

The depth-only subsurface indirect setup shader now passes provider-level checks on Radeon and GeForce. The controls compare depth and material outputs, indirect counters, list capacity and guard values. The existing color-output variant also passes its Radeon regression check. These checks cover setup rather than complete skin rendering. [GPU readbacks](../evidence/matrix-awakens/subsurface-847-provider-gpus.json)

Two additional setup variants classify tiles into separate indirect lists. Both pass provider-level checks on Radeon and GeForce for each list and for limited list capacity. [GPU readbacks](../evidence/matrix-awakens/subsurface-classified-gpus.json)

## Hair strand visibility

Frosty ships 28 variants of the shader that gathers visible hair samples into per-pixel lists (global entries 977–1004). All 28 now translate. The 12 linked-list variants keep up to 8, 16 or 32 samples per pixel. Six also write velocity. Readbacks on Radeon and GeForce checked depth order, overflow, both velocity components and untouched pixels. [Linked-list GPU results](../evidence/matrix-awakens/hair-visibility-linked-gpus.json)

The 12 multisample variants read strand IDs and depths at 2×, 4× or 8× MSAA. All five writable outputs matched the CPU reference on both GPUs, including samples that merged or failed the depth test. Four other variants use single-sample inputs with direct or indirect dispatch. [Multisample GPU results](../evidence/matrix-awakens/hair-visibility-msaa-gpus.json) · [Single-sample results](../evidence/matrix-awakens/hair-visibility-single-gpus.json) · [Indirect tile results](../evidence/matrix-awakens/hair-visibility-indirect-gpus.json)

The separate hair raster pass translates all 24 packaged compute shaders. Its two compaction shaders passed readback on both GPUs. The voxel page allocator (752) also passed on both GPUs for sparse clusters, overlapping pages, a full output buffer and an empty dispatch. [Hair raster results](../evidence/matrix-awakens/hair-visibility-raster.json) · [Raster compaction](../evidence/matrix-awakens/hair-visibility-compaction-gpus.json) · [Voxel page allocation](../evidence/matrix-awakens/hair-voxel-page-allocation.json)

Visible character hair still needs an in-game check. The recorded reduced-settings chase explicitly disabled strands. Frosty requested hair-card deformation (2676) in 14 of the 30 recorded traces. None requested entries 977–1004. Shader 2676 passes DXIL validation. Verifying its packed inputs needs the live buffer view formats at `t1`, `t2` and `t12` during a dispatch with active vertices. The packaged-settings run completed Wake Up and activated the first chase street corner. Visible strands still need checking in a character scene. The two remaining strand-lighting pixel shaders (2409 and 2414) were also absent from the 30 traces. They sum sample counts across 64 lanes, reserve space once per wave, then write each node’s starting offset into its 48-byte record. [Chase settings](../evidence/matrix-awakens/chase-gameplay.json) · [Shader requests](../evidence/matrix-awakens/shader-inventory.json) · [Packaged-settings run](../evidence/matrix-awakens/packaged-settings-room.json) · [Pixel wave results](../evidence/matrix-awakens/pixel-wave-hosts.json)

## Nanite material classification

Frosty’s `ClassifyMaterials` shader reads the triangles visible in each 64 × 64 tile and records which materials to draw there. Tests of the packaged shader at global entry 2725 checked its draw arguments and material tile masks on both GPUs. The eight cases cover empty tiles, mixed materials, streamed cluster pages and tiles offset from the origin. [NVIDIA readbacks](../evidence/matrix-awakens/classify-materials-nvidia.json) · [Radeon readbacks](../evidence/matrix-awakens/classify-materials-amd.json)

## Pixel wave elections

The five remaining global raster programs contain pixel-shader EXEC elections: three mesh distance-field object-culling programs and two hair environment-lighting programs with additional lane permutations. [Election survey](../evidence/matrix-awakens/pixel-wave-elections.json) · [Survey tool](../tools/survey_pixel_elections.py)

These programs remain untranslated. Desktop pixel shaders cannot request a fixed 64-lane wave, and replacing a wave reservation with individual atomics can select a different subset when a list overflows. Helper invocations and downstream list ordering also need verification before enabling a translation.

A synthetic pixel draw used 32-lane waves on the tested NVIDIA GPU and 64-lane waves on Radeon. Both exposed helper-lane neighbors. The control checks host allocation and guards; it does not establish the Xbox shader's selected subset. [Host GPU results](../evidence/matrix-awakens/pixel-wave-hosts.json)

## Pixel shader stencil export

Frosty’s global pixel shaders 1748 and 1763 read visibility data and write depth and a stencil byte for each surviving pixel. Radeon supports this output directly. On the tested RTX, TuxBox routes the stencil byte to an extra integer render target and copies it back to the depth/stencil texture. This path handles a single-sample texture with full-byte replacement on a passing fragment. In two draws per shader, all 128 depth and stencil pairs matched on each GPU, including pixels discarded by the shader and pixels rejected by the depth test. [NVIDIA readbacks](../evidence/matrix-awakens/stencil-export-nvidia.json) · [Radeon readbacks](../evidence/matrix-awakens/stencil-export-amd.json)

```sh
python tools/check_frosty_stencil.py /path/to/Mount --adapter 10de:2705 \
    --output build/stencil-export.json
```

## Temporal super-resolution

Frosty requested global compute entry 2653 when it loaded the city sandbox. The packaged shader map identifies it as `FTSRFilterFrequenciesCS`, a temporal super-resolution (TSR) filter. Its 256 threads use a value shared within each Xbox wave to select the image coordinates to process. The default first-boot profile now includes this permutation. [First-boot shader list](../tools/reference/frosty-menu-shaders.json)

The packaged shader wrote two color images and a filter weight on both NVIDIA and Radeon. Five GPU tests covered black, uniform and patterned images, swapped color inputs, and a clipped output rectangle. Each GPU returned the same bytes for all 15 output images. The tests check constant colors, local color bounds and untouched pixels outside the rectangle. [NVIDIA readbacks](../evidence/matrix-awakens/tsr-filter-nvidia.json) · [Radeon readbacks](../evidence/matrix-awakens/tsr-filter-amd.json)

The next flyover pass, `FTSRUpdateHistoryCS` at global entry 3385, combines current-frame color and translucency and can generate a lower-resolution mip. TuxBox translates its packed halfword arithmetic and the shared-memory exchanges across its 32-thread group. The default launch now prepares this shader too. On both GPUs the original Xbox dispatch produced identical images across five cases, including a half-transparent overlay and a patterned input. The tests compare the mip against a CPU 2 × 2 average and check the pixels outside the written region. [NVIDIA history tests](../evidence/matrix-awakens/tsr-history-nvidia.json) · [Radeon history tests](../evidence/matrix-awakens/tsr-history-amd.json)

All 34 compute programs in the game’s 14 packaged TSR shader maps passed standalone DXIL validation, including all eight history-update permutations. Eleven were observed during the retail replay that reached entry 3385. [Per-program survey and retail trace](../evidence/matrix-awakens/tsr-maps.json)

At 3840×2160 on the RTX 4070 Ti SUPER, the temporal history update at global entry 3385 took 92 ms per dispatch. TuxBox shortened the conversion to half precision and removed redundant rounding after packed minimum and maximum instructions. The same dispatch then took 41 ms. All eight output textures matched byte-for-byte with both a camera cut and a continuing frame. [GPU timings and numerical checks](../evidence/matrix-awakens/tsr-history-performance.json)

The map survey uses either Python’s `cityhash` module or the Abseil development libraries with `pkg-config` and a C++ compiler.

```sh
python tools/check_frosty_tsr_filter.py /path/to/Mount --adapter 10de:2705 \
    --output build/tsr-filter.json
python tools/check_frosty_tsr_filter.py /path/to/Mount --adapter 10de:2705 \
    --update-history --output build/tsr-history.json
python tools/map_frosty_tsr.py /path/to/Mount --output build/tsr-maps.json
```

## View or update the map

Run `python tuxbox.py status --shaders` to see the recorded counts and the hashes of requested programs outside the preparation profile. The JSON inventory includes each requested hash, stage, matching archive entries and observed runs. A standalone DXIL pass means the translator emitted HLSL for that individual shader, Microsoft DXC compiled it, and the DXIL validator accepted the result. Linked stages and the rendered frame still need GPU checks.

To record a new observation, set `TUXBOX_TRACE_SHADERS=1` and `TUXBOX_LOG=1`, then launch Frosty on the desktop with `tools/run_frosty.sh --desktop`. Pass its saved `steam-default.log` below. Each trace label describes the entire run from process startup. The current trace identifies the stripped vertex, pixel, geometry, mesh and compute programs that enter TuxBox’s shader cache lookup. The inventory labels older logs that record only the first hit of each stage and a limited number of misses.

```sh
python tools/map_frosty_shaders.py /path/to/Mount \
  --trace "Menu=/path/to/steam-default.log" \
  --reuse-survey evidence/matrix-awakens/shader-inventory.json \
  --index build/frosty-shaders.csv --output build/frosty-shaders.json
```

The CSV contains one row for every shipped entry. It records the Unreal shader ID, native DXBC SHA-256 when present, map memberships, preparation status and trace labels. To repeat the frontend survey, replace `--reuse-survey` with `--compiler build/shader-compiler/x86_64-unknown-linux-gnu/release/xshader` after building the Linux compiler. This also fills in the per-entry `hlsl_emitted` CSV column. Add `--dxc /path/to/dxc` to compile the distinct observed programs and check their output with the adjacent `dxv` executable. The CSV records those results under `standalone_dxil_validated`. Add `--independent-compiler build/independent-compiler/x86_64-unknown-linux-gnu/release/xshader` to compare both compilers on every distinct program and fill the `independent_hlsl` column. With DXC enabled, this also validates all independently emitted programs, including ones that have yet to appear in a trace. Reusing a survey carries forward the aggregate results for matching archive hashes and the measured results for known requests.

To publish updated counts, run the mapper with all the trace labels you want to cover, use `evidence/matrix-awakens/shader-inventory.json` as the output, then regenerate the SVG with `python tuxbox.py status --svg docs/progress.svg`.
