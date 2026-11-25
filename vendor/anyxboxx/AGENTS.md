# AGENTS.md — AnyXboxX

Guide for AI coding agents and contributors working in this repository. Human contribution rules (fork, pull request, review) are in [CONTRIBUTING.md](CONTRIBUTING.md). Issues, pull requests and commit messages are written in English.

## What the project is

AnyXboxX is a GPL-3.0-or-later prototype that runs Xbox Series X (GDK, "Scarlett") executables on Windows by high-level emulation: console system calls are served by a C++ runtime (native mode, Wine-style) or by Python handlers inside a Unicorn sandbox (isolated mode). A second track translates console RDNA 2 GPU shaders to HLSL/DXIL. It does not run commercial games.

## Layout

| Path | Content |
| --- | --- |
| `PrototypeScarlett/scarlett.py` | Command line entry point (`--rapport`, `--banc`, `--natif`, `--lancer`, `--rdna2`, `--carte`, `--traduction`…) |
| `PrototypeScarlett/emu/` | Python: PE loader, Unicorn machine, HLE handlers (`hle/`), native mode (`native/`), reports |
| `PrototypeScarlett/emu/rdna2*.py` | RDNA 2: decoder, operands, reference interpreter (`rdna2_exec*`), HLSL recompiler (`rdna2_hlsl*`), GPU runner glue (`rdna2_gpu.py`) |
| `PrototypeScarlett/emu/rdna2_ops.py`, `rdna2_shapes.py` | **Generated** from LLVM's public encoding tests by `tools/rdna2_mnemonics.py` and `tools/rdna2_shapes.py`. Never edit by hand. |
| `runtime/` | C++ runtime DLLs (`xgameruntime`, `xmem`, `d3d12_xs`, `kernelx`, `anyxboxx_tls`), native launcher `anyxboxx.exe`, GPU runner `anyxboxx_gpu_run.exe`. `runtime/third_party/libhttpclient-task/` is Microsoft's MIT `Task` module, kept as is. |
| `rust/` | Rust workspace. `anyxboxx-rdna2` bridges to TomBox's RDNA 2 decoder; `anyxboxx-texture` decodes BC7 and GFX10 `SW_4K_S` tiles (ported from Xbox2Windows); `anyxboxx-audio` decodes XMA2 with TomBox's FFmpeg port. |
| `third_party/tombox/` | Git submodule: [omonkeyz/xbox-emulator](https://github.com/omonkeyz/xbox-emulator) (TomBox continuation, GPL-3.0-or-later since its commit `51f5319`, MIT OR Apache-2.0 before), pinned commit. Do not edit; propose changes upstream. |
| `third_party/victory/` | Copy of [VIctory](https://github.com/Marvisak/VIctory) (GPL-3.0-or-later) on the Neptune (VIctory Edit.) 2026-11-25c base, as assembled for GTA VI (see [THIRD_PARTY.md](THIRD_PARTY.md)), built and launched by `tools/lancer_victory.ps1`. Its own Cargo workspace; its `docs/asset-policy.md` applies too. Our own code now, with no upstream: nothing is synced or pinned, and it builds even if every repository it came from is gone. See rule 10 for taking changes from elsewhere. |
| `third_party/leonida/` | Unchanged frozen copies of Leonida's translators, called by `PrototypeScarlett`. The Leonida project was deleted on 2026-11-24 and its submodule removed; nothing tracks it any more. |
| `third_party/xbox2windows/` | Unchanged copies of [R0Y00/Xbox2Windows](https://github.com/R0Y00/Xbox2Windows) tools (MIT, DXIL retarget), commit in its README; refreshed by `tools/sync_voisins.py`. |
| `tools/` | Generators (GDK interface table, kernelx exports, RDNA 2 tables), comparison and measurement tools (TomBox, FSSM, Xbox compiler probe, GPU check), `jour_j.py`, the VIctory launcher `lancer_victory.ps1`, neighbour watch (`sync_voisins.py`, `veille.py`) |
| `site/` | GitHub Pages site, published as is by `.github/workflows/pages.yml` |
| `tests/` | pytest suite (the README table holds the current test count) |
| `docs/` | Research notes and logs, in English. `docs/fr/` holds the French version of each document (same tree). `docs/i18n/` holds the 8 README translations. |

## Setup and commands

Windows x64, Python 3.12, Visual Studio 2022/2026 (C++ workload, CMake), Windows SDK 10.0.26100 (provides `dxc`), Rust stable (MSVC).

```powershell
git submodule update --init
py -3.12 -m venv .venv
.\.venv\Scripts\python.exe -m pip install -r requirements.txt -r requirements-dev.txt
.\.venv\Scripts\python.exe tools\build_tombox.py   # DLL console de TomBox dans runtime\build\tombox (lues par défaut)
cmake -S runtime -B runtime/build -A x64
cmake --build runtime/build --config Release
cargo build --release --manifest-path rust/Cargo.toml
.\.venv\Scripts\python.exe -m pytest
```

- Tests needing the C++ runtime are skipped when it is not built, except in CI (`ANYXBOXX_REQUIRE_RUNTIME=1`).
- GPU tests (`tests/test_rdna2_hlsl.py`) need `dxc` and `anyxboxx_gpu_run.exe`; without a GPU they use WARP (`ANYXBOXX_GPU=warp` forces it).
- Lint used by the maintainers: `python -m ruff check --select W605,F,E9 PrototypeScarlett tests tools` (`third_party/` is not linted).
- VIctory's asset policy on its copy: `cargo xtask policy`, run from `third_party/victory` (CI job `politique VIctory`).

## Rules

1. **No protected content in the repository.** Never add game files, console or SDK files, keys, dumps, extracted assets or shaders, or commercial executables (executables, DLLs, `.rpf` archives, textures, sounds, saves), not even partial ones or in tests. Byte-pattern signatures are allowed to locate functions or data in the game executable: short patterns written by us, with masks, plus the per-build offsets of the functions we hook (under names we choose) and the build identity (hash of the executable, CodeView GUID/age). The game's own code (sections, routines, decompiled or disassembled code beyond those signatures) stays forbidden. The only possible exception is a proprietary constant table without which a format cannot be decoded and that cannot be recomputed (like VIctory's BCPack tables): it is added only by the maintainer's decision, named in the exceptions register of [THIRD_PARTY.md](THIRD_PARTY.md), and the repository licence does not cover it. Everything else (assets, extracted shaders, textures, sounds, archives, saves, dumps, keys, executables) stays forbidden. Model: VIctory's `docs/asset-policy.md`. Running and analysing your own copy of the executable and of the game build is allowed and encouraged: launch it (native mode, VIctory through `tools/lancer_victory.ps1`, AnyXboxX), measure it, debug it, analyse it statically and dynamically, and use what you learn to steer your pull requests. Build test inputs from source (`tests/pe_builder.py`, the encoders in `tests/test_rdna2_exec*.py`).
2. **Sources.** Implementation follows public sources: AMD's RDNA 2 ISA documentation (where to read it, and how to label RDNA 3, LLVM or Mesa evidence: [docs/research/rdna2-sources.md](docs/research/rdna2-sources.md)), Microsoft Learn (GDK, Win32, D3D12), LLVM's tests, and open-source code under a compatible licence, credited in [THIRD_PARTY.md](THIRD_PARTY.md), plus what you measure by running your own copy of the game. The limit: no game code, assets or extracted files in the repository; the only game-derived data allowed are the byte-pattern signatures, per-build offsets and build identity of rule 1, and the registered exceptions. Measurements and findings drawn from running the game (summarised logs, counts, technical identifiers such as shader hashes or pass names, test results) may be cited in pull requests and in the docs; say how each was obtained (measured, read statically, assumed).
3. **No fake success.** An unimplemented function returns `E_NOTIMPL` (or the console's error) and is logged; an untranslated instruction raises an error that names it.
4. **Verify against an oracle.** Decoder changes are checked against the LLVM tables (`python tools/rdna2_mnemonics.py <llvm .s folder> --verifier`); every HLSL translation is checked against the reference interpreter, byte for byte, on GPU or WARP.
5. **Tests with every change.** New behaviour comes with a test that fails without it.
6. **README in 9 languages.** `README.md` (English) and `docs/i18n/README.*.md` must stay in sync: same table rows, same test count (rounded down to the hundred, written `800+`, checked by `tests/test_readme_sync.py`), same images. Image text is in English.
7. **Git**: `main` is protected; work on a branch and open a pull request; CI (`.github/workflows/tests.yml`) must pass. Commit messages: `type: description` (`feat`, `fix`, `docs`, `test`, `refactor`, `chore`), written in English (older history is in French).
8. **Code style**: small focused files (under about 400 lines), small functions, explicit error handling, no hidden mutation of shared state, constants instead of magic numbers where it helps reading. `third_party/` is exempt (VIctory alone is about 123,000 lines; the other folders are copies or submodules of other projects): it keeps its own style, and neither the 400-line rule nor ruff applies to it.
9. **Native mode runs code unsandboxed.** Only run trusted test programs with `--natif` or `anyxboxx.exe`.
10. **Code taken from other repositories is committed as ours.** Nothing in the repository depends on another repository to build: no new submodules, no pinned forks. To take a change (a commit, a pull request, a fork's branch), commit it yourself (`git cherry-pick -Xsubtree=third_party/<project> <sha>` after fetching the repository, or a port by hand) and end the message with `Source: <repository> <PR or commit>` and `Co-authored-by: Name <email>` for its author. Add or update that repository's row in the sources table of [THIRD_PARTY.md](THIRD_PARTY.md). Its licence must be compatible (GPL-3.0 or more permissive).

## Where to start

Own a copy of the executable and of the build? Start with `python tools/jour_j.py <package folder>` ([docs/day-one.md](docs/day-one.md)): its report stays outside the repository. To launch the game under VIctory, use `tools/lancer_victory.ps1`; the native launcher and `scarlett.py --lancer` are the other entry points.

Open issues list what remains, each with a "done when" checklist. Most console-library and Direct3D 12.x work is blocked on a legally usable Scarlett executable; RDNA 2 shader work (`emu/rdna2*`) and GDK runtime functions (`runtime/xgameruntime`) can progress from public documentation.
