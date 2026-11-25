# Technical status

Measured on: Windows 10 22H2, RTX 3060 12 GB, 32 GB RAM. Series S profile (`--console series-s`) with the title's own upscaler.

## Summary

- The VIctory-based runtime boots, gets through the menus, saves and legal screens, reaches the **lit world** and starts gameplay at about **4 fps** in the prologue on an RTX 3060, now GPU-bound.
- The **tile-grid overlay is fixed** (rectangle-list pipelines decode right). **Streaming freezes** and the GPU cost remain.
- **Shadowed areas are too dark.**
- The **intro cutscene is very slow** unless it is skipped.
- **Not yet verified playable.**

## What works
- Executable maps, all imports bind, the title's console DLLs load and run.
- Intro, first-boot setup and menus at 30 fps; audio plays. No Xbox Live: sign-in fails cleanly and the title carries on offline.
- World data streams in through DirectStorage; the open world loads.
- VIctory-based runtime: the world is drawn with the title's own lighting. Autosave and reload of a checkpoint are reported working by the base (`runtime/victory/README.md`).
- XCL: world-start GPU crash fixed (the title survives 7+ minutes in the world), pipeline journal and offline build, launcher.

## What does not
- Shadowed areas are too dark: the title's ray-traced bounce light and shadows do not run yet (`runtime/victory/docs/fork-ray-tracing.md` describes the work so far).
- About 4 fps in the prologue. The per-frame read waits are cut from about 156 to about 26 ms, so the frame is now GPU-bound. Streaming freezes remain. The intro cutscene is very slow unless skipped.
- Some HUD text is cut off in captures.
- Some cards lose the first room's walls and roof with certain switches (`runtime/victory/VICKY.md`).
- Memory use is high (the base reports around 24 GB in use during play).
- XCL: the final image is black; first-run shader compiles stall the world; two-pass mesh draws (`XCL_TWO_PASS_INDIRECT=1`) can reset the GPU.

## Notes on measurement
- The 4 fps figure was taken with `VICTORY_OWN_UPSCALER=1`, `VICTORY_NO_WATCHDOG=1`, `VICTORY_RELAY=2`, `VICTORY_SYNC_PRECISE=1`, `VICTORY_READ_SETTLE_PRECISE=1`, `VICTORY_PRECOMPILED_GRAPHICS=1`, `VICTORY_READFIX=1`, `VICTORY_COPY_BACK_ONLY_READBACK=1`, `VICTORY_NARROW_ROOT=1`, `VICTORY_NARROW_VIEWS=1`, `VICTORY_NARROW_BVH=1`, `VICTORY_PREP_BARRIERS=1`, `VICTORY_PREP_TABLES=1`, `VICTORY_PREP_REUSE=1` and `VICTORY_PIPELINE_WAIT_MS=0` (what `play-victory.ps1` sets), with the experimental msfix and mem2 switches off.
- Other numbers (shader coverage, per-feature timings) are in `runtime/victory/README.md` and `runtime/victory/VICKY.md`; they were measured by their authors on their own machines, not on the reference machine above.

## Shader coverage
- VIctory-based translator: all **26,084** programs translate. **26,013** pass DXIL validation on their own; the other **71** are oversized mesh shaders (outputs past the host's 32 KiB) that validate only when linked with their pixel shader (`VICTORY_LINK_PAIRS`, `crates/victory-shader/src/emit/link.rs`). So "100% translated" is true; "100% validate on their own" is not.
- `victory warm` (the launcher's shader build) translates 25,889 distinct shaders from the title's archives into the cache in about 171 s on the reference machine.
- 25,200 of 25,471 graphics pipelines are prebuilt by `victory precompile-graphics` (491 s). Play runs use them (`VICTORY_PRECOMPILED_GRAPHICS=1`) and the launcher runs that step; the rectangle-list decode fix is in this copy.
- XCL counts differently: its precompile walks the 26,503 records of the shader pack and goes through its own HLSL and DXC path, so its translated and compiled numbers are not the VIctory ones above. Its last ~4,000 records (from about 83%) are large compute shaders whose driver compiles take 6–190 s each.

## Known gaps
- Ray tracing: structure builder and trace programs are partly translated and off by default.
- On 32-lane GPUs, wave64 compute groups larger than 64 threads and whole-wave sections.
