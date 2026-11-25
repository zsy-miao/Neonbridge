# Earlier Matrix Awakens experiments

This page records historical experiments, including reduced graphics settings and external reference runtimes. These are not requirements for the current launcher, and older failures may have been resolved. For the current status and original-settings setup, see [The Matrix Awakens](MATRIX_AWAKENS.md).

## Earlier comparison runs

The sections below document historical experiments and may describe reduced graphics settings, earlier failures or external reference runtimes. They are not requirements for the current launcher. Use the original-settings instructions above for a new test.

### Launch a reference configuration on Linux

To open the prepared copy with TuxBox’s graphics library, run `./tools/run_frosty_reference.sh --tuxbox-d3d12 --desktop`. The first isolated menu capture was taken 65 seconds after launch. The service keeps the game open when the terminal or editor closes. Follow the live output with `journalctl --user -fu tuxbox-frosty.service -o cat`.

After preparing the layout and prefix below, run `./tools/run_frosty_reference.sh --desktop`. This starts a user service named `tuxbox-frosty` and returns to the terminal. It keeps the game in its own memory group, with an 11 GiB memory ceiling and up to 2 GiB of swap. Follow the live console from any terminal with the command below. Press Ctrl+C to leave the viewer while Frosty keeps running. Use `systemctl --user status tuxbox-frosty` to check the service.

```sh
journalctl --user -fu tuxbox-frosty.service -o cat
```

Run `./tools/run_frosty_reference.sh` to see live output directly in the launching terminal. The launcher prints the run directory. It contains the TuxBox session log (`tuxbox.log`) and Proton’s original output (`steam-default.log`). The default shows game debug messages, exceptions and Wine errors. For loader and module tracing, run `WINEDEBUG=+timestamp,+loaddll,+module,+seh ./tools/run_frosty_reference.sh`.

The launcher uses the current X11/XWayland display, the isolated graphics prefix, and the working copy recorded in `build/frosty-native/latest-graphics-bringup`. The default launch uses the desktop audio output when its PulseAudio playback check succeeds. Earlier comparison runs explicitly disabled the Xbox startup-movie plugin. An unresponsive audio service can stall the title during mixer initialization. The launcher sends 100 milliseconds of silence to the default output and starts muted if playback times out after three seconds. On hosts with only `pactl`, it checks the sink query instead. Allow roughly 45–90 seconds for the intro and menu on the RTX. The verified reduced-effects configuration uses the media copy-list provider and the packaged video and audio decoders. To reproduce the headless engine test with the same launcher, pass `-nullrhi -nosound -DisablePlugins=XboxCommonGDKMoviePlayer`. The Windows package-token control is independent of both runs.

The launcher accepts keyboard, mouse and XInput controllers through Frosty’s gamepad interface. **I/K** selects a menu item. **Enter**, **Z** or a **left click** confirms the highlighted item. **Escape** or **X** sends B. **WASD** moves the left stick. Hold the **right mouse button** to aim and move the mouse to steer the right stick. While aiming, the **left mouse button** presses the right trigger. Releasing aim restores the cursor. Arrows also steer the right stick. I/J/K/L map the D-pad, Z/X/C/V map A/B/X/Y, M/N map Menu/View, Q/E map the shoulders and R/F map the triggers. With a controller connected, desktop inputs join the lowest connected XInput port. Other controllers keep their own slots. Setting `TUXBOX_KEYBOARD_GAMEPAD=0` leaves the physical XInput path active. The isolated Proton guest checked 48 ordered keyboard transitions, signed mouse motion on both axes, mouse buttons, return to neutral and cursor restoration. A Linux virtual Xbox 360 controller exercised Proton XInput and the same GameInput device while keyboard and mouse events were held and released. [Desktop input results](../evidence/matrix-awakens/desktop-gamepad.json) · [Controller and hotplug results](../evidence/matrix-awakens/xinput-desktop.json)

A retail menu replay on this machine confirmed I/K and the Xbox D-pad selected both rows. B toggled Controller Rumble off and on. With the controller still connected, a left click on the highlighted Start Chase action started the chase. The controller was a Linux uinput Xbox 360 device read through Proton’s XInput. [Retail input replay](../evidence/matrix-awakens/retail-mixed-input.json)

To prepare a working copy, start with the mounted-content graphics layout and prefix described below. Reflink the staged directory into a fresh directory on the same filesystem and copy its prefix. Point that prefix's `dosdevices/g:` at the new layout. Stage the sibling TomBox x64 release `xmem.dll` and `acphal.dll` beside the executable. Build and register the media adapters from the packaged codecs and Proton as described below. Build the mouse, keyboard and XInput provider using `tools/check_frosty_keyboard_gamepad.py` and place its `GameInput.dll` beside the executable. Also stage the x64 `dxil.dll` from the same DXC release as `dxcompiler.dll` for SM6 compilation. Use the local TomBox working-tree sources for the graphics patches. The local fork contains changes beyond commit `ca6cf6a6`. Its reference DLLs use the static CRT. Apply the ordered patch stack from a private copy of that checkout. Set `tuxbox` to this Project TuxBox repository's absolute path.

```sh
for change in indirect-signatures raytracing-prebuild raytracing-batch raster-profile \
              single-descriptor-copy bc-allocation graphics-stream descriptor-ranges \
              depth-plane raw-transcode mesh-stream visibility-atomic wave64-lightgrid \
              device-descriptor-api media-copy chase-stage-linkage merged-vertex-count \
              subsurface-setup placed-depth hair-cluster buffer-store-format-xy \
              texture-upload-snapshot compute-swizzle srv-read-state compute-exec-group \
              particle-sort-counter host-shader-link; do
    git apply "$tuxbox/tools/reference/tombox-$change.patch" || exit 1
done
RUSTFLAGS='-C target-feature=+crt-static' cargo xwin build --locked --release --target x86_64-pc-windows-msvc -p d3d12_xs
```

Place the resulting `d3d12_xs.dll` beside Frosty in the working copy. Historical comparison runs used overrides for conventional meshes, hair cards, or runtime ray-tracing geometry builds. The standalone launch instructions above use the original package settings.

