# How each runtime is wired

How Neonbridge builds, prepares and starts each runtime in `vendor/`. Every setting below is its author's own (the source is noted in its `vendor/<name>/NEONBRIDGE.toml`); the only things Neonbridge adds are listed under [What Neonbridge adds](#what-neonbridge-adds).

All seven are builds of [VIctory](https://github.com/Marvisak/VIctory) (Marvisak): the same `victory.exe` with the same commands (`run`, `translate`, `warm`). They differ in their fixes, their switches and their shader cache format.

## Family tree

```
VIctory (Marvisak)
├── victory-fork (fxceless)          → vendor/victory
│   ├── Flamingo (realblastex)       → vendor/flamingo
│   ├── VI-BR-PORT (edsuuu)          → vendor/vibrport
│   └── KLBF Lab (PastiiJay)         → vendor/klbf   (VIctory + victory-fork)
├── VIctory-Belobug-AMD (Belobug)    → vendor/belobug
├── VItoriaBr (uiliannn)             → vendor/vitoriabr
└── ScarlettPort (ZigZagBoi1)        → vendor/scarlettport/runtime/victory
```

## At a glance

1. **KLBF Lab** (`klbf`)
   - Build: `cargo build --release` → `target/release/victory.exe`
   - Prepare: `translate <game> --cache --pipelines`
   - Run: `run <game exe> --execute` (Series X profile). The `fsr` preset adds `--console=series-s` and AMD FidelityFX
   - Its switches: `SHADER_OPT=0`, `SKIP_PIPELINES=39f0e9aeb156a61c`, `LATE_WAIT=1`, `WIDE_BACK=1`, `PIPELINE_WAIT_MS=24`, `RELAY_COPY_QUEUE=0`
   - Shader cache: VSH1
2. **Flamingo** (`flamingo`)
   - Build: `cargo build --profile diag` → `target/diag/victory.exe`
   - Prepare: `translate <game> --warm all`
   - Run: `run <game exe> --execute --console series-s`
   - Its switches: ray tracing off, `NO_WATCHDOG`, `RELAY=1`, `LATE_WAIT=3`, 1 GB card buffers, `WIDE_MERGE`, its own on-screen display
   - Shader cache: VSH2
3. **victory-fork** (`victory`)
   - Build: `cargo build --release` (`--profile diag --features diagnostics` for diagnostics)
   - Prepare: `translate <game> --warm all`
   - Run: `run <game exe> --execute` (Series X profile)
   - Its switches: ray tracing off
   - Shader cache: VSH1
4. **VI-BR-PORT** (`vibrport`)
   - Build: `cargo build --profile diag --features diagnostics` → `target/diag/victory.exe`
   - Prepare: `translate <game> --write-cache --used-only`
   - Run: `run <game exe> --execute` (Series X profile)
   - Its switches, for 8 GB cards: `RELAY=1`, `WIDE_BACK`, `WIDE_REFRESH`, `DISPLAY_MODE=1920x1080`, `NO_WATCHDOG`
   - Shader cache: VSH1
5. **VIctory-Belobug-AMD** (`belobug`)
   - Build: `cargo build --release`
   - Prepare: `translate <game>`
   - Run: `run <game exe> --execute`
   - Its switches: none (its defaults are tuned for AMD Radeon)
   - Shader cache: VSH2
6. **VItoriaBr** (`vitoriabr`)
   - Build: `cargo build --release`
   - Prepare: `translate <game> --warm all`
   - Run: `run <game exe> --execute`
   - Its switches: ray tracing off, `RELAY=2`, `TEXTURE_NO_WAIT`, 6 GB card buffers, `WIDE_REFRESH`, `WIDE_MERGE`, `JOIN_COPIES`, `DISPLAY_MODE=1920x1080`
   - Shader cache: VSH1 / VSH3
7. **ScarlettPort** (`scarlettport`)
   - Build: `cargo rustc` of `runtime/victory` with diagnostics, as its `play-victory.ps1` does
   - Prepare: `warm <game>`
   - Run: `run <game exe> --execute --console series-s`, with the D3D12 Agility SDK 1.614.1 beside the exe
   - Its switches: its own upscaler, `NO_WATCHDOG`, `RELAY=2`, precise sync, precompiled graphics pipelines, the `NARROW_*` and `PREP_*` set, ray tracing off
   - Shader cache: VSH1

Every switch above is a `VICTORY_` environment variable (`RELAY=1` means `VICTORY_RELAY=1`).

## What Neonbridge adds

The same for every runtime, and none of it changes how a runtime draws:

- **Folders:**
  - `VICTORY_GAME_DIR` points at your game.
  - `VICTORY_CACHE_DIR` points at the runtime's own cache, because the cache formats above differ.
  - `VICTORY_DATA_DIR` points at one save and settings folder for the whole family, `%LOCALAPPDATA%\6ixpack\save`.
- **Before the run:**
  - Checks the graphics card (`--check`) and the game files (`--verify`).
  - Lighter graphics settings in the game's `settings.xml` (`NEONBRIDGE_SAFE_SETTINGS=0` keeps yours).
  - The shader warm (the Prepare step above).
- **During the run:**
  - Auto-press A (`VICTORY_PRESS_FILE`, `NEONBRIDGE_AUTO_A=0` off).
  - The phone stream.
- **After the run:**
  - A frame-rate summary.
  - A cache backup.
- **Display settings, only when you set them:**
  - `NEONBRIDGE_RESOLUTION` sets `VICTORY_DISPLAY_MODE`.
  - `NEONBRIDGE_FPS_CAP` sets `VICTORY_PACE_FPS`.

## Mixing

The family shares one binding ABI, so a crate from one tree can usually replace the same crate in another. Mixing a VSH2 runtime (Flamingo, Belobug) with a VSH1 one rebuilds the shader cache. See "Mix and match" in the README: no mix has been tested yet.

## Known gaps seen here

- **VI-BR-PORT:** the game stops itself on a breakpoint about 20 s in, after the menus, right after calling a graphics-device method that VI-BR-PORT does not implement (`method not implemented object="device" slot=0x330`). Seen on an RTX 4060 8 GB, every run, with its author's settings; the other runtimes never call it. It does not work with Neonbridge yet.
