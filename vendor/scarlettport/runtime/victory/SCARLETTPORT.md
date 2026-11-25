# Provenance

This tree is the VIctory-based runtime used by ScarlettPort, vendored as plain source (no build output, no history). It is licensed GPL-3.0-or-later; `LICENSE` is unchanged.

## Lineage

1. **VIctory** by Marvisak and contributors (https://github.com/Marvisak/VIctory), GPL-3.0-or-later. The loader, the platform services, the Direct3D 12 back end and the shader recompiler start here.
2. **fxceless/victory-fork** (https://github.com/fxceless/victory-fork), GPL-3.0. Our base is its `main` at `4e17478`.
3. **VIcky** by Xeltax (https://github.com/Xeltax/VIcky), branch `fx-heapmap-4e`: the heap map for NVIDIA's view-heap cap and the per-program wave64 switch, plus commits by its contributors (including an integer-blend fix and a window-close fix). See `VICKY.md`.
4. **AnyXboxX** (LooperSalty, https://github.com/LooperSalty/victory): the stable stack and notes that our earlier base and this work drew on; its `lot5` branch was checked for overlap and licence (GPL-3.0).
5. **victory-amd** by myokex (https://github.com/myokex/victory-amd, a fork of fxceless/victory-fork, GPL-3.0), merged at `8e6d29e`: AMD fixes (texture views told apart by the memory their mips take, views of placed textures put back, dispatches over 65,535 groups split, DirectStorage page arrays read when run, compute pipelines waiting separately, view indices clamped on AMD, programs that stop the AMD compiler left out). Its README is kept as `README.md`. Commits by its other contributors (uiliannn, sagouindupm, lukka, DoubleD17, AscilCH) are in the range below.
6. **Community pull requests** picked on top, keeping their authors where the commit was applied as is. Numbers are those of the repository they came from (upstream VIctory for #1215; otherwise fxceless/victory-fork):
   - #1215 (upstream): direct writes into title memory, `VirtualQuery` only after a failure
   - #156: ray-tracing work (acceleration-structure reads by slot)
   - #173: ray-traversal fix (patch applied by hand; its author is Bennet0k)
   - #191, #141, #187: submission relay and in-flight waits (#187's relay left opt-in, `VICTORY_RELAY=1|2`)
   - #147: shader cache keyed by the translator's own source
   - #95, #92, #177: texture and memory-lock reworks, banded detiling
   - #47, #45, #42, #48: further fixes from the same group
   - Not taken: upstream #1529 and 6d021fe, pipeline pre-build commits c47b529 / 9c58a1c / 819f622, #175, #186 (conflicting designs).
7. **ScarlettPort changes** on top: Windows 10 Agility SDK opt-in, automatic input (`VICTORY_AUTO_INPUT`), a live press file with stick tokens, capture-every and frame dumps, fast boot (`VICTORY_FAST_BOOT`, testing only) and save slots (`VICTORY_SAVE_SLOT`), `SCARLETT_*` aliases for every `VICTORY_*` switch, a host descriptor-words table (descriptors read as numbers), exposure, ambient and shadow-mask stand-ins (off by default), `VICTORY_SHADER_OPT` and the relay left off by default, a `VICTORY_RAYTRACING=off` answer, and test switches (`VICTORY_TEST_NO_GPU`). Since the earlier snapshot: keyboard and mouse played like a PC game (`VICTORY_KBM`, `docs/input.md`), a splash inside the title's window, pipelined DirectStorage reads, precompiled graphics and compute pipelines (`VICTORY_PRECOMPILED_PSO`), linked mesh/pixel pairs, a geometry-stage path for merged ES-GS programs, ReBAR host copies (`VICTORY_REBAR`), and the opt-in speed switches for indirect clamps, barriers, synchronisation and memory copies (`VICTORY_INDIRECT_*`, `VICTORY_BARRIER_*`, `VICTORY_SYNC_PRECISE`, `VICTORY_KNOWN_WRITES`, `VICTORY_FAST_HASH`, `VICTORY_COPY_BACK_ONLY_READBACK`, ...; see `docs/RUNTIMES.md`). The ray-tracing node test (`image_bvh_intersect_ray`, `image_bvh64_intersect_ray`) is emulated as inline DXIL in `crates/victory-shader/src/emit/intersect.rs`.

Authors whose commits are in the range `4e17478..fx-cand` (kept in the history of our working repository, not in this snapshot): Xeltax, fever308, iurysf, fxceless, UNYTIX, sedzz, ruvenesh, marquezini, gdawid94, Matheus, Looper_Salty, Last Thylacine, Christian Blomert, Bennet0k, Adrito, myokex, sagouindupm, uiliannn, lukka, DoubleD17, AscilCH, AlanQiang0418, God489, iNaxxs, Rivaldo Freitas de Carvalho, Ziggy.

Also credited: XCL by ivall and contributors (https://github.com/ivall/xcl, GPL-2.0-only), used as a reference for how a title behaves under another layer.

## This snapshot

- Source: branch `fx-cand` at `bce072d` of our fork's working repository: `fx-final` at `3b720c9` (the rectangle-list topology decode fix that removes the tile grid, `fx-wipe`, on top of `26f8afd`) plus the read-wait branches below. Every new switch is off by default in the runtime; `scripts/play-victory.ps1` and the launcher turn on the verified ones (see `docs/RUNTIMES.md`).
- Merged since the previous snapshot (`fx-final` at `26f8afd`, which this repository took in 56e088e), by branch:
  - rectangle-list pipelines decoded right (`fx-wipe`, in `fx-final` 3b720c9): the tile grid over gameplay is gone
  - read-back fix (`VICTORY_READFIX`) and copy-back only for read-back buffers (`VICTORY_COPY_BACK_ONLY_READBACK`, `fx-rbonly`)
  - narrow root signatures, views and BVH (`VICTORY_NARROW_ROOT`, `_VIEWS`, `_BVH`)
  - prepared barriers, tables and list reuse (`VICTORY_PREP_BARRIERS`, `_TABLES`, `_REUSE`)
  - f3 full view translation (`fx-f3full`, default off) and the cache key naming it
  - the shader cache keeps fx-final's keys by default (`fx-cachekey`)
- Merged in the previous snapshot (`fx-final` at `26f8afd`; before it `fx-harvest` at `38e8e7c`, which this repository took in 48ba23a), by branch:
  - boot fixes
  - saves fix (`fx-saves`, `fx-saves-h2`)
  - splash and account picker (`fx-splash`)
  - keyboard and mouse (`fx-kbm`)
  - pacing (`fx-pace`)
  - readback precise settles (`VICTORY_READ_SETTLE_PRECISE`, off by default in the runtime; the play script turns it on)
  - indirect batching (`fx-batch`)
  - dispatch hazards (`fx-dispatch`)
  - memory-copy cuts (`fx-mem-copy`)
  - loading pipeline (`fx-load`)
  - msfix pipeline cache (25,200 of 25,471 pipelines prebuilt by `victory precompile-graphics`; `fx-msfix`, off by default)
  - mem2, the second round of memory work (`fx-mem2`, off by default)
  - cutscene diagnostics (`fx-cutscene`)
  - audio (`fx-audio`)
  - overlay (`fx-overlay`)
  - stable watchdog (`fx-stable`)
  - cachekey opt-in (`fx-cachekey`)
  - f3 view fix (`fx-f3view`, default off)
- `patches/victory-stack.patch` (repository root) is the diff of this tree against fxceless/victory-fork `main` at `4e17478` (`git diff 4e17478 fx-cand`). The patch is generated from this tree, so it carries the wording edits below.
- The base already took the title's name out of its tree; this snapshot keeps it that way. Two doc comments (`readback.rs`, `descriptor/slot_map.rs`), one line of `VICKY.md` (a former repository name) and one line of `README.md` (a note on how the fork was written) differ from the working repository for that reason.

Build and run: see `docs/BUILDING.md`, `scripts/setup-victory.ps1` and `scripts/play-victory.ps1`.

- ScarlettPort naming (this repository): `apps/victory/build.rs` stamps a version resource (ProductName and FileDescription "ScarlettPort"), and top-level window captions start with "ScarlettPort" (`crates/victory-hle/src/modules/lifecycle.rs`). The release ships the binary as `runtime\ScarlettPort-Runtime.exe`.
