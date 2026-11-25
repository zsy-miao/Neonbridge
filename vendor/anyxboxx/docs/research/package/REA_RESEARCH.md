> Français : [REA_RECHERCHE.md](../../fr/recherche/paquet/REA_RECHERCHE.md)

# Inspection with REA

REA 6.10.1 was installed temporarily from the official npm package and run with Node.js 24.22.1. The inspection is static and starts no game executable. The source repository consulted was at commit `a125d37bfad92fc3b5e472c6c18ee71654037221`.

## Launcher

The `inspect-managed-members` command identifies a .NET assembly named `Lancer GTA VI`, version `0.0.0.0`, with CLR metadata `v4.0.30319`. Its `Launcher` class declares three methods. REA reports full coverage of the examined metadata.

| Method | Body offset in the file | IL size | Decoded instructions |
| --- | ---: | ---: | ---: |
| CanLaunch | 592 | 76 | 30 |
| RunGame | 680 | 79 | 39 |
| Main | 788 | 296 | 126 |

SHA-256 of the analyzed launcher: `c0bdbbf2076e03f72343d5cc4f73c1abd227dd8abea4aab3e190ad2b8d3ef191`. The REA identifier of this analysis is `ev_3d5448eb94cb8d5630703a2b65f9c58a84e3b26a10ea68ca596647abfed1f8c5`. The coordinates and fingerprints only hold for this exact file. The behavior of the methods is explained in `GAME_ANALYSIS.md`, based on a reading of their IL code.

## Installation folder

The `inspect-desktop-application` command examined 3,941 entries and probed 49 files, with 16,044 bytes read. It reports .NET candidate files, including the launcher and the Scarlett prototype. It also identifies the PE headers of libraries. The results come from a partial, successive read of the folder. They do not prove that the files were run together. REA did not interpret the content of the RPF files or the mission logic.

## Reproduction

An installation of REA 6.10.1 and Node.js 24.22.1 meets the versions used here. From the folder containing the files, run:

```powershell
rea inspect-desktop-application . --detail complete --json
rea inspect-managed-members ".\Lancer GTA VI.exe" --json
```

The full output contains the coordinates of the methods, instructions and references. The folder walk includes the subfolders present at the time of the run, so the coverage figures may change.

## Inspection of `Catalog000.bin`

An additional static inspection with REA 6.10.1 confirms a PE32+ x64 file of 815,104 bytes, SHA-256 `4afbdbe22aad2bc338a5ba1ffe83cccb96e072eb4e92f6a5dd90b49b2e227c59`. The following four analyses report full coverage of the relevant PE directories:

| REA command | Observed result |
| --- | --- |
| `inspect-pe-imports` | No import or delay-import directory. |
| `inspect-native-exports` | No export directory. |
| `inspect-pe-resources` | No PE resource directory. |
| `inspect-pe-certificates` | No PE certificate directory. This does not check for a possible external signature. |

An additional read with `pefile` finds a null entry point and a single section with an empty name, marked readable (`0x40000000`). The raw data of this section takes 811,008 bytes starting at `0x1000`. Its empirical Shannon entropy is about 7.992 bits per byte. This measure cannot tell compression, encryption or another encoding apart. These observations do not reveal the function of the content, and the absence of the usual directories says nothing about possible structures specific to the format.

To reproduce the REA analyses:

```powershell
rea inspect-pe-imports .\Catalog000.bin --json
rea inspect-native-exports .\Catalog000.bin --json
rea inspect-pe-resources .\Catalog000.bin --json
rea inspect-pe-certificates .\Catalog000.bin --json
```

## FidelityFX libraries

REA listed the import and export tables of the two DLLs, without loading them. Their SHA-256 are `431bdd2e26c0c8214fd7601140eb9fe6d8a249f5000539453a9a9b6a0b7c0ee8` for `amd_fidelityfx_loader_xbox.dll` and `faff02f294a7e9149ce079dffe3dedbcabe99b1f180ab14222aa8689e00a75fa` for `amd_fidelityfx_upscaler_xbox.dll`, respectively. Both export the same five names: `ffxConfigure`, `ffxCreateContext`, `ffxDestroyContext`, `ffxDispatch` and `ffxQuery`. REA reports 22 imports spread over nine DLLs for the loader and 86 imports in 24 DLLs for the upscaler. These numbers describe the tables, not the calls actually executed.

The loader imports `LoadLibraryA` and `GetProcAddress`, among others. Its bytes contain the names `amd_fidelityfx_upscaler_xbox.dll` at file offset `0x19f0` and `amd_fidelityfx_framegeneration_xbox.dll` at `0x1a18`. The first file is present at the root of the package. The second was not found in the walk of the folder files. These clues are compatible with dynamic resolution, but prove neither the loading path nor that a missing module is required.

The local layout declares the loader and the upscaler, but does not mention the frame generation module. This absence from the layout does not suffice to determine whether it is needed in another configuration.

The upscaler contains the string `FFX_FSR4_XBOX_ACO` at file offset `0x47088` and the name `amd_fidelityfx_upscaler_xbox_fsr4_full_720p1080p_input_sanitize_scarlett_opt2.dll` at `0xb9dea`. This last file was not found in the folder. There are also strings naming FSR4 models. These strings indicate FSR4-related content in the library. They do not establish that the game enables it or that it loads this file. The version resources declare versions `1.0.2.0` for the loader and `3.1.5.0` for the upscaler. These are declared metadata, not an authentication of their origin.

Reproduction with REA:

```powershell
rea inspect-pe-imports .\amd_fidelityfx_loader_xbox.dll --json
rea inspect-native-exports .\amd_fidelityfx_loader_xbox.dll --json
rea inspect-pe-imports .\amd_fidelityfx_upscaler_xbox.dll --json
rea inspect-native-exports .\amd_fidelityfx_upscaler_xbox.dll --json
```

The native decompilation attempt of REA returned `provider_unavailable`: no PE-compatible provider was configured, notably Ghidra. The precise relations between strings, functions and calls therefore remain to be established.
