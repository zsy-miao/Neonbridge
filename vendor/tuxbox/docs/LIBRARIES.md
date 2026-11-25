# What Frosty needs from Xbox libraries

A working preview, movie, storage read or controller action counts the same whether TuxBox implements it, forwards a call to Proton or uses a DLL supplied with the game. The library section of the progress map counts each reviewed behavior once. Run `python tuxbox.py status --libraries` for the provider, test scope and result of each path. [Progress map](progress.svg) · [Path evidence](../evidence/matrix-awakens/library-paths.json)

## Media playback

Frosty loads an app-local `mfplat.dll`. TuxBox builds this DLL as a bridge to Proton’s Media Foundation. Most matching exports forward directly to Proton. TuxBox handles the Xbox-specific device and surface calls used to deliver video frames. Proton supplies the source reader, and the package supplies the H.264 and HEVC decoders. The moving menu preview and Wake Up cinematic exercise this path. Other exports present in the packaged MFPlat DLL have no bearing on that result. [Media bridge](../project_tuxbox/media.py) · [Decoder and GPU checks](../evidence/matrix-awakens/independent-media.json) · [Wake Up capture](../evidence/matrix-awakens/wake-up-cinematic.webp)

Audio has separate evidence. Frosty submitted a buffer to Proton’s XAudio2 at the menu, and the packaged AAC decoder produced soundtrack samples in a control. The Wake Up test was muted. A chase run with audible, synchronized playback would verify the remaining audio path and show whether the ACP adapter needs more work. [Audio and provider audit](../evidence/matrix-awakens/provider-audit.json)

## Graphics and platform services

| Game behavior | Working implementation |
| --- | --- |
| Submit and present frames | TuxBox’s `d3d12_xs.dll` creates the device and translates calls on its command lists, queues and resources. The reduced-settings run presented Wake Up and streamed in the bullet-time scene. [Desktop run](../evidence/matrix-awakens/wake-up-xg.json) |
| Address textures | TuxBox matched the packaged XG layouts and sampled addresses for all 2,039 distinct texture requests captured through Wake Up room activation. Separate tests cover the room’s four-sample depth and color targets. [Retail replay](../evidence/matrix-awakens/packaged-settings-room-xg.json) · [Depth](../evidence/providers/xg-library.json) · [Color](../evidence/providers/xg-msaa-color.json) |
| Create shader utilities | Frosty imports `DxcCreateInstance` from `dxcompiler_xs.dll`. TuxBox forwards it to desktop DXC. Translating the native RDNA2 shader program is separate work. [DXC adapter](../runtime/dxc/README.md) · [Shader translation](SHADERS.md) |
| Map memory, dispatch tasks and read assets | TuxBox supplies XMem, Game Runtime and storage. Frosty imports one storage factory. A queue obtained from that factory read and decompressed two game blocks with byte-for-byte matches. [Memory](../evidence/providers/xmem-pages.json) · [Queues](../evidence/providers/xgameruntime-queues.json) · [Storage](../evidence/providers/xcrdapi-storage.json) |
| Handle player input | TuxBox GameInput handled keyboard, mouse and controller input in the retail menus. [Input replay](../evidence/matrix-awakens/retail-mixed-input.json) |

With the packaged graphics settings, both XG implementations stopped at Lumen compute shader 219. That failure is tracked under shaders. Frosty also loads the game's GameChat2 DLL. The single-player runs have supplied no reason to implement a chat provider. [Packaged-settings comparison](../evidence/matrix-awakens/three-library-comparison.json) · [Loader trace](../evidence/matrix-awakens/provider-audit.json)

## Inspect imports and exports

Use the import list to find the entry point for a failing call. A factory can lead to many methods, and a runtime trace can reveal a DLL that the executable loads dynamically. Add a path to the game audit when a title relies on the behavior, and record the outcome or the failure.

```sh
python tools/library_gaps.py xcrdapi.dll mfplat.dll
python tuxbox.py status --d3d12-xs
```

For an ABI investigation, `python tools/library_gaps.py mfplat.dll --all-exports` shows the full export table. The package scan also retains names, ordinals, hashes and import relationships for the reviewed build. [Package inventory](../evidence/frosty-xbox-library-inventory.json) · [Provider export checks](../evidence/library-status.json)

## Run the provider checks

```sh
python build.py
python tuxbox.py verify --only xmem --only xg-provider --output build/provider-results.json
```

After collecting new evidence, refresh the inventory and regenerate the map.

```sh
python tools/scan_frosty_xbox_libraries.py /path/to/package --update-map
python tuxbox.py status --svg docs/progress.svg
```
