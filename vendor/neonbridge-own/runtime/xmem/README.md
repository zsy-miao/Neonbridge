# Xbox memory provider

The [Rust provider](src/lib.rs) implements CPU allocation and physical-page profiles. `XMemAlloc` and `XMemFree` use the default path when no hooks are installed; the default exports remain separately callable. Guest callbacks, their arguments and reset behavior have been checked against the supplied DLL. GPU-visible mappings remain open. [Allocation hooks](../../evidence/providers/xmem-hooks.json) · [Console export inventory](../../evidence/library-status.json)

| Attributes | Guest request | CPU backing | Verified alignment |
| --- | ---: | --- | ---: |
| `0x0480009e` | 12 bytes | Process heap | 16 bytes |
| `0x30804000` | 65,536 bytes | A committed virtual memory reservation | 64 KiB |

Both size/attribute pairs were observed in the fork's Coin Run allocation log. Its retail analysis independently recorded 469 texture allocations using the second attribute word. Win64 supplies 16-byte heap alignment and aligns new reservations to the system allocation granularity. Returning a CPU address for the second form establishes only CPU access and alignment. Binding the allocation to a GPU resource needs a separate test. [Previous homebrew allocator call](https://github.com/brcly/TomBox/blob/ca6cf6a6b44e36d3ca4420db55a660a7bc6d6d5a/crates/compat/xmem/examples/coin/game.rs) · [Title trace](https://github.com/brcly/TomBox/blob/ca6cf6a6b44e36d3ca4420db55a660a7bc6d6d5a/docs/DEVELOPMENT_LOG.md) · [Windows heap alignment](https://learn.microsoft.com/en-us/windows/win32/api/heapapi/nf-heapapi-heapalloc) · [Windows virtual memory reservations](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-virtualalloc)

The packaged HEVC, H.264 and AAC decoders request the process heap with attributes `0x04820000`. The allocator guest checks this form and the packaged decoder controls compare real video frames and PCM output with FFmpeg. [Independent media controls](../../evidence/matrix-awakens/independent-media.json)

The packaged D3D12 drivers also use 16-byte-aligned process heap allocations for initialization (`0x04800092`), blob objects (`0x04800040`), and writable blob data (`0x04800078`). Both drivers created and released blobs with the TuxBox allocator on Linux. [D3D12 control runs](../../evidence/providers/d3d12-exports.json)

Callers must supply the original attributes when releasing a live allocation. A zero-byte request returns null. Unknown attribute words return null with `ERROR_NOT_SUPPORTED`. For ordinary heap attributes, a null free is forwarded to the host heap; its console-specific error behavior remains unverified.

Frosty requests `XMemGetWorkingSetStatistics` (ordinal 16) with selector `0x20000001` during static initialization. Its 120-byte result uses the process commit counters and host-available memory to estimate a title budget. The guest checks the result against host measurements after touching an 8 MiB allocation.

Frosty's CPU binned allocator calls `XMemVirtualAlloc` (ordinal 31) with profile 1. The fourth argument selects the profile and the fifth supplies page protection. The provider maps reservation, commit and top-down placement to Windows. The guest checks that a committed page is writable and its reserved neighbors remain inaccessible. Console-specific GPU page flags still need a separate implementation. [Virtual memory provider](src/virtual_memory.rs)

Frosty reserves its physical-page apertures with `XMemVirtualAlloc` profile `0x23`. It allocates 64 KiB page tokens with flags `0x20400000`. Each batch uses a demand-committed host section. The same page can be mapped into multiple addresses, and CPU writes through one address are immediately visible through every alias. Releasing the physical token removes its active section views and restores the reservation for `VirtualFree`. [Title call sites and driver ABI](../../evidence/matrix-awakens/xmem-physical-pages.json) · [CPU page controls](../../evidence/providers/xmem-pages.json)

