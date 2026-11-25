> Français : [jour-j.md](fr/jour-j.md)

# First launch of a console executable

Steps to follow once the package executable (for example `game_scarlett_master.exe`) is available. Everything runs from the AnyXboxX root.

## 1. Prepare (once)

```powershell
git submodule update --init
cmake -S runtime -B runtime/build -A x64
cmake --build runtime/build --config Release
cargo build --release --manifest-path rust/Cargo.toml
python tools/build_tombox.py d3d12_xs xg_xs xmem
```

## 2. Safe diagnosis

```powershell
python tools/jour_j.py ..
```

The tool:

- inventories the executable: SHA-256 hash, headers, sections, TLS and imports;
- tells, for each imported function, who provides it: the package, AnyXboxX, TomBox, Windows or nobody;
- runs `--rapport` and `--lancer`, which execute the program in Unicorn, isolated from the PC.

The report is written to `%LOCALAPPDATA%\AnyXboxX\jour-j\<date>\rapport.md`, outside the repository: it names functions of the game and must not be versioned.

## 3. Real launch (optional)

```powershell
python tools/jour_j.py .. --natif --duree 120
```

`anyxboxx.exe` then runs the game code directly on the processor, with TomBox's console DLLs (`ANYXBOXX_TOMBOX_DIR`) and the call trace (`ANYXBOXX_TRACE=1`).

The native report lists pipeline state objects (PSOs) from TomBox's sampled creation and rejection events, with the last graphics PSO counter snapshot when available. Each diagnostic launch sets `TOMBOX_LOG` to a fresh file under the report directory. The report identifies that local log when TomBox writes it.

**This mode does not isolate the game.** Use it only for an executable of known origin.

### What the Windows host must provide

Native mode binds some imports to Windows itself, not to AnyXboxX. When one is missing, `anyxboxx.exe` stops before the entry point and names it (`dll!function` or `dll!#ordinal`).

- **GameInput** (`GameInput.dll!#1`). The title imports `GameInputCreate` by ordinal 1, as the GameInput library of the GDK links it. AnyXboxX does not serve it on purpose: the host's GameInput runtime gives the real controllers, and every GameInput runtime supports the v0 interface that this `GameInputCreate` creates ([GameInputCreate](https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/input/gameinput/functions/gameinputcreate), [API versions](https://learn.microsoft.com/en-us/gaming/gdk/docs/features/common/input/overviews/input-version-interop)). Recent Windows versions include it (`System32\GameInput.dll`); if it is missing, install the redistributable with `winget install Microsoft.GameInput` ([GameInput NuGet](https://learn.microsoft.com/en-us/gaming/gdk/docs/features/common/input/overviews/input-nuget)).
- **Windows API sets and libraries with no console counterpart in AnyXboxX**: `api-ms-win-core-psm-appnotify-*` (`RegisterAppStateChangeNotification` and `RegisterAppConstrainedChangeNotification`, called right after the GDK start-up), `ext-ms-win-rtcore-ntuser-*`, Media Foundation (`MFPlat.dll`) and Winsock (`WS2_32.dll`, imported by ordinal). The 1.0.2.1 executable was only measured on Windows 11, where all of them resolve. Windows decides per device which API set contracts exist, whatever their `api-` or `ext-` prefix ([Windows API sets](https://learn.microsoft.com/en-us/windows/win32/apiindex/windows-apisets)), and the N editions of Windows ship without Media Foundation until the [Media Feature Pack](https://support.microsoft.com/windows/media-feature-pack-for-windows-n-8622b390-4ce6-43c9-9b42-549e5328e407) is installed. Use a recent Windows 11 that is not an N edition, or install that pack.

## 4. After the diagnosis

- The **missing** functions in the report become issues, one per DLL.
- The game's **shaders** go through `--rdna2 … --liste` then `--hlsl`; the cache (`emu/shader_cache.py`) avoids recompiling them.
- Nothing from the game enters the repository: not the executable, not its shaders, not its data. Only function names and findings may be published.
