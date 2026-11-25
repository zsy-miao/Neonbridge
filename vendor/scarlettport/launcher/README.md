# ScarlettPort launcher

Native Rust launcher (eframe/egui). It embeds no runtime: it runs the engines from a `runtime`
folder beside `ScarlettPort.exe` (the release ZIP has both), and never unpacks, downloads or
writes executables. Settings live in `%LOCALAPPDATA%\ScarlettPort\launcher.json`. Network: an optional update
check against GitHub Releases (`src/update.rs`; downloads only on click, verified by SHA-256,
swapped in at the next start, previous version kept). Discord status over the local IPC pipe
(`src/discord.rs`, `DISCORD_APP_ID`, empty until the project's app exists). One-click hardware
report (`src/report.rs`) opens a prefilled issue.

- **Play**: pick the folder containing `game_scarlett_master.exe`, press BUILD SHADERS once, then PLAY.
  The header shows your own game's splash art, read from that folder at run time (nothing is shipped).
- **Engines**: World (VIctory-based runtime, `runtime\victory.exe`, default) and Classic (XCL, `runtime\xcl.exe`, when present).
- **Shader build** (`src/shaderbuild.rs`): World runs `victory warm` (about 3 min) and then `victory precompile-graphics` (about 8 min); Classic runs
  `xcl shaders ... --precompile --pipelines --only cs`. Progress per phase with the current step and its
  elapsed time; parallel jobs bounded by CPU count and free RAM; resumable (`<cache>\build-*.json`);
  optional skip of slow steps (Settings, Options).
- **Presets** (`src/presets.rs`): Stable (the verified C1 environment), Experimental (C1 plus the
  safe fx-cand2 switches, its own cache) and Custom. **Runtime**: Bundled, or a Local build (any
  `victory.exe`; updates never touch it).
- **Settings, Features** (`src/tuning.rs`): full textures, HDR, PlayStation pads, rumble, keyboard
  hints, frame cap, upscaler/FSR/frame generation, Discord. Set as `VICTORY_*` variables and
  written to `%LOCALAPPDATA%\ScarlettPort\victory.toml` (passed as `VICTORY_CONFIG`).
- **Verify game files** (`src/verify.rs`): archives, a complete `pack\` (flags a missing or split
  one), `dump-manifest.log`, links, empty files. Read-only.
- **Saves** (`victory saves`), **Logs and crashes** (`victory crashes list`, copy report), a
  performance readout after each run (`src/perf.rs`), and a first-run wizard.
- **Progress notes**, **Links**.

## Build

```
cd launcher
cargo build --release --locked      # -> target\release\ScarlettPort.exe
```

Put `victory.exe` and its `D3D12\` folder in `target\release\runtime\`, or set `SCARLETT_RUNTIME_DIR`.
`ScarlettPort.exe --screenshot=FILE.png` renders the window to a PNG and exits; `--page=play|verify|saves|logs|settings`,
`--tab=runtime|features|...` and `--wizard=0..4` pick what it shows.
Releases: `.github/workflows/release.yml` (tag `v*`).