To point the launcher at the prepared copy, write the absolute layout directory to a file named `stage` beside its `prefix` directory, then put their parent directory's absolute path in `build/frosty-native/latest-graphics-bringup`. To pin the tested Proton installation, save its absolute directory in a file named `proton` next to `stage`. An explicit `PROTONPATH` takes precedence. With neither setting the launcher searches the registered Steam libraries. Keep the original package separate from the staged copy.

### First-boot shader preparation

With TuxBox’s D3D12 provider, the launcher reads the profiled shader inputs from Frosty’s PAK and checks each original Xbox shader against its full SHA-256. It prepares desktop DXIL for the profiled compute and graphics programs, including the Slate shaders used in the first presented frame. Global compute entries 379, 402 and 414 share one compilation. The original ten-input compute trial took 47.45 seconds cold and 0.11 seconds warm. [Shader profile](../tools/reference/frosty-first-boot.json) · [Compute timings](../evidence/matrix-awakens/tuxbox-compute-preparation.json)

Set `TUXBOX_SHADER_PREP=menu` to prepare just the graphics and compute programs for menu testing. This profile includes the programs observed in TuxBox’s retail boot trace. The launcher matches each original shader in the PAK. Some geometry shaders require different output registers for different pixel shaders. The launcher also prepares those linked programs under the hashes of both cooked inputs. In a measured 122-output run, preparation took 60.69 seconds from an empty cache and 0.23 seconds on the next launch. In the Slate GPU control, both pixel shaders drew the expected red rectangle and preserved all 175 blue pixels outside it. [Menu preparation timings](../evidence/matrix-awakens/tuxbox-menu-preparation.json) · [Menu shader list](../tools/reference/frosty-menu-shaders.json) · [Cooked Slate GPU readback](../evidence/matrix-awakens/tuxbox-slate-draw.json)

With the reference graphics provider, the launcher prepares 11 profiled DXIL programs before Frosty starts, including the two large particle simulations used in the chase. It selects the profile by the retail executable and graphics provider hashes, reads the cooked shaders from the staged PAK and verifies translated HLSL against the runtime's cache keys. Missing entries compile with DXC and pass `dxv` before installation. Existing programs are checked by SHA-256. Structurally broken files move to a sibling `tuxbox-invalid` directory before compilation, so the runtime can rebuild a missing entry if preparation fails. An interrupted preparation resumes with the completed entries. The cold DXIL trial took 46.5 seconds. Checking the same 11 entries took 0.001 seconds. The title can still request additional shaders while playing. [Compilation results](../evidence/matrix-awakens/first-boot-shaders.json) · [Profile](../tools/reference/frosty-first-boot.json)

The GPU stage creates seven compute pipelines with the title’s D3D12 root signature and node mask. Five cache keys matched pipelines recorded during retail runs. On the RTX 4070 Ti SUPER the two largest pipelines took 26.8 and 24.7 seconds to create with empty driver caches. A second Proton process took 3.0 and 2.3 milliseconds for the same calls. The reduced-effects replay has yet to request those two grouped particle pipelines. These figures measure compilation time. In-game frame pacing remains unmeasured. [Pipeline timings](../evidence/matrix-awakens/first-boot-pipelines.json) · [Retail cache comparison](../evidence/matrix-awakens/retail-pipeline-keys.json)

An end-to-end preparation-only run with empty shader and GPU caches took 1 minute 59 seconds. Repeating that command with the same caches took 4.0 seconds, including the private display and launcher startup. The desktop launcher prepares shaders automatically for this build. When compilation is needed, a progress window shows the shader or GPU stage. Warm launches skip the window. Set `TUXBOX_SHADER_PREP_UI=0` to use terminal or service logs alone. With the game closed, `./tools/run_frosty_reference.sh --prepare-only` prepares the cache and exits. Use `--no-shader-prep` to bypass the stage for a run. The generic command accepts the same options before the target. A normal launch continues if prep fails, and a preparation-only run returns the error.

For a fresh checkout, use the same patched TomBox source as the staged provider. Apply the [host shader linkage patch](../tools/reference/tombox-host-shader-link.patch) to that checkout so the translator can match geometry outputs to their pixel inputs. Build the host translator with `cargo build --locked --release --target x86_64-unknown-linux-gnu -p tuxbox-xshader --bin xshader`. Set `TUXBOX_XSHADER` to the resulting absolute binary path, or install it in Project TuxBox at `build/frosty-shader-prep/xshader`. The first GPU preparation run builds the Rust worker with Cargo's `x86_64-pc-windows-msvc` target, the xwin SDK and lld-link. Later runs reuse it from the preparation cache. Cache misses also require liblz4 and the native DXC toolchain with `dxc` and `dxv`. To select a specific toolchain set `DXC` to its compiler executable.

## Measurements on the retail build