The guest also exercises ordinary caller-owned Win32 reservations. Those use one live CPU mapping per token and preserve bytes during explicit unmap and remap. GPU page-table mapping and DirectStorage access still need separate controls. [CPU scatter controls](../../evidence/providers/xmem-scatter.json) · [Microsoft’s Xbox GPU mapping sample](https://learn.microsoft.com/en-us/samples/microsoft/xbox-gdk-samples/advancedesram/)

The provider also supports CPU scatter remapping, write-watch suspension and ordinary title and private heaps. `XMemVirtualQuery` returns the host region and Xbox title flags for supported CPU addresses, including released reservations. Its guest checks page protection, image memory, write-watch state, output guards and unsupported addresses. [Scatter](../../evidence/providers/xmem-scatter.json) · [Write watch](../../evidence/providers/xmem-watch.json) · [Heaps](../../evidence/providers/xmem-heaps.json) · [Virtual queries](../../evidence/providers/xmem-query-http.json)

Allocator options control validation, direct allocation tags, heap notifications and the page-size policy. A tagged direct CPU allocation exposes its packed and decoded tag through `XMemVirtualQuery`. The desktop retail profile leaves automatic graphics write watch disabled. [Checked option behavior](../../evidence/providers/xmem-watch.json)

## Allocation pools and diagnostics

CPU allocation pools commit and retain memory in 4 MiB blocks. Setting hysteresis changes how many free blocks a pool keeps. Preallocation fills that cache, and allocation statistics report its size, high-water extent, commits and decommits. The free-page query reports the ordinary CPU backing available to the physical-page allocator. The guest-facing history page retains eight working-set samples at half-second intervals. [Provider and reference controls](../../evidence/providers/xmem-pools-diagnostics.json)

## HTTP callbacks

`XMemSetWinHttpHooks` installs seven guest callbacks. The companion `tuxbox_xhttp.dll` publishes the Xbox import table and forwards each unset callback to desktop WinHTTP. Source-built guests can link `tuxbox_xhttp.lib` for these seven Windows SDK names and ship the companion beside `xmem.dll`. An existing game importing `winhttp.dll` directly needs a loader that consumes the Xbox import table to use the callbacks. [Guest and host session checks](../../evidence/providers/xmem-query-http.json)

## Build and run

Install the Rust toolchain and add the `x86_64-pc-windows-msvc` target using rustup. From the repository root on x64 Linux, use Python, `clang-cl`, `lld-link` and Windows SDK/CRT libraries installed via xwin. The default SDK root is `~/.cache/cargo-xwin/xwin`. Set `XWIN_SDK_ROOT` to use another installation. On native Windows, run the same Python build in an **x64 Native Tools Command Prompt** with a recent MSVC toolset and Windows SDK.

```sh
python build.py xmem
```

`cargo build --release -p tuxbox-xmem` builds the Rust archive. The Python build links it into the PE DLL and builds the independent C guest. The earlier `python runtime/xmem/build.py` command runs the same target. The executable and app-local DLL are in `build/xmem-smoke`. The smoke test checks the actual provider path, keeps both allocations live, writes and verifies all 65,548 bytes, checks alignment and checks that the released small block fails heap validation and observes the virtual reservation returning to the free state. [Heap validation](https://learn.microsoft.com/en-us/windows/win32/api/heapapi/nf-heapapi-heapvalidate) The guest also checks zero-size and unsupported-attribute results.

On Windows, from the build directory:

```bat
xmem-smoke.exe --report xmem-result.txt
```

On x86-64 Linux, from the build directory, select an installed Proton distribution and an isolated writable prefix:

```sh
WINEPREFIX="$HOME/.local/share/project-tuxbox/xmem-prefix" \
PROTONPATH="/path/to/proton-directory" umu-run "$PWD/xmem-smoke.exe" --report xmem-result.txt
```

A passing guest exits zero and writes `xmem-default: PASS (allocator pairs 1/6 and 2/7, ordinary and 64 KiB allocations, host working set v2, virtual reserve/commit)` to both stdout and the optional result file. To audit the binary boundary, run `llvm-readobj --coff-imports xmem-smoke.exe` and `llvm-readobj --coff-exports xmem.dll` in the same directory. Record native Windows and individual Proton providers separately. [UMU launch variables](https://github.com/Open-Wine-Components/umu-launcher#usage)
