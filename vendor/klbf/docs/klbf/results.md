# Test log

One machine: **AMD Radeon RX 9070 XT 16 GB** (RDNA4, Adrenalin 32.0.32029.2010) · Ryzen 7 9800X3D · 32 GB DDR5 · Windows 11 (26200).
Text only, no screenshots. Times are seconds from launch; the world starts at about 60 s.

| Date (UTC) | Project | Build | Result |
|---|---|---|---|
| 2026-11-24 22:36 | **AMD test branch (v6), card buffers** | same + `VICTORY_CARD_BUFFERS_MIN_MB=1 VICTORY_CARD_BUFFERS_MB=1024`, `VICTORY_LATE_WAIT=1`, `VICTORY_WIDE_BACK=1`, no DRED | **5.3–5.5 fps (≈182–190 ms/frame) in the cell; device removed after ~11 min** (with `VICTORY_LATE_WAIT=1` and card buffers); frame: ~58 ms CPU preparing, ~103 ms on the GPU, ~24 ms copying back; GPU ~67% busy |
| 2026-11-24 22:16 | **AMD test branch (v6)** | v3 fixes + #1599 + texture queue 1024 MB / 1000 ms, 44 AMD skips | **~4.4 fps (≈220–230 ms/frame) in the cell, stable, no device removal**, with `VICTORY_LATE_WAIT=1 VICTORY_WIDE_BACK=1` (same with or without DRED); room fully lit (ambient still purple). With `VICTORY_LATE_WAIT=3` + card buffers 1 GB: 6–9 fps (peak 112 ms/frame) but device removed after ~3 min |
| 2026-11-24 21:23 | **Purple fix v3** (test build) | fork `578d459` + KLBF caption + skip `39f0e9aeb156a61c` + 43 AMD compiler-crash skips + 32-lane pixel shaders run per half-wave on 64-lane AMD hardware (per-half ballots and reductions) + NaN/Inf guard on float outputs | **purple/magenta lighting gone on AMD**; ~28 min in the world, played from the cell through the corridor, the office cutscene and the robbery (2-star wanted level), ~3 fps, committed memory flat at ~26.6 GB, no device removal; stopped by the title's own breakpoint (`0x80000003`). Still: ambient light missing (dark rooms), triangle edges visible on lit surfaces, white eyes, invisible skin parts, some missing walls, over-exposure in bright scenes |
| 2026-11-24 21:05 | Purple fix v1/v2 (test builds) | per-half wave handling only / plus denormal flush | half-wave handling removes most purple but darkens the room; denormal flush adds triangle-edge lines (reverted) |
| 2026-11-24 20:48 | Lab 25 halves (25a GPU PRs, 25b shader PRs, 25c control) | bisection of Lab 25's black picture | 25a and 25c draw the scene (~3 fps); 25b turns black after ~3 min: one of the shader-translation PRs blanks the picture |
| 2026-11-24 20:13 | **Lab 24** | Lab 23 + memory diagnostics, with compute shader `39f0e9aeb156a61c` skipped (`VICTORY_SKIP_PIPELINES`) | **memory leak found and stopped**: that one shader never leaves the AMD driver's compiler and the process grew ~240 MB/s while it waited; skipped, committed memory stays flat at ~24.6–25.6 GB; 663 s, stopped by script, no device removal |
| 2026-11-24 19:41 | **Lab 21** | Lab 19 + #1301 (RDNA2 opcode tables) + two renamed instructions routed | **1,505 s, stopped by script, no device removal; median 283 ms/frame in the world (~3.5 fps, 85 samples)**; mission objective shown; picture still dark |
| 2026-11-24 19:33 | **Lab 23** | [fxceless/victory-gta6](https://github.com/fxceless/victory-gta6) `578d459`, unchanged, `VICTORY_D3D12_DRED=1` | **the first gameplay scene renders on AMD**: characters, buildings and the yard are drawn (seen with the game's high-contrast option on); 1.4 s/frame at first, then **~0.52–0.55 s/frame (~2 fps)**; no device removal; stopped at 514 s when Windows ran out of virtual memory: the process had committed ~103.6 GB (32 GB RAM + 90 GB page file; `memory allocation of 2097152 bytes failed`) |
| 2026-11-24 19:07 | Lab 19 | Lab 17 + #1273 (real UAVs on placed textures) | 1,503 s, stopped by script, no device removal; **~500 ms/frame in the world (2 fps, 90 samples), twice as fast as Lab 15**; picture still dark |
| 2026-11-24 18:42 | Lab 18 | Lab 15 + fewer write-watch calls (one call over busy pages) | 1,503 s, stopped by script, no device removal; ~1,025 ms/frame in the world (92 samples); watch time unchanged (~420–450 ms/frame): no gain |
| 2026-11-24 18:17 | Lab 17 | Lab 15 + #1250 + #1230 (zero element counts, missing-value reachability) | 1,504 s, stopped by script, no device removal; ~1,017 ms/frame in the world (91 samples); picture unchanged (still mostly black) |
| 2026-11-24 17:50 | Lab 16 | Lab 15 + #1215 | 1,503 s, stopped by script, no device removal; ~1,000 ms/frame in the world (89 samples): no speed or picture change |
| 2026-11-24 16:23 | **KLBF Lab 15** | AnyXboxX integration + #431 + #67, `VICTORY_SHADER_LOOP_LIMIT=1024`, `VICTORY_CAPPED_DYNAMIC=262144` | **1,503 s, stopped by script, no device removal**; ~1,030 ms/frame in the world; partial world geometry visible |
| 2026-11-24 16:02 | AnyXboxX integration (unchanged) | `LooperSalty/victory` `anyxboxx/stable`, its launcher settings (loop limit 32) | **1,221 s, stopped by us, no device removal**; pad works; first world geometry visible on this card (with high-contrast accessibility on); ~1 fps |
| 2026-11-24 15:54 | Lab 14 | Lab 03 + #1023 (ordered queue/memory waits) | 455 s, stopped by us; no visible world change; earlier run 116 s (same intermittent fault) |
| 2026-11-24 15:41 | Lab 11 | Lab 03 + `GetWriteWatch` batched per block | 859 s stopped by us, then 119 s (intermittent low-address fault in compute `37e19ccb4f889f63` / `eeed82b877b44b5f`); no speed gain |
| 2026-11-24 16:19 | Lab 12 | Lab 03 + #1036 + #1015 + #1019 | 130 s, low-address page fault (`0x2f32000`); no picture change before it |
| 2026-11-24 16:08 | Lab 10 | Lab 09 + fix for blocks committed without a remap notice | 194 s, low-address page fault (`0x2a0a000`); watch time split: ~300 of ~337 ms/frame is the kernel `GetWriteWatch` side |
| 2026-11-24 15:53 | Lab 09 (ours) | Lab 03 + unmapped view blocks reported once instead of re-read every submission | unmapped re-reads 484 → 11 MB/frame, frame time 900–1000 → 680–780 ms; but device removed at 542 s (low-address page fault) |
| 2026-11-24 14:37 | Lab 08 | Lab 03 with #955 instead of #584 | no visible change (world still blank/black at ~1 fps); closed by us |
| 2026-11-24 14:20 | Lab 07 | Lab 03 + #393 + #388 + #230 (host copy work) | 433 s, no device removal; still ~1 fps: bytes *checked* per frame 11.4 → 7.1 GB, but ~10 GB per frame are still *refreshed* (copied) |
| 2026-11-24 14:17 | Lab 03 + `VICTORY_SHADER_LOOP_LIMIT=64` | – | no speed-up on this card (GPU 3–7 % busy); device removed at 161 s (low-address page fault) |
| 2026-11-24 13:50 | Lab 05 | Lab 03 + #641 | 254 s, low-address page fault in compute `37e19ccb4f889f63`: #641 brings back the fault #493 removed |
| 2026-11-24 13:46 | Lab 04 | Lab 03 + #641 + #393 | 184 s, same low-address page fault |
| 2026-11-24 13:30 | **KLBF Lab 03** | same, no scripted keys, XInput pad | **1,006 s, stopped by us, no device removal**; pad input and rumble work, mission objectives advance; world still black at ~1 fps (6,500 draws/frame), menus 10–93 fps; 16.5 GB working set, GPU ~10 % busy |
| 2026-11-24 13:14 | **KLBF Lab 03** | **+ #584 + #493** | **483 s, stopped by us, no device removal; gameplay reached (control prompts)** |
| 2026-11-24 13:11 | KLBF Lab 02 | + #584 | 160 s, device removed: page fault at a low address (`0x29f4000`) in a compute dispatch |
| 2026-11-24 12:54 | KLBF Lab | + #493 (bounded root SRVs) | 147 s, device removed: read far past the end of the view heap (`0x1f00f0000`) |
| 2026-11-24 12:38 | KLBF Lab | + #688 (dynamic slots on a capped heap) | 144 s, device removed, page fault |
| 2026-11-24 11:19 | **KLBF Lab** | **`klbf-lab` branch, `VICTORY_D3D12_DRED=1`** | **221 s: ~160 s in the world, audio and subtitles, ~1 fps; then device removed + page fault** |
| 2026-11-24 11:16 | KLBF Lab | same, without DRED | 87 s, device hung |
| 2026-11-24 10:30 | KLBF Lab | `klbf-lab` branch | 160 s, ~100 s in the world |

Notes:
- On this card every change we stacked on top of Lab 03 that touches how the GPU reads title memory (#641, #1019 group, our unmapped-skip) ends in the same low-address page-fault class (compute `37e19ccb4f889f63` / `eeed82b877b44b5f`). Lab 03 stays the stable baseline.
- `VICTORY_KEYS` makes the keyboard stand-in pad the signed-in user's pad: a real pad then only reaches the pause button. Leave it unset to play with a pad.
- Alt+Enter (fullscreen) freezes the picture in the world: stay windowed.
- `VICTORY_D3D12_DRED=1` made runs clearly more stable on this card.