- **Indirect commands.** The original TomBox provider rejected Frosty's compute (105, 12 bytes), mesh (107, 16 bytes) and ray (9, 104 bytes with global root) descriptors. The adapted provider created all three. An independent compute dispatch verified all 81 readback words through the Xbox-facing and desktop devices. Subsequent TuxBox tests executed indirect mesh dispatches on NVIDIA and Radeon. The current TuxBox provider also executes indirect rays on both GPUs. [Ray dispatches](../evidence/matrix-awakens/ray-command-paths.json) [Baseline](../evidence/matrix-awakens/indirect-baseline.json) · [Patched provider](../evidence/matrix-awakens/indirect-candidate.json)
- **Raster renderer.** The patched runtime exposes a device profile without DXR, copies descriptor ranges through single-descriptor host calls and uses the host's allocation requirements for block-compressed textures. Frosty created two translated vertex/pixel pipelines, entered `InProgress`, and called PresentX twice on a 3840×2160 swap chain. A 24 fps screen capture showed flat areas only. Both sampled 960×540 display-buffer readbacks at the first present were uniformly black. The captured draw trace recorded 28 compute dispatches and two indexed draws. The original clear shader also ran in an independent readback with intact guards. [Graphics profile and retail trial](../evidence/matrix-awakens-launch.json) · [Clear shader check](../evidence/matrix-awakens/retail-clear.json)
- **Shaders.** Frosty registers 4,576 global shaders and 38,831 material shaders. The initial non-ray global survey translated 3,413 of 4,264 unique containers to HLSL, including 1,895 of 2,501 compute programs. The current compiler covers all 192 mesh entries. These counts measure HLSL generation. The GPU readbacks below cover individual programs. TomBox and DXC compiled one real global vertex entry and one compute entry to desktop DXIL. The 13,876-byte input to the first observed `DeserializeStateObjectX` call matched the archived ray-generation shader `OcclusionMainRG_d7cd43b4`. Its four native programs require ray-tracing instruction and continuation translation, plus the Xbox state-object inspection methods. [Shader measurements](../evidence/matrix-awakens/retail-shaders.json)
- **Geometry.** Frosty's runtime build setting requests host acceleration-structure sizes for the original mesh geometry. The provider returned 34,944 bytes for the first retail result and 6,656 for scratch. The Xbox-facing batch interface then recorded host geometry builds. An independent GPU test submitted two structures in one guest batch and verified four triangle hits and a miss, including instance, triangle geometry and distance. [BLAS sizing and traversal](../evidence/matrix-awakens/blas-batch.json)
- **GPU-built scenes.** Frosty uploads its instance records to GPU memory and can update their transforms before it builds the scene. TuxBox preserves those updates and replaces a mesh address when its native acceleration structure needs more space. Radeon needed 56,704 bytes for Frosty’s 31,232-byte sphere. NVIDIA fit it in place. The Xbox and desktop paths agreed on all 3,171 rays on each adapter. [GPU scene checks](../evidence/matrix-awakens/gpu-instance-relocation.json)
- **Engine and audio.** The D3D11On12 bridge reset Electra's actual Media Foundation device manager. In the separate GE decoder test, the first H.264 Sandbox frame matched FFmpeg byte for byte. [Valve media control](../evidence/matrix-awakens/media-valve.json) · [GE media control](../evidence/matrix-awakens/media-ge.json) The private Pulse endpoint and TomBox AcpHal let Unreal start XAudio2 and submit an audio buffer. Two streams connected. The loopback recorded silence. With rendering disabled, the title loaded `/Game/Map/P_Construct_WP`, entered `InProgress`, and subsequently faulted in `UInstancedStaticMeshComponent::BuildRenderData` on tick 2. [Engine and audio controls](../evidence/matrix-awakens/frosty-engine-control.json)

On the earlier raster run, Proton returned zero after Unreal requested a fatal exit. The desktop launcher now returns a failure status when the log records both events on the same thread. The clear shader control verified all 1,400 float and uint words on NVIDIA. Both graphics pipelines drew 81 red pixels and preserved 175 blue background pixels per draw on NVIDIA and AMD. [Clear shader](../evidence/matrix-awakens/retail-clear.json) · [NVIDIA draw](../evidence/matrix-awakens/retail-draw.json) · [AMD draw](../evidence/matrix-awakens/retail-draw-amd.json)

- **Depth and stencil.** The title successfully created its 1024×1024 depth/stencil texture with Xbox flags `0x01000006` and its shader-writable depth view. The wrapper copies depth in both directions across the corresponding resource-state transitions. A guest using the Xbox-facing calls wrote 209,715 pixels and checked every one of the 1,048,576 depth words and stencil bytes on both adapters. The title trace establishes creation. The compute writes come from the guest. [NVIDIA depth](../evidence/matrix-awakens/depth-wrapped-nvidia.json) · [AMD depth](../evidence/matrix-awakens/depth-wrapped-amd.json)
- **Nanite pages.** The loader created the compute pipeline for the shipped `TranscodePageToGPU` shader. In independent dispatches on both GPUs, all 2,048 words matched across copies, packed triangle indices and guards, including atomic writes across a word boundary. [NVIDIA page transcode](../evidence/matrix-awakens/transcode-nvidia.json) · [AMD page transcode](../evidence/matrix-awakens/transcode-amd.json)
- **Nanite rasterization.** The retail default Nanite path then requested a 680-byte mesh pipeline containing `HWRasterizeMS` and `HWRasterizePS`. The mesh shader has 1,708 bytes of native GPU instructions and stripped DXIL. A 22-second capture at 24 fps showed only flat fields. Turning off mesh shader rasterization in the working copy selects another cooked pipeline whose pixel shader uses a two-word image atomic. The subsequent shader control is recorded below. [Retail trial](../evidence/matrix-awakens-launch.json)

