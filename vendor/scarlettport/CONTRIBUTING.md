# Contributing

AI coding agents are welcome. The rules are the same for code written by hand and code written by an agent.

## The short version

1. **Pick a task** from the task board in the README, or open an issue. Comment on it to claim it so nobody duplicates your work.
2. **One change per PR**, ideally under ~400 changed lines. Split anything bigger.
3. **Test it on real hardware** and say what you ran: GPU, driver, Windows build, and what changed (fps, how far the game got, which log lines appeared or went away).
4. **Never commit game files.** No executables, `.rpf` archives, shaders dumped from the game, `MicrosoftGame.config`, saves or logs that contain game data. `.gitignore` blocks the common ones, but check your diff anyway.
5. **Keep upstream licenses and credits** in `runtime/xcl` and `runtime/victory`.
6. **One author per commit.** Commit messages must not contain `Co-Authored-By` trailers; CI rejects them. The author of a commit is the person who takes responsibility for it and who the license and history attribute it to, so a commit has exactly one. Credit others in the PR description instead.

CI (`.github/workflows/ci.yml`) also checks that no `.exe`, `.dll`, `.pdb`, `.rpf` or file over 2 MB is tracked, that `scripts/*.ps1` parse, and that the runtimes and launcher build. Build instructions: [docs/BUILDING.md](docs/BUILDING.md). Direction: [docs/ROADMAP.md](docs/ROADMAP.md).

## PR description

```markdown
<What changed and why. "Fixes #N".>

**Tested**
- GPU + driver, CPU, Windows build
- what you ran and what happened (numbers if you have them)

**Not tested**
- what you couldn't check (AMD/NVIDIA, Windows 10/11, …)

**AI use**
- none / tool + model, and what it did
```

## Working on a runtime

- `runtime/xcl`: read `runtime/xcl/AGENTS.md` for its build, test and code-style notes.
- `runtime/victory`: `cargo rustc --release -p victory --bin victory --features diagnostics --locked -- -C link-arg=/EXPORT:D3D12SDKVersion,DATA -C link-arg=/EXPORT:D3D12SDKPath,DATA`
- Fixes that are general (not title-specific) are worth upstreaming too. Each runtime has its own repo.

## Useful switches

| Variable | Runtime | Effect |
|---|---|---|
| `XCL_LAZY_PSO=0` | XCL | compile every pipeline up front instead of on first use |
| `XCL_PRESS_AT=20:A,23:DOWN,…` | XCL | scripted controller presses (seconds:button) |
| `XCL_DXC_THREADS=N` | XCL | shader compile threads |
| `VICTORY_AUTO_INPUT=1` | VIctory | press A every 2 s for the first 2 minutes |
| `VICTORY_WAVE64_EMULATE=1` | VIctory | emulate 64-lane waves on 32-lane GPUs |
| `VICTORY_CAPTURE_EVERY=N` | VIctory | save the frame and every render target every N frames |
| `VICTORY_SHOW_TARGET=WxH-fN` | VIctory | show an internal render target in the window (with `VICTORY_DEBUG_PS=1`) |
