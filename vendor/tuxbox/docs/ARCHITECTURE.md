# Execution and performance

Project TuxBox targets decrypted, unpacked Series X and Series S applications on x86-64 Linux and Windows. The first architecture choice is high-level compatibility. Keep ordinary guest x86-64 instructions executing on the host CPU. Intervene where the guest calls a console API, submits a console GPU command or depends on console OS behavior.

| Boundary | Initial implementation | Next observation that can change the design |
|---|---|---|
| Process and package | Read the declared executable from `MicrosoftGame.config`, or from the registry hive `microsoft_gameconfig.bin` when the XML is absent. Both paths support `--app-id` and require the declared x64 executable inside the package. Use the Windows PE loader or Wine/Proton for imports, TLS and unwind. | Record the first unsupported import, TLS failure or console subsystem assumption in a specific executable. Preserve the failing call as a small guest fixture. |
| Memory and platform calls | The Rust XMem provider exports the Xbox ABI by name and ordinal. XMem maps the observed allocation attributes to the Windows process heap, virtual reservations and 64 KiB page allocations. Frosty also uses TuxBox’s Game Runtime task queues and asynchronous storage reads. | Prove alignment and lifetime through guest-visible writes. Physical-page aliasing and GPU-visible addresses require a resource/VA mapping with explicit ownership. |
| Host GPU | One desktop D3D12 backend serves Windows drivers and the Proton D3D12-to-Vulkan path. The Xbox serializer forwards to the host. A guest device wrapper implements a bounded SDK 0x20002 profile and owns the mapping from guest queues, lists and resources to desktop D3D12. The single-plane PresentX path copies an RGBA8 texture to a DXGI swap chain. | Record the next shader, draw or presentation contract reached by a title. Extend the guest fixtures and replay them on each target adapter and provider. |
| Guest shaders | TuxBox loads prepared desktop DXIL keyed by the original shader bytes. The checkout includes the Rust RDNA2-to-HLSL translator. The launcher builds it locally, compiles profiled shaders with DXC and validates the DXIL before caching it. Linked vertex and geometry shaders use the pixel shader’s input signature. | Decode metadata, bindings and native RDNA2 instructions into a small typed representation. Translate one complete compute shader, compare all output and guard elements with an independent oracle, then repeat on another GPU vendor. |
| Assets, input and services | Inventory the supplied DLLs and asset types. Route simulator and graphics fixtures provide reproducible input and output. | Forward file access when the title already provides its own archive reader. Add storage codecs, GameInput, audio and local-user calls against a concrete title trace. |

A Series X/S CPU can execute the same instruction set as a target PC and still rely on incompatible syscall, memory or GPU behavior. Keep ABI variants keyed by observed caller metadata. An unfamiliar command or shader should yield a diagnostic with the build, interface or shader identity and enough context to create a reproduction. Returning success from a guessed implementation makes the next failure harder to identify.

## Runtime language

Implement new guest-facing runtime code in Rust. Keep each exported function's calling convention, data layout, error behavior and ownership contract explicit. Cargo builds static libraries for `x86_64-pc-windows-msvc`. The platform linker uses the export definitions to produce the DLLs that games load.

The graphics provider owns the guest COM objects in Rust. Resources, allocators, queues and command lists retain their guest device identity while their native handles stay inside the provider. Independently compiled guests check the ABI, GPU output and presentation through the same DLL a game loads.

## Performance measurements

1. **CPU baseline.** Run the same Route Game PE on each provider. The timed region replays 8,388,608 moves and must produce the reference checksum. Save QPC ticks, frequency, host CPU and build hash. Repeat at least five times for comparisons. Keep prefix creation, shader compilation and process startup outside the timed region.
2. **Submission and rendering.** For the first guest draw, time CPU submission using QPC and GPU execution with D3D12 timestamp queries and the queue frequency. Use the same resolution, payload, adapter, power policy and steady clocks for the equivalent desktop control. Collect frame-time p50, p95 and p99 separately from input-to-present latency.
3. **Shader and pipeline caching.** Key persistent results on guest ISA, resource metadata, wave mode, translator version, compiler version, backend and applicable driver/device identity. Run the scene once with an empty cache and again with a warm cache. Report compile count, time and stalls.
4. **Copies and synchronization.** Count uploaded bytes, barriers, submissions and waits. Reuse descriptors and resources through fence completion. Readback fences in correctness tests are intentional. Keep them outside timed frame loops unless the actual guest requires synchronization.
5. **A playable title.** Record a fixed replay, a screenshot or readback, sound/input checks and a frame-time trace over ten minutes. Compare physical Windows and Linux on the same machine before claiming an overhead figure. Consider a direct Vulkan or SPIR-V backend when profiling isolates a cost the shared D3D12 path cannot address.

Series S needs its own guest settings, memory budget and title evidence. A passing Series X capture or a PC with an AMD GPU covers only the configuration actually exercised.

### Recorded baselines

Each multi-sample CPU report contains the median in milliseconds and the individual QPC readings alongside the binary hash, CPU and provider. The reference checksum for the 8,388,608-move replay is `24316bf89788468d`. Compare matching builds, sample counts and workloads across runs. [Valve Proton CPU measurements](../evidence/valve-nvidia.json)

The GPU suites record correctness evidence. The homebrew frame submits two clears to one texture, compares both full snapshots and checks a separate compute-queue fence. The windowed suite adds independent screen samples after two calls to PresentX. On Linux it can run on an isolated Xvfb display. Retail evidence currently identifies the device-creation record and queue/list call sites in a particular executable. Per-provider results record which fixture checks actually ran. [Graphics fixture and reproduction](../runtime/d3d12/README.md) · [Retail call sites](../evidence/fortnite-device.json) · [Run results](RESULTS.md)

### Graphics run conditions

The November 21, 2026 Proton runs used independent 1280 × 720, 24-bit Xvfb displays. Valve Proton Experimental on the RTX 4070 Ti SUPER verified both displayed frames. On the Ryzen integrated GPU, offscreen submission and recovery passed, but PresentX returned `0x80070057`. Mesa reported that the virtual X server lacked DRI3. A follow-up on the physical desktop also failed its presentation gate and exited with code 3 before writing a receipt. GE-Proton 11-11 on the NVIDIA adapter submitted the frame, but the corner and center screen samples differed from the expected colors. The two presentation failures remain open. [Valve NVIDIA](../evidence/valve-nvidia.json) · [Valve AMD](../evidence/valve-amd.json) · [GE NVIDIA](../evidence/ge-nvidia.json)

Standalone Wine ran on the physical desktop with the NVIDIA adapter. The existing desktop clear, root-signature dispatch and Lanes checks passed. The Xbox-facing factory returned `0x80070057` at device creation before the three guest-frame checks could reach their commands or presentation. A separate control reproduced the same HRESULT by running the desktop clear with its feature-level request raised from 11.0 to 12.0, the level requested by the Xbox-facing provider. [Wine NVIDIA](../evidence/wine-nvidia.json)
