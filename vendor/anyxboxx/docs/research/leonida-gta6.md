> Français : [leonida-gta6.md](../fr/recherche/leonida-gta6.md)

# Leonida and the Xbox Series 1.0.2.1 build

State on 23 November 2026, upstream commit `561b65a832`. Leonida ([ItsNotRexed/Leonida](https://github.com/ItsNotRexed/Leonida), GPL-3.0) statically recompiles the Xbox Series 1.0.2.1 build for Windows: the game's x86-64 code runs natively, the console system software is reimplemented, and the RDNA 2 shaders are translated to SPIR-V for Vulkan. The repository contains no game, no key and no executable.

Since 23 November, AnyXboxX first aims to run this build: we contribute to Leonida and reuse their work, while keeping AnyXboxX a generic project.

## What we take

- **The whole repository**: the upstream repository was deleted on 2026-11-24 and its submodule was removed; only the frozen copies in `third_party/leonida/` remain.
- **The copies of the translator** RDNA 2 → SPIR-V in `third_party/leonida/`: `PrototypeScarlett/emu/rdna2_spirv.py` calls them. They do not change.

## Where Leonida stands (from their README)

| Topic | Their state |
| --- | --- |
| Game startup | no: they also lack the executable |
| RDNA 2 shaders → SPIR-V | 26,503 of 26,503 translated, all valid according to `spirv-val` |
| Resources | models (OBJ), textures (DDS, BCPack partial), audio (AWC) exported from the player's copy |
| PE loader and shims | a stand-in executable runs end to end (`gta6rt --run`) |
| Direct3D 12 Xbox → Vulkan | in progress (device, queues, resources); ray tracing in progress |

Their critical path and the D3D12 methods still open are tracked in their issue #313. To work on a topic, reserve it in the open issue "Coordinating who works on what". A reservation expires after 12 h without a PR.

## GDK functions: comparison of the two runtimes

The comparison is done by name search, without compiling, so the numbers are approximate. AnyXboxX declares 396 methods (`runtime/xgameruntime/gdk_api_table.inc`) and implements about 170. Leonida names about 310 functions in `runtime/shims/xgameruntime*.cpp` and implements about 190. No import of the executable was measured: their `docs/runtime_imports.md` only lists the families that the package's DLLs link (XAsync, XTaskQueue, XUser, XStore, XGameSave, XPackage, XGameUi).

**Implemented by us, missing or stubs on their side** (contribution leads):

- `XPackage*`: `EnumeratePackages`, `GetPackageKind`, `EstimateDownloadSize`, `CreateInstallationMonitor`, `InstallChunksAsync`, `MountWithUiAsync`, `GetWriteStats`, the chunk functions (`runtime/xgameruntime/package_interface.cpp`).
- `XUserGetGamerPictureAsync` and its results (`user_interface.cpp`): stub `offline_failure<16, E_FAIL>` on their side.
- `XStoreShow*UIAsync`, `XStoreDownloadAndInstallPackagesAsync`, `XStoreAcquireLicenseForDurablesAsync`: offline store network error on our side, stub on theirs.
- `XGameRuntimeIsFeatureAvailable`, `XDisplayAcquireTimeoutDeferral` (`system_interface.cpp`).

**Implemented by them, missing on our side** (to take over):

- `XGameStreaming*`, `XGameUiShowMessageDialogAsync` and `XGameUiShowErrorDialogAsync`.
- `XStoreIsLicenseValid`, `XStoreCanAcquireLicenseFor*Async`, `XStoreQueryAddOnLicensesAsync`.
- `XUserFindForDevice`, `XUserRegisterForDeviceAssociationChanged`, `XNetworkingQueryConfigurationSetting`, `XNetworkingVerifyServerCertificate`.

Before each contribution, check on their current `main` that the function is still missing, then reserve it.

## Method

1. A local integration branch (`outils/leonida`) = their `main` + our pending PRs. We run this branch, without waiting for their merges.
2. Small PRs, one per reserved topic, rebased right after each merge: their hot files (`d3d12_vulkan_device.cpp`, `check.py`, `run_gpu_tests.py`) change in more than 100 open PRs.
3. No game file in either repository.
