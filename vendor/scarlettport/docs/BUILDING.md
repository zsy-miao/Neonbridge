# Building

Windows 10 22H2 or Windows 11 x64 only. Neither runtime builds on macOS or Linux.

## Prerequisites

- Rust via [rustup](https://rustup.rs), MSVC target (`stable-x86_64-pc-windows-msvc`). `runtime/victory` pins its own toolchain in `rust-toolchain.toml`; rustup installs it on first use.
- Visual Studio Build Tools with the "Desktop development with C++" workload and a Windows 10/11 SDK.
- PowerShell 5.1 or later for `scripts/*.ps1`.
- A GPU and driver with Shader Model 6.6 and mesh shader support to run (not needed to build).

Commands below are run from the repository root.

## XCL

```powershell
cd runtime\xcl
cargo build --release -p xcl --locked
cargo test --release -p xcl --lib --locked -- --test-threads=4
```

GPU tests print `SKIPPED <test>: <reason>` and pass when the adapter cannot run them. Or use `.\scripts\setup.ps1`, which also fetches the Agility SDK. More notes: `runtime/xcl/AGENTS.md`.

## VIctory stack

```powershell
cd runtime\victory
cargo rustc --release -p victory --bin victory --features diagnostics --locked -- -C link-arg=/EXPORT:D3D12SDKVersion,DATA -C link-arg=/EXPORT:D3D12SDKPath,DATA
```

The two `/EXPORT` flags are required so Windows loads the Agility SDK placed next to the executable. `.\scripts\setup-victory.ps1` runs this command and does the Agility SDK step.

The source in `runtime/victory` is fxceless/victory-fork plus merged community work and our changes (`runtime/victory/SCARLETTPORT.md`). Run it with `.\scripts\play-victory.ps1 -Game <your dump>` (Series S profile by default); the environment it sets is described in [RUNTIMES.md](RUNTIMES.md).

Tests that do not need a GPU: `cargo test --release --locked --lib -p victory-core -p victory-formats -p victory-pe -p victory-loader -p victory-shader -p victory-pipeline`. Tests in `victory-gpu` and `victory-hle` need a Shader Model 6.6 device. With `VICTORY_TEST_NO_GPU=1` they run as on a machine without a card (device tests are skipped).

## Agility SDK

Windows 10 needs the D3D12 Agility SDK (1.614.1) next to the executable, in a `D3D12` folder containing `D3D12Core.dll` and `d3d12SDKLayers.dll`. `setup-victory.ps1` downloads it from NuGet (`Microsoft.Direct3D.D3D12`) into `runtime\victory\target\release\D3D12`. To do it by hand, download the package, unzip it, and copy both DLLs from `build\native\bin\x64`. Do not commit these DLLs.

## Launcher

The launcher embeds no runtime. It runs the engines from a `runtime` folder beside `ScarlettPort.exe` (`victory.exe` plus its `D3D12\` folder; optionally `xcl.exe`, `dxcompiler.dll` and `dxil.dll` for the Classic engine). It never unpacks, downloads or writes executables.

```powershell
cd launcher
cargo build --release --locked
cargo test --release --locked
```

To run a development build, either copy `runtime\victory\target\release\victory.exe` and its `D3D12` folder into `launcher\target\release\runtime\`, or set `SCARLETT_RUNTIME_DIR` to a folder holding them. `ScarlettPort.exe --screenshot=FILE.png` renders the window to a PNG and exits.

Releases are built by `.github/workflows/release.yml` on a `v*` tag: launcher and VIctory-based runtime in release mode, the Agility SDK from NuGet, a ZIP with `ScarlettPort.exe` and `runtime\`, and SHA-256 checksums. No binaries are committed.
## Where things live

| What | Where |
|---|---|
| Build output | `runtime\xcl\target`, `runtime\victory\target`, `launcher\target` |
| Launcher engines | `runtime\` beside `ScarlettPort.exe` |
| Launcher settings and default cache | `%LOCALAPPDATA%\ScarlettPort`, cache on the fixed drive with the most free space (`<drive>\ScarlettCache`) |
| Shader caches | `<cache>\victory` (World), `<cache>\dxil` (Classic); build progress in `<cache>\build-world.json` / `build-classic.json` |
| XCL title data, pipeline journal | `%XCL_DATA_ROOT%` or `%LOCALAPPDATA%\XCL`, then the package name; `cache\pipelines.bin` inside it |

Caches are safe to delete; they are rebuilt on the next run (slowly: first-run shader compiles stall the world).
