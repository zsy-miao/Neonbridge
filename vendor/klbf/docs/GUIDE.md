# KLBF Lab: beginner's guide (Windows)

Every step below is a command to copy and paste, in order. You only need your own copy of the game's Xbox Series files in a folder. KLBF Lab includes no game files.

In this guide the game files are in **`C:\Games\GTA6`** (the folder that holds `game_scarlett_master.exe` and `shaders.rpf`). If yours are elsewhere, replace that path everywhere.

Open **PowerShell** (Start menu → type `PowerShell` → Enter) and paste each block. Wait for one block to finish before the next.

---

## 1. Check your PC (1 minute)

- A recent graphics card with DirectX 12 (tested on AMD Radeon RX 9070 XT; other AMD and NVIDIA cards are being tested).
- **More than 8 GB of video memory** recommended: 8 GB cards (RX 6600, RTX 3070…) currently crash at start.
- **32 GB of RAM**, or 16 GB plus a large page file (step 2). A run uses about 20 GB of memory.
- About 15 GB of free disk space for the tools and the shader cache.

## 2. Page file (only if you have 16 GB of RAM)

Windows needs extra virtual memory. This opens the right settings window:

```powershell
SystemPropertiesPerformance.exe
```

Then: **Advanced** tab → **Virtual memory: Change…** → tick **Automatically manage paging file size for all drives** (or untick it, pick your fastest drive, **Custom size**, Initial `32768`, Maximum `49152`) → **Set** → **OK** → restart the PC.

## 3. Install the tools (10–20 minutes, once)

Installs Git, Rust and the Microsoft C++ build tools, all from their official sources through `winget` (built into Windows 10/11):

```powershell
winget install --id Git.Git -e --source winget
winget install --id Rustlang.Rustup -e --source winget
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --source winget --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

**Close PowerShell and open a new one** so the new tools are found.

## 4. Download and build KLBF Lab (5–15 minutes)

```powershell
cd $HOME
git clone https://github.com/PastiiJay/klbf-lab
cd klbf-lab
cargo build --release
```

For the latest test build instead, use `git clone -b beta https://github.com/PastiiJay/klbf-lab`.

## 5. Get AMD's FidelityFX SDK (for the faster Series S mode)

Download **FidelityFX SDK v2.4.0** from AMD's official page: https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/releases. Unzip it to **`C:\FidelityFX-SDK`**. Check that this file exists: `C:\FidelityFX-SDK\Kits\FidelityFX\signedbin\amd_fidelityfx_loader_dx12.dll`.

## 6. Settings (paste at the start of every PowerShell session)

```powershell
cd $HOME\klbf-lab
$env:VICTORY_SHADER_OPT = '0'
$env:VICTORY_SKIP_PIPELINES = (Get-Content docs\klbf\skip-amd.txt -Raw).Trim()
$env:VICTORY_LATE_WAIT = '1'
$env:VICTORY_WIDE_BACK = '1'
$env:VICTORY_PIPELINE_WAIT_MS = '24'
$env:VICTORY_RELAY_COPY_QUEUE = '0'
$env:VICTORY_FFX_DIR = 'C:\FidelityFX-SDK\Kits\FidelityFX\signedbin'
```

If your graphics card stops responding about a minute after the game world starts loading, add `$env:VICTORY_LATE_WAIT = '0'`.

## 7. Pre-build the shaders (10–20 minutes, once)

This compiles all of the game's shaders for your graphics card ahead of time, so the game doesn't stutter or crash on them later:

```powershell
target\release\victory.exe translate --cache --pipelines --jobs 8 C:\Games\GTA6\shaders.rpf
```

If it crashes (the graphics driver sometimes does), **run the same line again**: it resumes where it stopped and remembers what crashed. It's done when it ends without a crash. Do it again after each KLBF Lab update.

## 8. Play

```powershell
target\release\victory.exe run --console series-s C:\Games\GTA6\game_scarlett_master.exe --execute
```

- **Plug in the controller before starting.**
- On the stable build the start screen and menus can stay **black**: press **A** (or Enter) to get past them.
- The first minutes in a new area are slow while shaders finish loading.
- Saves work (autosave and manual).

## Updating later

```powershell
cd $HOME\klbf-lab
git pull
cargo build --release
```

Then step 6, step 7 again, and step 8.

## Something went wrong?

| What you see | What to do |
|---|---|
| `amd_fidelityfx_loader_dx12.dll not found`, then a crash | Step 5, and check the path in `VICTORY_FFX_DIR` |
| Crash about 3 seconds after start, black window | Not enough memory: step 2 (page file), close other programs |
| `X3506` or `vs_5_1` in the log, black window | Old Windows shader compiler: copy the x64 `d3dcompiler_47.dll` from a recent Windows SDK (`Redist\D3D\x64`) next to `victory.exe` |
| `error: unexpected argument '--cache'` | You built an old copy: redo step 4 |
| Pre-build says everything is already built but the game still crashes in `amdxc64.dll` | A cache from another VIctory-based build is in the way: delete `%LOCALAPPDATA%\victory\cache\pipelines`, then redo step 7 (thanks @Ethan-Hill) |
| Black screen with sound, log says `amd_fidelityfx_loader_dx12.dll not found` with `<FidelityFX SDK>` in the path | `VICTORY_FFX_DIR` still holds the example path: set it to the folder where you unzipped the SDK (step 5/6) (thanks @Ethan-Hill) |
| Graphics card stops responding after ~1 min | `$env:VICTORY_LATE_WAIT = '0'` |
| Crash while loading: the log shows `dstorage: request failed: zlib … hr=0x8007000d`, then `0x80000003` at `victory-image…+0x19b5024` | Some game files are incomplete or damaged: copy the game folder again from your source (thanks @Emaxoso) |

Still stuck? Open an [issue](https://github.com/PastiiJay/klbf-lab/issues) with your graphics card, driver version, Windows version and the last lines PowerShell printed.


## Linux / Proton (community notes)

KLBF Lab is tested on Windows. Community players run it under Proton on AMD cards (RX 7800 XT, RX 6900 XT); it reaches the game world but is slower. Two tips from them:

- **Seconds per frame while the GPU is idle and `wineserver` uses a whole core**: the slowdown is in Wine, not in KLBF Lab. A `wineserver` that keeps the views in a tree fixes it: [CachyOS/wine-cachyos#50](https://github.com/CachyOS/wine-cachyos/pull/50). (Found and written up by @zocram4cc, measured by @Adiker in [#19](https://github.com/PastiiJay/klbf-lab/issues/19).)
- **`X3506` / `vs_5_1` in the log**: put the x64 `d3dcompiler_47.dll` from a recent Windows SDK (`Redist\D3D\x64`) next to `victory.exe` and add `d3dcompiler_47=n,b` to Wine's DLL overrides (from @gmankab).

Report Linux results in an [issue](https://github.com/PastiiJay/klbf-lab/issues) with your distro, Proton version, GPU and driver.

---

Thanks to [@HishamMuhammed](https://github.com/HishamMuhammed), whose questions shaped this guide.
