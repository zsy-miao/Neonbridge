> Français : [etat-de-l-art.md](../fr/recherche/etat-de-l-art.md)

# Xbox emulators and compatibility layers

Research done on 21 November 2026 on the web and GitHub. **Verified**: page read. **To confirm**: secondary source or snippet.

## Findings

No serious public emulator runs Xbox Series X|S games. The credible projects are high-level **compatibility layers** (HLE), modelled on Wine: they replace the console libraries with PC equivalents instead of simulating the hardware. Videos and sites that promise a ready-to-use "Xbox Series X emulator" are scams.

## Projects

| Project | Target | Approach | Status |
|---|---|---|---|
| [WinDurango](https://github.com/WinDurango/WinDurango) | Xbox One (Durango) | translation layer to Windows: Xbox WinRT, KernelX, D3D11.X; LGPL-3.0 | Sonic Mania, Minecraft 1.2, LIMBO playable; Forza Horizon 2 starts (verified) |
| XWine1 | Xbox One | translation layer, unpublished | announced in September 2024 ([Digital Trends](https://www.digitaltrends.com/gaming/xbox-one-emulation-xwine1/), to confirm) |
| [Xodus](https://tomshardware.com/software/linux/xbox-pc-and-game-pass-titles-are-coming-to-linux-through-xodus-heroic-launcher-devs-embark-on-new-open-source-reverse-engineering-project) | GDK PC games on Linux | GDK / MSIXVC reverse engineering | project just started (to confirm) |
| [Xenia](https://github.com/xenia-project/xenia) | Xbox 360 | PowerPC recompilation, GPU to D3D12/Vulkan | partial compatibility (verified) |
| [xemu](https://github.com/xemu-project/xemu) | Original Xbox | low-level emulation | reference for the OG Xbox (to confirm) |
| [huan](https://github.com/huan-project/huan) | Xbox One | calls itself a cross-platform emulator | unverified, be careful |

## Useful tools

- [xvdtool](https://github.com/emoose/xvdtool) and [XvdTool.Streaming](https://github.com/LukeFZ/XvdTool.Streaming): read XVD/XVC containers. Decryption needs keys that are not in these repositories.
- [Qiling](https://github.com/qilingframework/qiling): binary emulation on Unicorn, with a PE loader and Windows APIs (GPL-2.0).
- [Speakeasy](https://github.com/mandiant/speakeasy): Windows API emulation for malware analysis (MIT). Direct model for the AnyXboxX HLE page.
- [GDK documentation](https://learn.microsoft.com/gaming/gdk/): public signatures of `XSystemGetDeviceType`, `XGameRuntimeInitialize`, `XTaskQueue*`, etc.
- [Xbox One Research wiki](https://xboxoneresearch.github.io/wiki/): XVD formats, file system, security.

## What AnyXboxX took from them

| Idea | Source | Implementation |
|---|---|---|
| Replace APIs with host code (HLE) | WinDurango, Speakeasy | `emu/hle/`: decorator-based registry, thunk page |
| Group the DLLs that expose the same functions | WinDurango (KernelX) | `family()`: kernel32, kernelx, api-ms-win-core-* |
| Do not stall on an unknown function | Speakeasy | returns `E_NOTIMPL` and writes it to the log |
| Relocations, TLS, TEB/PEB through GS | Qiling, PE loaders | `emu/loader.py`, `emu/machine.py` |
| Compatibility report | Xenia compatibility logs | `scarlett.py --rapport` |

## Update of 21 November 2026

- [omonkeyz/xbox-emulator](https://github.com/omonkeyz/xbox-emulator), continuation of [TomBox](https://github.com/drycheetah84/TomBox) (Rust, GPL-3.0 or later since 22 November 2026, MIT or Apache-2.0 before): replacement console DLLs, console Direct3D 12.x, RDNA 2 shader translator. Fortnite reaches the Battle Royale lobby there. Details and reuse in AnyXboxX: [tombox.md](tombox.md).
- [xodus-gaming/xgameruntime](https://github.com/xodus-gaming/xgameruntime) (C++, LGPL-2.1): free reimplementation of `xgameruntime.dll` for Wine, coming from WineGDK (archived on 13 November 2026).
- WinDurango (Xbox One, LGPL-3.0) is still active; XWine1 still has no public repository.
