# KLBF Lab patch notes

Every KLBF Lab version, newest first. Measured on one machine: Windows 11, AMD Radeon RX 9070 XT, Ryzen 7 9800X3D, 32 GB RAM. Full run log: [docs/klbf/results.md](docs/klbf/results.md).

Product and controller names (Xbox, DualSense…) are only used to say what a build works with. They belong to their owners; KLBF Lab is not affiliated with or endorsed by them.

## 🧪 Brewing
- **v8**: latest fxceless/victory-fork (shader clean-up passes) + our AMD fixes; candidate fix for the purple ambient light (upstream #1696); speed: fork #95 and #71; Series S + FSR (fork #32, #86); ahead-of-time shader compilation.

## AMD test branch `klbf-amd` — 24 Nov 2026
- **Base:** fxceless/victory-gta6 `578d459` + VIctory #1599.
- **AMD fixes:** memory leak (shader `39f0e9aeb156a61c` stuck in the AMD compiler, skipped by default); 32-lane pixel shaders run per half wave on 64-lane hardware (purple blocks); NaN/Inf filter on colour outputs; larger, configurable texture upload queue (late/missing textures).
- **Result on RX 9070 XT:** room fully lit, ~4.4 fps stable; 6–9 fps with experimental settings (device removed after ~3 min). Ambient light still purple; triangle edges, dark faces remain.

## Lab 23 (current) — 24 Nov 2026
- **Base:** the [fxceless/victory-gta6](https://github.com/fxceless/victory-gta6) fork at `578d459`, unchanged. All credit to its author.
- **What it does on our card:** the first gameplay scene is drawn (characters, buildings, the yard); ~2 fps after warm-up; no device removal.
- **Known problem:** committed memory keeps growing; our run stopped at 514 s when Windows ran out of virtual memory.

## Lab 21 / 22 — 24 Nov 2026 (test builds, not published)
- Shader coverage fixes on the Lab 15 base: #1250, #1230, #1273, #1301 (+ two renamed instructions), then #1130, #298, #1074, #385, #1412, #953, #539, #704 + #193.
- **Result (Lab 21):** 25 min with no device removal, median 283 ms/frame (~3.5 fps) in the first gameplay scene, mission objective shown, picture still dark.

## Lab 15 — 24 Nov 2026 (tag `klbf-lab-15`)
- **Base:** the [AnyXboxX](https://github.com/LooperSalty/AnyXboxX) integration by LooperSalty, plus #431 (AntoninMichelq) and #67 (alessiodeia).
- **Result:** 25 min in the world with no device removal; ~1 fps; rough world shapes visible.

## Lab 03 — 24 Nov 2026 (tag `klbf-lab-03`)
- VIctory + #452, #431, #67, #584 (limit raised), #493.
- **Result:** 1,006 s with no device removal; pad input and rumble work; black world.