- **Mesh stream and visibility.** The wrapper translates the Xbox mesh and amplification subobject types and preserves both shader stages. A guest drew 64 expected texels with and without an amplification payload. Both original Nanite visibility pixel shaders now execute on NVIDIA and AMD with the shipping `R32G32_UINT` format. All 247 texels per draw sequence matched the independent 64-bit depth/ID comparison. The checkout now translates the retail mesh shader. See the Nanite results above. A separate Xbox draw test exercised the two-render-target pass that writes depth and stencil from visibility data on both GPUs. [Stencil export tests](SHADERS.md#pixel-shader-stencil-export) · [Mesh stream](../evidence/matrix-awakens/mesh-stream-retail.json) · [Mesh control](../evidence/matrix-awakens/mesh-stream-gpu.json) · [NVIDIA visibility](../evidence/matrix-awakens/visibility-nvidia.json) · [AMD visibility](../evidence/matrix-awakens/visibility-amd.json)
- **Light grid.** The original linked-list shader reserves space across a 4 × 4 × 4 group. TuxBox preserves Xbox lane order on both GPUs. GeForce and Radeon each matched all 8,192 output words across eight dispatches covering sparse lights, repeated captures, full buffers and empty inputs. Two productive groups allocated 96 distinct list entries in the expected order within each group. [AMD light grid](../evidence/matrix-awakens/lightgrid-amd.json) · [NVIDIA light grid](../evidence/matrix-awakens/lightgrid-nvidia.json)
- **Reduced-effects retail run.** The engine streamed sandbox vehicle packages, reached both intro game-mode and game-state `InProgress`, and opened a 3840×2160 HEVC stream. Valve Proton’s decoder enumeration returned no activation objects. The provider logged presentation number 300. The 52-second screen recording of the same configuration on the preceding provider contained 1,248 samples and three distinct images, all flat black or white. A repeat produced `VK_ERROR_DEVICE_LOST` followed by faults in D3D12 command-list workers. The first incorrect GPU command remains unidentified. [Trace and capture fingerprints](../evidence/matrix-awakens-launch.json)

- **Descriptor heaps and the intro movie.** Frosty selects the descriptor-copy API with a byte at offset `0x18` in the guest device. The GPU control verified 43 words with both vkd3d descriptor modes. An 85-second run with that provider reached 5,100 presentations. All 2,040 screen samples contained only flat fields and window transitions. With both original media decoders registered, the title subsequently received its first 4K D3D12 NV12 sample and requested Xbox copy-list IID `30bc77b9-8379-4b25-95de-b8588c42275d`. Its request returned `E_NOINTERFACE`. The surviving recording from this run contains 204 samples. [Descriptor control](../evidence/matrix-awakens/descriptor-api.json) · [Retail trace and capture fingerprints](../evidence/matrix-awakens/retail-intro.json)
- **Intro media controls.** Four 3840×2160 frames from the shipped logo movie matched FFmpeg byte for byte on the CPU and after GPU readback on Valve and GE. The packaged AAC decoder on Valve and GE produced 96 blocks of six-channel, 48 kHz audio. The largest absolute difference from FFmpeg was 0.00002472 on the normalized float samples. The first menu capture used `-nosound`. [Valve HEVC](../evidence/matrix-awakens/hevc-valve.json) · [GE HEVC](../evidence/matrix-awakens/hevc-ge.json) · [Valve AAC](../evidence/matrix-awakens/aac-valve.json) · [GE AAC](../evidence/matrix-awakens/aac-ge.json)

- **First visible retail menu.** The media copy-list adapter implements the Xbox command-list interface and copies both planes of decoded NV12 video. A GPU fixture verified every one of 3,572 luma bytes and 1,786 chroma bytes. The retail 50-second recording on Valve/NVIDIA then showed the Warner Bros. and Unreal Engine logos and the Start Chase / Settings menu. Down selected Settings in a separate input run. At this stage the preview logged an H.264 activation error. [GPU control](../evidence/matrix-awakens/media-copy.json) · [Reviewed retail run](../evidence/matrix-awakens-launch.json)

For a fresh title prefix, the Frosty preparation tool builds the Media Foundation bridge from the Xbox export table and Proton, stages the packaged HEVC, H.264 and AAC decoders, and registers their final paths. To rebuild just the media components, place TuxBox’s `xmem.dll` beside the shipping executable and run the following command. Repeat registration after moving the directory.

```sh
python -m project_tuxbox.media --destination "$shipping_directory" --packaged-os "$embedded_xvd" \
    --proton "$proton" --prefix "$title_prefix"
```

The standalone checks used the shipped HEVC intro, H.264 menu preview and six-channel intro soundtrack. Both Xbox MFPlat entries passed a GPU sample test with desktop and TuxBox D3D12 devices. The original Xbox names at ordinals 35 and 117 resolve to the surface-buffer and device-manager operations. [Independent media checks](../evidence/matrix-awakens/independent-media.json)

The menu preview uses the standard H.264 decoder CLSID. Earlier controls used both `Frosty/Content/Movies/Chase.mp4` and `Sandbox.mp4`. Four frames from each source matched FFmpeg by plane on the GPU. With this adapter, the retail capture shows moving video in the menu preview. The isolated stereo output matched the original intro soundtrack with a waveform correlation of 0.998 after alignment. [H.264 control](../evidence/matrix-awakens/h264-valve.json) · [Retail video and audio measurements](../evidence/matrix-awakens-launch.json)

### Start Chase

The controller test moved from Start Chase to Settings and back, then activated Start Chase with Z. Electra opened the first 3840×1600 Wake Up movie. The original decoder delivered both planes of four frames, including frame 240, byte-for-byte against FFmpeg. The first chase screen recordings displayed fields of color. [Retail trials](../evidence/matrix-awakens/chase-retail.json) · [Wake Up decode control](../evidence/matrix-awakens/wake-hevc-valve.json)

The opening chase now shows the falling green glyphs and the “Searching…” screen. Six captures from the retail window matched six distinct frames from the original movie, with spatial RGB correlations of 0.970–0.9996. The first 3840×1600 movie was followed by a 3840×2160 stream in the 150-second run. The patched renderer read both source planes correctly. Frosty supplies movie constants at a 16-byte-aligned offset that the RTX uniform-buffer path rounds down. The launcher builds an app-local Vulkan layer if needed, selects vkd3d-proton's descriptor-set path and uses its storage-buffer handling for those constants. The original converter then produced all 256 expected pixels in the GPU control. [Retail comparison](../evidence/matrix-awakens/retail-chase-cinematic.json) · [Converter control](../evidence/matrix-awakens/film-converter-nvidia.json)

One chase replay faulted in the placed-texture uploader during a CPU copy. The staged provider takes an owned snapshot of each readable subresource and uses those same bytes for hashing and detiling. Regression checks covered native and XG texture addressing, packed mips and eviction/reuse of the title's CPU pages. [Upload patch](../tools/reference/tombox-texture-upload-snapshot.patch) · [GPU readbacks](../evidence/matrix-awakens/placed-upload-snapshot.json)

Two material pipelines then exposed a mismatch between vertex and pixel shader registers. The provider links those interfaces and handles the merged-stage vertex count and attribute registers. GPU stream output matched the position, texture-coordinate and primitive-ID controls on both GPUs. The subsurface setup shader also passed all 1,024 channel checks on NVIDIA. The depth-texture fix preserves the title’s requested placement and represents its writable depth plane separately. At the retail size of 2976×1676, all 4,987,776 depth values and stencil bytes matched on both GPUs. [Stage linkage](../evidence/matrix-awakens/chase-stage-linkage.json) · [NVIDIA vertex](../evidence/matrix-awakens/merged-vertex.json) · [AMD vertex](../evidence/matrix-awakens/merged-vertex-amd.json) · [Subsurface setup](../evidence/matrix-awakens/subsurface-setup.json) · [NVIDIA depth](../evidence/matrix-awakens/chase-placed-depth.json) · [AMD depth](../evidence/matrix-awakens/chase-placed-depth-amd.json)

The strand-hair path also requested cluster culling and prefix sums. The prefix-sum GPU check read back 16,480 words from the two original shaders across eight cases. Both shaders now compile and produce the expected results. The desktop profile continues to use hair cards. [Culling results](../evidence/matrix-awakens/hair-cluster.json) · [Prefix sums](../evidence/matrix-awakens/hair-prefix.json) · [Related shader survey](../evidence/matrix-awakens/hair-prefix-survey.json)

After opening the second Wake Up movie, the title requested global compute shader 2227. Its 8×8 groups exchange data between lanes using 33 native swizzle instructions. The runtime now translates this pattern using shared memory. The original shader completed four GPU dispatch groups on both GPUs, and all 8,448 independently checked lane values matched on each. The title subsequently displayed Keanu Reeves in the red-armchair sequence. [RTX shader readback](../evidence/matrix-awakens/compute-swizzle-nvidia.json) · [Radeon shader readback](../evidence/matrix-awakens/compute-swizzle-amd.json)

A cold replay later activated ConstructFurniture. DXC spent 103.346 seconds compiling the 40,288-byte `SimulateMainComputeCS` shader. Unreal reported that the game thread had waited 120 seconds for the render thread. The working profile now allows 300,000 milliseconds for that wait. The title log confirmed it applied the setting. The dedicated prefix contains validated DXIL for material archive entries 37635 and 15458, cache keys `988fe343db918144` and `3efcdc501389cc3b`. On the warm replay the game still hit its 300-second timeout. A standalone guest returned the first cached shader and spent the rest of its 230-second test creating the host pipeline. [Cold and warm retail traces](../evidence/matrix-awakens/retail-chase-cinematic.json)

With both compiled shaders cached and Niagara GPU particles and compute effects disabled, a further run opened the later movies and activated `Cine_00_Wakeup_2_ConstructBulletTime`. A transition barrier then used Xbox state `0x120000c0`. Proton rejected bits `0x12000000` from both the before and after states, and Unreal reported `CommandList->Close() E_INVALIDARG`. That historical test used a reduced-effects file with Niagara overrides. Input-driven 3D gameplay is still unverified. [Retail replay](../evidence/matrix-awakens/retail-chase-cinematic.json)

Frosty’s own `SetSRVs` code combines the two high bits with ordinary shader-read states. The runtime now maps its two observed encodings to desktop all-stage read and non-pixel read. On the RTX and Radeon, a guest wrote and sampled two distinct 32×32 textures and checked every pixel through those transitions. Individual meanings for the high bits remain undocumented. [SRV state mapping and GPU results](../evidence/matrix-awakens/srv-read-state.json) · [Runtime patch](../tools/reference/tombox-srv-read-state.patch)

For the two large particle simulations, the final translator groups adjacent pure register writes with the same execution mask. Their RTX pipeline creation times were 26.0 and 23.8 seconds in isolated GPU tests. The runtime limits this optimization to the two measured native programs. Both compiled results are present in the prepared desktop prefix. [Pipeline timings](../evidence/matrix-awakens/simulate-exec-groups.json) · [Mask and memory-operation control](../evidence/matrix-awakens/compute-exec-mask.json)

Beyond the repaired transitions, the bullet-time scene requested the original `GenerateParticleSortKeys` compute shader (global entry 3333). Its elected lane updates one visible-particle counter for the 64-lane group. The new provider produced the expected sort keys, particle IDs and count for dense, sparse and empty inputs on both adapters. The controls verified all 1,152 output words per GPU, including slots around both 32-lane and 64-lane boundaries. [RTX results](../evidence/matrix-awakens/particle-sort-nvidia.json) · [Radeon results](../evidence/matrix-awakens/particle-sort-amd.json)

The combined provider passed the particle-sort failure in the retail title. The final Wake Up movie showed the crowd and then the street. The crowd capture matched frame 141 of the original WU_VID_0260 movie (0.997, 0.999 and 0.997 spatial RGB correlations). Frosty activated the city vista, lighting, chase intro and `Chase_Gameplay_hero` layers. The 423-second input-to-stop replay had no title fatal or shader translation failure. Some sampled transitions were black, and interactive 3D gameplay remains unverified. The desktop prefix received another 608 compiled shaders from this replay after DXIL validation. [Retail evidence](../evidence/matrix-awakens/retail-chase-cinematic.json)

The extended desktop run reached the chase dialogue and continued issuing graphics commands for 12 minutes 55 seconds after the chase layer activated. The dialogue capture shows a faint curve. More than 99.9% of its pixels measured 12/255 or lower in all three channels. Pressing the mapped Menu button during this sample produced no visible overlay. The test ended by reopening the title at its menu. [Timestamped replay](../evidence/matrix-awakens/retail-chase-cinematic.json)

In the bounded replay, the close of the fourth movie (`WU_0130`) provides a repeatable place to inspect the missing geometry. Samples at 105, 111, 117 and 123 seconds after Start Chase were black. The next movie opened around 228 seconds. For a separate diagnostic run, TomBox supports `TOMBOX_RTCAP=1` at process start and a one-frame trigger at `%TEMP%/tombox_capture_now`. The resulting `%TEMP%/tombox_rtcap/<n>/draws.txt` identifies bound shaders and targets, and `report.txt` gives end-of-frame channel ranges. Target numbers are local to each run. This diagnostic holds references to render targets, so memory measurements from it differ from ordinary desktop runs.

### What carries over to ProjectScarlett

Both packages include an 8 KiB `microsoft_gameconfig.bin` registry hive. Each declares the same executable, app ID and device family as its XML manifest. Both request advanced Series X memory and SMT. ProjectScarlett also requests advanced Series S memory. TuxBox can use the binary declaration to select the game when the XML is absent. Run `python tuxbox.py inspect <package>` to list its binary files, or inspect an individual `.bin` with `--json` to see the decoded fields. [Package binary inventory](../evidence/package-binaries.json)

Frosty's shipping symbol database describes 769,032 address ranges. The two symbol directories contain byte-identical copies for each of the three builds. The shipping symbols identify the caller of D3D12 slot 44 as `FXboxCommonD3D12Adapter::CreatePlacedResourceX`. Both `Catalog000.bin` files declare PE subsystem 17 (Xbox Code Catalog) and the Force Integrity flag. They contain one read-only section with no entry point or code. Their internal payload remains undecoded. Frosty's hidden `.unsync/manifest.bin` belongs to Unreal's build-delivery tooling. [Call-site evidence](../evidence/matrix-awakens/d3d12-startup.json) · [Windows SDK subsystem definitions](https://github.com/microsoft/win32metadata/blob/main/generation/WinSDK/RecompiledIdlHeaders/um/winnt.h) · [Epic's Unsync overview](https://dev.epicgames.com/community/learning/tutorials/8JYW/epic-for-indies-setting-up-an-unreal-engine-studio-the-epic-way)

To look up an address from a game log, subtract the module's load address and pass the result to `--rva`. Repeat the option to look up several addresses in one pass. The slot 44 call below resolves to offset `0x90` within the function. Gaps and overlapping ranges are reported explicitly.

```sh
python tuxbox.py inspect "$MOUNT/Symbols/Frosty-XSX-Shipping-Symbols.bin" --rva 0x5759ce0
```

All 16 supplied DLLs loaded from their packaged paths in separate x64 Windows processes on Valve Proton. Three import the staged Frosty graphics or memory providers. The probe checked every named import from those providers. `XboxBvhBuilder.dll` resolves root-signature serialization and `XMemVirtualQuery`. The FidelityFX upscaler resolves the root-signature deserializer and memory allocation/statistics exports. `Bink2Scarlett.dll` resolves `XMemAlloc` and `XMemFree`. The executable is absent, so its device ABI and startup path remain unknown. [All 16 library loads](../evidence/scarlett-all-library-load.json) · [Probe](../tools/check_scarlett_libraries.py) · [Package inventory](../evidence/package-inventory.json)

TomBox’s `fidelityfx_probe.exe --verify-provider` obtained all four expected render resolutions from the supplied FidelityFX loader and directly from the upscaler. Its `bvh_sort_probe.exe XboxBvhBuilder.dll` executed the packaged geometry kernels and verified a four-pass sort of 16,384 key/index pairs on the Radeon adapter. The NVIDIA `--records32` profile verified guarded record kernels and prefix scans. These controls used the same descriptor-heap provider as the 85-second Frosty capture. [Functional controls and binary hashes](../evidence/scarlett-library-controls.json)

## Linux run with the reference runtimes

This is a historical research setup. The current launcher builds its providers from this checkout and does not require these reference runtimes.

With Valve Proton Experimental, ScarlettX's x64 Game Runtime took Frosty through `InitializeApiImplEx(10003, 7032, 2)` into Unreal startup. The TomBox desktop GDK alternative failed earlier with `0x80040154` (COM class unregistered). With ScarlettX and the original XCRDAPI, mapping the package root as `G:` let Frosty enumerate the archives and exposed a null DirectStorage factory in `FXSXFileIoStoreImpl`. Substituting TomBox's x64 XCRDAPI and the x64 DLLs from Microsoft.Direct3D.DirectStorage 1.3.0 got the title through I/O dispatcher initialization, mounting the Frosty IoStore container, and loading engine configuration. [Reference run](../evidence/matrix-awakens/reference-runtime.json)

### From content mounting to GPU device creation

Two WineDbg samples placed the main thread in `XTaskQueueDispatch`, called by `LocalGDKTask` from `FGDKUserManager::AddUserInternal`. Frosty attempts to terminate the queue before it dispatches the manual completion port with an infinite timeout. The title's `XTaskQueueTerminate` wrapper calls threading interface slot 22. ScarlettX 3eb6948 provided a working termination function through its flat export, but slot 22 still returned `E_NOTIMPL`. The patch connects the slot to that function. In a bounded standalone control the original DLL waited 500 ms. The patched DLL returned from dispatch immediately. On the real title, adding a user still failed with `E_NOINTERFACE`, and Unreal continued into graphics startup. [Queue patch](../tools/reference/scarlettx-task-queue-terminate.patch) · [Thread samples and control results](../evidence/matrix-awakens/reference-runtime.json)

In the earlier Project TuxBox D3D12 build, the next call failed because `D3D12XboxSetProcessDebugFlags` was not exported. With the sibling TomBox release build of `d3d12_xs.dll`, the title created the wrapped device, named the RTX 4070 Ti SUPER, and queried feature support. vkd3d-proton then rejected indirect argument type 105 in the compute command signature. The later read fault occurred during error handling. Both graphics runs produced the same diagnostic, fault RVA and exit code. A later run with the TuxBox device factory accepted Frosty’s release `0x20004` record and reached the root device’s `QueryInterface(IDXGIDevice1)`. The game received `E_NOINTERFACE`. After TuxBox implemented that interface and its `GetAdapter` call at slot 7, a second run identified the RTX 4070 Ti SUPER and 16,376 MB of dedicated video memory. The runtime stopped at an unsupported device method before a frame. The title’s call sequence points to slot 13, `CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS)`, with a 64-byte buffer. [Earlier trials and provider hashes](../evidence/matrix-awakens-launch.json) · [Isolated TuxBox device trial](../evidence/matrix-awakens/tuxbox-device-factory.json)

With the TuxBox D3D12 provider, Frosty initializes descriptor caches and memory pools, creates 4K display buffers and reaches the viewport’s frame timing calls. It now passes both known slot 44 placement failures, a 32 × 32 six-mip BC1 texture and a 256 × 1792 RGBA16F texture. GPU readback checks cover aliased pages, late commits, replacement after recording and all six faces of a BC1 cube. [Placement controls](../evidence/providers/d3d12-placement-valve.json) · [Packaged XG comparison](../evidence/providers/xg-placement-layouts.json)

With the XML removed from a private game layout, TuxBox selected the shipping executable from `microsoft_gameconfig.bin`. Frosty loaded its first prepared compute shader, bound its UAVs, opened Construct and reported `InProgress`. Its 712-byte graphics pipeline stream goes through Xbox device slot 86 on `ID3D12Device2`. TuxBox created the pipeline, submitted the indexed draws and presented a 3840×2160 display buffer. Frosty read the frame statistics and advanced past creation of its 1024×1024 depth and UAV texture. The Xbox media reset also returned success. Each trial used a private layout and prefix with the existing reference platform libraries. [Graphics startup history](../evidence/matrix-awakens/d3d12-startup.json) · [Menu trials](../evidence/matrix-awakens/tuxbox-menu-startup.json)

To run with TuxBox’s D3D12 provider, make a separate copy of the working reference layout and prefix. Give the new working directory its own `stage` and `proton` files and point its prefix’s G: drive at the copied layout. Build the provider with `python build.py d3d12` and copy `build/d3d12-smoke/d3d12_xs.dll` beside the shipping executable. Rebuild the Media Foundation bridge with HEVC surface support using `tools/check_frosty_hevc.py` and put the resulting `mfplat.dll` beside it. Keep the stage’s existing decoder registration and companion media DLLs. Record the absolute path to this new working directory in `build/frosty-native/latest-tuxbox-d3d12`.

The launcher uses the same Proton and Vulkan settings as the reference run. The menu shader profile includes captured vertex and pixel inputs, the geometry outputs linked to their matching pixel shaders, and compute shaders requested during startup. To test binary-config discovery, move `MicrosoftGame.config` out of the copied layout first. [First-boot shader manifest](../tools/reference/frosty-first-boot.json)

```sh
./tools/run_frosty_reference.sh --tuxbox-d3d12 --prepare-only
./tools/run_frosty_reference.sh --tuxbox-d3d12 --desktop
journalctl --user -fu tuxbox-frosty.service -o cat
```

| Command signature in Frosty's Xbox adapter | Guest type | Byte stride | TuxBox guest result |
|---|---:|---:|---|
| Indirect compute dispatch | 105 | 12 | Created and executed, 64 GPU words checked |
| Indirect mesh dispatch | 107 | 16 | Two indirect dispatches and a zero-count control passed GPU readback on both adapters. |
| Indirect ray dispatch | 9 | 104 | A signature created with the guest global root dispatches from a 104-byte buffer offset. Occlusion and intersection match desktop D3D12 on both GPUs. [GPU checks](../evidence/matrix-awakens/ray-command-paths.json) |

The reproducible runtime map covers 140 linked GDK wrappers across 19 class GUIDs and 20 interface IDs. Each row contains the symbol, query location, class, interface, and method slot. Runtime observations cover a subset of those interfaces. Regenerate the CSV with `python tools/map_frosty_runtime.py "/path/to/Frosty-package"`. The graphics profile checker separately verifies the three descriptor types, strides and device calls against this build. [GDK wrapper map](../evidence/matrix-awakens/runtime-wrappers.csv) · [Graphics profile](../evidence/frosty-device.json)

To rebuild the reference Game Runtime, set `SCARLETTX_DIR` to a checkout of ScarlettX commit `3eb694821b51c862c52b1a98ac382ba40eb07f74`. From the TuxBox root, check the source blob and apply the adapter. The resulting source blob is `05464a3fee32ba9e6701eb8f8bea33eaef3768b1`.

```sh
test "$(git -C "$SCARLETTX_DIR" hash-object src/shims/xgameruntime_shim.cpp)" = 3c22b36ab2232948b92f1461c50cf201c3f27c96 &&
    git -C "$SCARLETTX_DIR" apply --unidiff-zero "$PWD/tools/reference/scarlettx-task-queue-terminate.patch"
```

Build ScarlettX’s x64 `xgameruntime` target. The recorded DLL used clang-cl, C++20, the static MSVC CRT and `SCARLETTX_BUILDING_XGAMERUNTIME=1`. To repeat the earlier unmodified-TomBox graphics control, use the staged title and its dedicated prefix. The prefix maps the package root to `G:`.

```sh
control=$(cat build/frosty-native/latest-wait-map)
stage=$(cat "$control/graphics-stage")
logdir=$(mktemp -d "$control/replay.XXXXXX")
xvfb-run -a unshare --user --map-current-user --net env -u WAYLAND_DISPLAY UMU_RUNTIME_UPDATE=0 \
  PROTON_LOG=1 PROTON_LOG_DIR="$logdir" WINEDEBUG=+timestamp,+module,+loaddll,+seh \
  timeout -k 4 40 python tuxbox.py run --backend proton \
  --prefix "$control/graphics-prefix" "$stage"
```

## What changed on the retail executable

| Trial | Observed result |
|---|---|
| Stock Valve Proton | Six directly imported DLLs missing. The loader stopped before entry. |
| TuxBox XMem, D3D12 and optional desktop DXC | Three DLLs missing. The title resolves the D3D12 device GUID and the DXC factory. |
| Add the game's embedded XG and the two XMem allocation names it imports | Two DLLs missing. A separate homebrew guest verifies XG texture-layout queries. |
| Also stage the embedded XCRDAPI and AcpHal | All directly imported modules load. Frosty enters its static initializers and stops in Unreal's allocator on the working-set query. |
| Add the Rust working-set query, then CPU virtual reserve and commit | Frosty passes both allocator calls and enters WinMain. The title reports that the Game Runtime is missing. |
| Install the embedded Game Runtime and GameConfigHelper in an isolated test prefix | Both modules load from System32. Game Runtime initialization fails on Valve and GE with the same HRESULT. |

In the embedded-runtime trial, seven direct symbols remained unresolved in loaded modules. Three belong to XMem physical pages, three to D3D12, and one to Media Foundation. XCRDAPI and the embedded Game Runtime also have unresolved dependencies of their own. A module load does not establish that audio, storage, or graphics work in the title. The trace checker records the title's own diagnostic, terminal exception and stack frame independently of those import counts. [Current receipts](../evidence/matrix-awakens-launch.json)

## Embedded Game Runtime comparison

Frosty searches System32 for `xgameruntime.dll`. Without it, the title takes the specific missing-runtime branch in WinMain. With the supplied DLL, initialization proceeds to the runtime's process-token security-attributes query. The native runtime performs a size-only `NtQueryInformationToken` call for `TokenSecurityAttributes` (class 39). Both Proton builds return `STATUS_NOT_IMPLEMENTED`, which maps to Win32 error 1 and the exact HRESULT reported by Frosty. An independent guest queried only the required size. [Control results](../evidence/matrix-awakens/token-query.json) · [Reproduce the query](../tools/check_token_query.py). [Microsoft GDK error codes](https://learn.microsoft.com/en-us/xbox/gdk/docs/reference/errorcodes)

## Compare the process-token query on Windows

The prepared ZIP is in `build/token-query/token-query-control.zip`. Rebuild it with `python tools/check_token_query.py --prepare-control`. It contains the x64 executable, a loose desktop package manifest and the PowerShell comparison. Loose registration requires Developer Mode on Windows 10 x64 build 19041 or later, or Windows 11 x64. [Microsoft's loose package registration guide](https://learn.microsoft.com/en-us/windows/apps/develop/testing/loose-file-registration)

Copy the prepared ZIP to the Windows system. From its directory, extract the probe to a local folder and run the comparison in 64-bit PowerShell.

```powershell
$control = Join-Path $env:TEMP ("tuxbox-control-" + [guid]::NewGuid())
Expand-Archive -LiteralPath .\token-query-control.zip -DestinationPath $control
& (Join-Path $control 'Compare-TokenQuery.ps1') -HostKind virtual-machine -OutputPath (Join-Path $PWD 'windows-vm.json')
```

For a physical x64 Windows control, use `-HostKind physical` and a separate output file.

The script runs the probe before registration and through the registered console execution alias. It verifies that the first process reports `APPMODEL_ERROR_NO_PACKAGE` and the second reports the registered package full name. It removes its registration during cleanup. [Windows app execution aliases](https://learn.microsoft.com/en-us/uwp/schemas/appxpackage/uapmanifestschema/element-uap5-appexecutionalias)

Compare `ntstatus` and `required_bytes` with the matching `exe_sha256`. The prepared executable returned `0xc0000002` and zero bytes on both Proton builds. Both direct launches reported `APPMODEL_ERROR_NO_PACKAGE`. The earlier probe used a different binary and measured the same size query. To repeat the matching Linux observations, run `python tools/check_token_query.py --proton "$VALVE" --proton "$GE" --output build/token-query/proton-control.json`. [Prepared binary's Proton results](../evidence/matrix-awakens/token-query-control-proton.json) · [Earlier size-only results](../evidence/matrix-awakens/token-query.json)

## Verified homebrew behavior

- The game's `xg_xs.dll` uses TuxBox's Rust allocator. Four RGBA8 and BC7 descriptors exercise linear and tiled layouts and mip chains. The guest checks reported size and alignment, complete active plane records, repeated queries and buffer guards. Loading XCRDAPI or AcpHal in the retail run establishes no equivalent read or audio result. [Fixture](../runtime/xg/guest.cpp) · [Provider results](RESULTS.md)
- Frosty requests an owned DXC blob in its pipeline-cache code. Valve and GE preserve all 257 tested bytes after the source and utility are released. The second DXC factory uses a caller-supplied allocator and releases its allocations. [Allocator comparison](../evidence/providers/dxc-allocator.json) · [Valve fixture](../evidence/matrix-awakens/valve-dxc-utils.json) · [GE fixture](../evidence/matrix-awakens/ge-dxc-utils.json)
- TuxBox accepts the tested 64-byte `0x20002` and 72-byte `0x20004` release records. Frosty’s AutoHDR-off factory call created a guest root device. [Static graphics profile and call sites](../evidence/frosty-device.json)

## Reproduce on Linux

From the repository root, set `SOURCE` to the original Mount directory, `SYSTEM` to its sibling `EmbeddedXvd/Windows/System32`, `WORK` to a fresh directory on the same copy-on-write filesystem as the dump, and `PROTON` to your Proton distribution. The verified DXC provider is the Windows x64 `bin/x64/dxcompiler.dll` from Microsoft's v1.8.2505.1 release, placed in `build/dxc-utils/`. Use a dedicated prefix for these two console runtime DLLs. [DXC release](https://github.com/microsoft/DirectXShaderCompiler/releases/tag/v1.8.2505.1)

```sh
python build.py
PREFIX="$PWD/build/prefix-frosty-test"
python tuxbox.py verify --proton "$PROTON" --prefix "$PREFIX" --only xmem --output build/frosty-xmem.json
cp -a --reflink=always "$SOURCE" "$WORK/Mount"
XSX="$WORK/Mount/Frosty/Binaries/XSX"
cp build/xmem-smoke/xmem.dll build/d3d12-smoke/d3d12_xs.dll \
  build/dxc-utils/dxcompiler_xs.dll build/dxc-utils/dxcompiler.dll "$XSX/"
cp "$SYSTEM/xg_xs.dll" "$SYSTEM/xcrdapi.dll" "$SYSTEM/AcpHal.dll" "$XSX/"
cp "$SYSTEM/xgameruntime.dll" "$SYSTEM/gameconfighelper.dll" "$PREFIX/drive_c/windows/system32/"
mkdir -p build/frosty-logs
rc=0
xvfb-run -a unshare --user --map-current-user --net env -u WAYLAND_DISPLAY \
  UMU_RUNTIME_UPDATE=0 PROTON_LOG=1 PROTON_LOG_DIR="$PWD/build/frosty-logs" \
  WINEDEBUG=+timestamp,+loaddll,+module,+seh \
  python tuxbox.py run --proton "$PROTON" --prefix "$PREFIX" "$WORK/Mount" || rc=$?
python tools/check_loader_trace.py "$SOURCE" --prefix-system32 "$PREFIX/drive_c/windows/system32" \
  --run current build/frosty-logs/steam-default.log "$rc"
```

To check TuxBox’s own texture address computer, build it with `python build.py xg` and run `python tuxbox.py verify --only xg-provider --output build/xg-provider-results.json`. For comparison with the supplied library, copy `$SYSTEM/xg_xs.dll` to `build/xg/` and run `python tuxbox.py verify --only xg --output build/xg-results.json`. The checked-in evidence contains hashes and measurements. The external DLLs stay in your dump, build directory or test prefix.
