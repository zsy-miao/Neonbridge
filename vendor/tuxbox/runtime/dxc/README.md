# Shader compiler

Frosty imports `DxcCreateInstance` from `dxcompiler_xs.dll`. The PE forwarder routes that export to the x64 desktop `dxcompiler.dll`. The CPU fixture requests `CLSID_DxcUtils` and calls `IDxcUtils::CreateBlob` with a binary input containing every byte value. It overwrites and frees the source, releases the utility and original blob interface, then checks the size and reads and writes all 257 bytes through a retained `IDxcBlob`.

The guest checks the Series DXC factories at ordinals 3 and 4 and passes a counting allocator to `DxcCreateInstance2`. The compilation check builds a Shader Model 6.0 compute shader with a root signature, inspects the DXIL output, and requests a missing entry point to exercise compiler diagnostics.

Run `python3 build.py dxc-utils` with the project's x64 Windows SDK toolchain. Extract `bin/x64/dxcompiler.dll` from the Microsoft Windows DXC release into `build/dxc-utils/` next to the guest and forwarder. The verified DLL is version 1.8.2505.32, SHA-256 `5888d3e590f5bfd8743484e7f31313996734adec5741506271a5c6368cbe8ba9`. [Microsoft DXC release v1.8.2505.1](https://github.com/microsoft/DirectXShaderCompiler/releases/tag/v1.8.2505.1)

On Windows, run `build\dxc-utils\dxc-utils.exe`. On Linux, use the project launcher with your Proton path.

```sh
python3 tuxbox.py verify --only dxc-utils --proton "/path/to/Proton" \
    --prefix build/prefix-dxc --output build/dxc-utils-results.json
```

On Scarlett, TuxBox implements both XDXR factories in Rust. The returned compiler links portable ray libraries, selects and renames exports, and prepares shader tables for desktop DXR. The linker guest exercises individual and grouped libraries and both shader table entry points. Five translated Frosty miss shaders retained the same entry-point bodies as the Xbox control. A ray-generation shader wrote the expected 64 GPU values with global and local root UAVs. [Ray compiler results](../../evidence/providers/xdxr-link.json)

For a miss shader that only reads and writes its payload, TuxBox can also emit a Series continuation collection. It preserves the shader body, moves up to five scalar payload words through the caller's driver runtime, and returns to the state on the continuation stack. The Series linker accepted these collections for all five Frosty miss shaders. Each linked scheduler preserved its payload write. Shader suspension, traversal and the console scheduler's GPU function pointers remain open. [Replay the cross-link control](../../tools/check_xdxr_continuation.py)

Shader table support currently covers reflected Shader Model 6.3 resources and the root bindings listed in the results. The standard DXC exports produce desktop DXIL. [Standard compiler controls](../../evidence/providers/dxc-factories.json)
