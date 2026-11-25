# Measured homebrew results

Each column represents one run on its named host, provider and hardware. Each receipt records the guest output, exit status and binary hashes.

| Gate | [arch-rtx5090-valve-amd](../evidence/arch-rtx5090-valve-amd.json) | [bazzite-rtx3080ti-valve-nvidia](../evidence/bazzite-rtx3080ti-valve-nvidia.json) | [cachyos-rx6900xt-valve-amd](../evidence/cachyos-rx6900xt-valve-amd.json) | [cachyos-rx6950xt-valve-amd](../evidence/cachyos-rx6950xt-valve-amd.json) | [ge-nvidia](../evidence/ge-nvidia.json) | [lanes-checks-rtx4070tis](../evidence/lanes-checks-rtx4070tis.json) | [valdi-bazzite-rtx4080](../evidence/valdi-bazzite-rtx4080.json) | [valve-amd](../evidence/valve-amd.json) | [valve-nvidia](../evidence/valve-nvidia.json) | [wine-nvidia](../evidence/wine-nvidia.json) | Physical Windows x64 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Upper route and guest output | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pending |
| Lower route and collision | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pending |
| Invalid input exit status | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pending |
| Root signature 1.0 and 1.1 serialization, deserialization and errors | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| D3D12 and DXGI identifier data by name and ordinal | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| xg-dcc | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| xg-library | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| TuxBox XG texture layouts and element addresses | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| TuxBox XG layouts and addresses across 78 formats | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Game Runtime initialization, task delivery and cancellation | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Opus PCM, decoder pre-skip and command completion | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Xbox storage file reads, zlib and queue completion | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| CPU title and private heap creation | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| CPU heap attribute profiles | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| CPU virtual-memory allocation profiles | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| CPU scattered physical-page mapping | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Guest write-watch dirty-page query | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Guest allocator callback routing | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Memory allocation tag encoding | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| xmem-http | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| CPU virtual memory and image queries | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Xbox memory allocation and working set | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pending |
| xmem-partitions | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| CPU page allocation, remapping and release | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Desktop D3D12 pixel readback | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pending |
| Xbox root-signature import and compute readback | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pending |
| Blob, debug interface, four DXGIX entry points and Series CPU memory clear | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Xbox capability records compared with the selected adapter | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Placed resources and allocation offsets in a shared memory heap | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Virtual-address BC1 cubes and R8 textures across page changes | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Whole-buffer and multi-mip texture copies through Xbox command-list slot 84 | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Scissored triangle through Xbox graphics pipeline and draw slots | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Mesh and amplification shader dispatch on a mesh-tier adapter | Pass | — | Pass | — | — | — | Pass | — | — | — | Pending |
| Frame origins, timeout tokens, completed presentation events and display statistics | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Shader reads and writes through tables, including a late-copied texture view | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Direct, compute and copy queue synchronization | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| GPU timestamps, occlusion readback and descriptor heap handles | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Indirect dispatch and texture cache flushes between compute writes | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Xbox Device2 graphics pipeline stream, ownership and masked draw | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Xbox 0x20004 and default factories, compute queues and process debug flags | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Xbox command-list clear and ordered copy readback | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Fail | Pending |
| RGB10A2 display allocation and two guarded GPU readbacks | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Guest command rejection, fence isolation and list recovery | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Pass | Fail | Pending |
| Xbox PresentX and two screen-verified frames | Pass | Pass | Pass | Pass | Fail | Pass | Pass | Fail | Pass | Fail | Pending |
| Two RGB10A2 frames sampled from the desktop window | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| 4K RGB10 display scaled to a TuxBox window | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| 4K RGB10 display fitted to the title window’s monitor | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| 4K RGB10 display scaled inside a smaller title window | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| Lanes Xbox imports, course, board readback and gameplay replay | Pass | Pass | Pass | Pass | — | Pass | Pass | — | — | — | Pending |
| 8,388,608-move CPU replay | Pass | Pass | Pass | Pass | Pass | Pass | — | Pass | Pass | Pass | Pending |
| Xbox DXC utilities and binary blob lifetime | — | — | — | — | Pass | Pass | Pass | Pass | Pass | Pass | Pending |
| Native Scarlett XG texture layouts with Rust XMem | — | — | — | — | Pass | — | — | Pass | Pass | Pass | Pending |
| Lanes gameplay replay (earlier guest with desktop graphics) | — | — | — | — | Pass | — | — | Pass | Pass | Pass | Pending |
| Xbox DXC caller allocator and factory ordinals | — | — | — | — | — | Pass | Pass | — | — | — | Pending |
| Scarlett DXC compute shader and compiler diagnostics | — | — | — | — | — | Pass | Pass | — | — | — | Pending |
| xdxr-link | — | — | — | — | — | Pass | Pass | — | — | — | Pending |
| Bottom-of-pipe label writes with and without a flush | — | — | — | — | — | Pass | — | — | — | — | Pending |
| Compute and direct queues waiting on each other's labels | — | — | — | — | — | Pass | — | — | — | — | Pending |
| Compute queue frame submission | — | — | — | — | — | Pass | — | — | — | — | Pending |
| A skipped command leaving its list and fence write intact | — | — | — | — | — | Pass | — | — | — | — | Pending |
| CopyMemoryToMemoryX direction and contents | — | — | — | — | — | Pass | — | — | — | — | Pending |
| A waiting queue reading a texture another queue writes | — | — | — | — | — | Pass | — | — | — | — | Pending |
| ExecuteIndirect matching a direct dispatch | — | — | — | — | — | Pass | — | — | — | — | Pending |
| A guest texture copied back to title memory | — | — | — | — | — | Pass | — | — | — | — | Pending |
| Single, copied and batched builds through XboxBvhBuilder | — | — | — | — | — | Pass | — | — | — | — | Pending |
| Lanes frame, submission, fence handover, copy and pipeline timings | — | — | — | — | — | Pass | — | — | — | — | Pending |
| A console shader translated before launch, loaded from the prepared cache | — | — | — | — | — | Pass | — | — | — | — | Pending |
| A console shader translated by the host worker while the game runs | — | — | — | — | — | Pass | — | — | — | — | Pending |
| The worker's translation reused on the next launch without compiling | — | — | — | — | — | Pass | — | — | — | — | Pending |

Rebuild this table from the repository root with `python tuxbox.py status > docs/RESULTS.md`. A dash means that the gate was omitted from that run. [Current Lanes replay](../examples/lanes/README.md#deterministic-replay) · [Xbox DLL checks](LIBRARIES.md#run-the-provider-checks) · [Retail results and priorities](PROGRESS.md)
