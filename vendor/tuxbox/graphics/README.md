# Desktop D3D12 GPU readback

This baseline exercises the host graphics stack used by the planned D3D12.x backend. It currently calls the desktop API directly. The fixture creates a 37 × 13 RGBA8 render target, clears it to one color, and clears an interior rectangle to another. It then transitions the texture to a copy source and copies it to a readback buffer. After a bounded fence wait, it compares all 481 pixels with the CPU reference using the pitch reported by the device. The narrow width forces padding between rows.

## Build

On x86-64 Linux, install Python 3, clang-cl and lld-link. Use a Windows SDK/CRT installation staged by xwin or cargo-xwin. The build script defaults to `~/.cache/cargo-xwin/xwin` and respects `XDG_CACHE_HOME` or an explicit `XWIN_SDK_ROOT`. On Windows, run this command in an x64 Visual Studio developer prompt with the Windows SDK installed.

```sh
python build.py graphics
```

## Run

On Windows, run the executable from the repository root. A relative report path resolves against the guest's working directory.

```bat
build\graphics\desktop-clear.exe --report build\graphics\windows.txt
```

On Linux, use the project launcher or run an installed Proton build directly. From the repository root in Bash, set `PROTON` to the full path of the selected Proton executable. This example creates an isolated prefix. Set `STEAM_COMPAT_CLIENT_INSTALL_PATH` if Steam is installed elsewhere.

```sh
cd build/graphics
export STEAM_COMPAT_CLIENT_INSTALL_PATH="${STEAM_COMPAT_CLIENT_INSTALL_PATH:-$HOME/.steam/root}"
export STEAM_COMPAT_DATA_PATH="$PWD/proton-prefix"
mkdir -p "$STEAM_COMPAT_DATA_PATH"
"$PROTON" run "$PWD/desktop-clear.exe" --report result.txt && cat result.txt
```

By default, the probe selects the first hardware adapter for which it can create a device. Use `--adapter VVVV:DDDD` to require explicit hexadecimal PCI vendor and device IDs. A passed report includes the DXGI adapter description, the IDs, the pixel count, the unpadded row size and the actual copy pitch. A valid run that fails leaves its requested report empty and exits with a failure code. Adapter selection skips interfaces marked as software by DXGI. Treat this as a correctness fixture. Timing its complete process includes runtime startup, device creation and synchronization.

The relevant desktop contracts are documented by Microsoft. [Copyable footprints](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nf-d3d12-id3d12device-getcopyablefootprints) · [Texture copy](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nf-d3d12-id3d12graphicscommandlist-copytextureregion) · [Fence completion](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nf-d3d12-id3d12fence-seteventoncompletion)
