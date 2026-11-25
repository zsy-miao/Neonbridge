> Français : [lancer-gta6.md](fr/lancer-gta6.md)

# Running GTA VI (Xbox Series 1.0.2.1 build) with AnyXboxX

This guide installs and starts the project, and lists everything you need to repeat our tests on the Xbox Series 1.0.2.1 build of GTA VI, up to where we are today: the first cutscene. It does not provide the game and does not explain how to get it: you bring your own copy. Never put game files in this repository or attach them to an issue.

The launch goes through [VIctory](https://github.com/Marvisak/VIctory) (Marvisak, GPL-3.0 or later), copied into this repository under `third_party/victory`, where it is our own code: nothing is synced or pinned. The copy started from the branch `anyxboxx/fork-7b0-lot9-murs` of the maintainer's fork [LooperSalty/victory](https://github.com/LooperSalty/victory), then took 57 commits for Proton and AMD cards (see below). That branch assembles the fork [fxceless/victory-fork](https://github.com/fxceless/victory-fork) (GPL-3.0) at `7b0f736`, eight earlier pull requests of it, seven new ones, the port of the offline precompilation of #282 and commits of ours (`xg_xs.dll` layout answered like Microsoft's library, see below), at commit `80d58f8`. The script `tools/lancer_victory.ps1` builds it and starts it on your game folder.

## Neptune base (25 November, evening)

`third_party/victory` now holds the corresponding source of Neptune (VIctory Edit.) 2026-11-25c (GPL-3.0-or-later; it stands on `fxceless/victory-fork`, `myokex/victory-amd`, `soldsawn/victory-nvidia-8gb`, ShakiraRED-dt, `ZSlayerHQ/victory-world-runtime`, `JoshuuDev/victory-amd` and others, see [THIRD_PARTY.md](../THIRD_PARTY.md)), plus our four `xg:` commits (Microsoft's `xg_xs.dll` as an oracle, `-XgDll`) and Discord Rich Presence off unless `VICTORY_DISCORD=1`. The sections below describe the previous base `80d58f8` and its 57 Proton/AMD commits, which are not carried over.

```powershell
.	ools\lancer_victory.ps1 -GameDir <game folder> -Warm   # first time: about 7 min of shader translation, then the game
.	ools\lancer_victory.ps1 -GameDir <game folder>         # after that
```

The launcher builds the release profile (`-Diag` or `-Dred` for the diagnostics build) and sets Neptune's `play.ps1` switch set for the Series X path (16 GB card: lower `VICTORY_CARD_WINDOWS_MB` and `VICTORY_CARD_BUFFERS_MB` on a smaller one). `-Warm` runs `victory.exe warm` on the game folder's `.rpf` files only. `-AutoA` taps A through the start screens, `-FrametimeDump` writes a frame time CSV into the cache folder. In the game, F10 hides the figures in the corner and F12 saves a picture to Pictures.

Measured by the maintainer on 25 November (RTX 4070 SUPER, i7-13700KF, Series X path at 4K, world, after `warm`, release build): median about 12.5 fps, about 15 fps over the last minute, peaks at 30, about 7,500 draws per frame, no crash; the previous base ran at about 5 fps there. Known issues (Neptune's notes): objects and clothes briefly black while their textures are swapped, parts of the world missing behind some doors and on some roofs, no facial animation, frame time jumps while the game streams; indoor light is an even ambient stand-in for the ray-traced lighting.

## Proton and AMD fixes (25 November)

`third_party/victory` is no longer a submodule: it holds a copy of VIctory at the previous pin `80d58f8` (next section, unchanged), followed in this repository's history by 57 commits for Proton and AMD cards (`git log -- third_party/victory`). Each commit names its source; one taken from someone else ends with `Source:` and `Co-authored-by:` lines. Every source repository is listed in [THIRD_PARTY.md](../THIRD_PARTY.md).

An existing clone that has the submodule must remove it before pulling, or the checkout stops on the files already there:

```powershell
git submodule deinit -f third_party/victory
Remove-Item -Recurse -Force third_party\victory, .git\modules\third_party\victory
git pull
```

On Linux, `rm -rf third_party/victory .git/modules/third_party/victory` replaces `Remove-Item`. This also deletes `third_party/victory/target`: the first build starts over. The shader cache key depends on the translator's source, so a cache filled with `-Precompile` on `80d58f8` is filled again.

What the 57 commits add:

- **Proton (Wine)**: the runtime's own HLSL (argument clamp, card compare, depth plane pass) compiles with Wine's d3dcompiler, root signatures built in Rust (fork #127, BitWuehler); a host pipeline library is made under vkd3d-proton (Marvisak/VIctory #1722, Impre-visible) and the early pipeline list is kept without one; far fewer `GetWriteWatch`, `VirtualQuery` and `VirtualProtect` calls per frame; guards held open over the runtime's kernel writes into title memory, and a fault under the guards' own lock no longer deadlocks; raw DirectStorage and `fread` reads land in a buffer and are copied in from user mode (JoshuuDev/victory-amd, Joshua Odom).
- **AMD (64-lane hosts, view heaps of 2^21)**: wave32 pixel programs on a 64-lane host compute ballots and reductions per half wave (PastiiJay/klbf-lab, PastiiJay); view indices clamped to the heap's last, null slot (fork #134, triippywhale, and #98, sillynocodecat); 64-bit integer compares on their dwords (fork #135, alexdeliu); f16 wave sums where not every lane runs, on one 64-lane wave.
- **Shaders**: `v_cvt_f32_f64` and `v_add_f64` sums (fork #9, MartinIndra02), `v_permlane16_b32` and `v_permlanex16_b32` (fork #134, triippywhale), mesh primitive indices masked to 9 bits (Marvisak/VIctory #1718, tjthebigbearcat-hub), `v_interp_p2_f16` with `op_sel`, `omod` and clamp (Marvisak/VIctory #1696, Calvix2k), pixel inputs interpolated as the console container's PSV0 declares (fork #150, wildheathen), NaN guard on colour exports and typed stores (PastiiJay/klbf-lab, PastiiJay), clean-up passes over each translated module (fork commit `1684989`, fxceless).
- **Memory views and textures**: the texture guard sees writes through every view of its pages (fork #115, Spexivus; correction by joroyt via myokex); heap records and buffer views made from the bytes mirrored at each submission (port of Marvisak/VIctory `86a5ed6b`, Marvisak); placed unordered texture views mirrored onto host textures (fork #114, Kaivyyy); late waits also wait for aliased writes and in-flight staging buffers (fork #191, gdawid9407, #206, AscilCH, #141, adritoyalser); views' own copies held to a budget; placed texture views told apart by the memory their mips take, lost ones put back into the queue, long texture queues brought in faster for a bounded time, dispatches of more than 65,535 groups made in parts, constant buffer views held to 64 KiB, DirectStorage page arrays read when the request runs (myokex/victory-amd, myokex).
- **Pipelines**: a pipeline still being made is waited for only in the first frame that asks for it; loading-time pipeline waits bounded per run, ray-tracing resolve programs not made while the structure builder is off, early pipeline list merged across runs (myokex).
- **Diagnostics and tests**: per-frame counters, A/B switches, magenta for missing textures, a mesh pipeline test that skips without mesh shaders (fork #14, oddiums).

Switches read by this code. `tools/lancer_victory.ps1` sets none of them, so a launch gets the defaults:

| Variable | Default | What it does |
| --- | --- | --- |
| `VICTORY_SHADER_OPT` | every pass | Clean-up passes over translated shaders; `0` turns them off. Part of the shader cache key. |
| `VICTORY_PS_HALVES` | on | Half-wave ballots and reductions, only on a host whose pixel waves have 64 lanes (AMD); `0` off. |
| `VICTORY_L6_HEAP_CLAMP` | on | View heaps of 2^21 and clamped indices, only on a host that takes such heaps; `0` off. |
| `VICTORY_L6_NAN_GUARD`, `VICTORY_L6_PSV_INTERP`, `VICTORY_L6_PLACED_UAV`, `VICTORY_L6_NULL_LIBRARY`, `VICTORY_L6_PLANE_PASS` | on | NaN guard, PSV0 interpolation, placed unordered texture views, null-blob pipeline library, depth plane pass; `0` turns one off. |
| `VICTORY_L7_<NAME>` (`DYNAMIC_SLOTS`, `CB_SLOTS`, `VIEW_COPY_LRU`, `VIEW_ADDRESS`, `TEXTURE_SLOTS`), `VICTORY_L7_VIEW_COPY_MB` | on, `512` | View slots and copies; `0` turns one off. |
| `VICTORY_L9_DSTORAGE_GUARDS`, `VICTORY_L9_GUARD_TITLE_VIEWS` | on | Guards held open over kernel writes; guards arm title views only. |
| `VICTORY_TEXTURE_SWEEP` | on | Lost placed texture views put back into the queue; `0` off, `count` only counts. |
| `VICTORY_TEXTURE_GATE_S`, `VICTORY_PIPELINE_GATE_S` | `60`, `120` | Seconds per run spent bringing in a long texture queue, or waiting for pipelines while a world loads; `0` never. |
| `VICTORY_RT_RESOLVE` | not set | Ray-tracing resolve programs are made only with `1` (or `VICTORY_BVH_BUILDER=1`). |
| `VICTORY_L9_ALIAS_UNCLEAN`, `VICTORY_HEAP_RECHECK` | off | `1` turns one on. Off on purpose: on Linux, run 28 (both on) lost the characters' face geometry and run 30 (`HEAP_RECHECK` on) blackened face skin (observed). |
| `VICTORY_L7_ROLES`, `VICTORY_L7_SKIP`, `VICTORY_L7_UNLIT` | off | Diagnostics: draws skipped by shader role. The role table is built locally from your own copy and never goes into this repository. |
| `VICTORY_NULL_MAGENTA`, `VICTORY_CAPTURE_MENUS` | off | Diagnostics: magenta for missing textures; captures in menus. |

Measured on Linux only, with this code (built as `890a83a` before the copy, same tree; Fedora 44, RX 9070 XT 16 GB, RADV through vkd3d-proton, GE-Proton11-12 with a community view-tree wineserver patch, VIctory log of run 31, about 23 min from launch, offline shader cache warmed with `victory translate --cache`):

- World: 266 ms median world frame, about 3.7 fps. Measured.
- No crash, no device loss, no DirectStorage failure, no `ERROR` line in the log. Measured.
- Faces, skin and geometry complete, textures stable. Observed.
- Indoor and shadowed areas are black: ray tracing does not run (the structure builder is off and the run skips the ray-traced, GI and reflection passes). Observed.
- Earlier, on stock Proton, batch 6 ran at a 685 ms median world frame; with the wineserver patch and the write-watch cuts of this branch, run 14 (a local batch that already carried most of these commits) ran at 274 ms. Measured.

Settings of that run (not set by the launcher; Linux only):

| Variable | Value | Why |
| --- | --- | --- |
| `VICTORY_LATE_WAIT`, `VICTORY_WIDE_BACK` | `0`, `0` | With late waits the device is lost on RADV (fork issue #184). |
| `VICTORY_SHADER_LOOP_LIMIT` | `256` | Same as the launcher. |
| `VICTORY_PIPELINE_WAIT_MS` | `50` | Host pipeline wait per frame. |
| `VICTORY_TEXTURE_BUDGET_MB` | `8192` | Texture budget for a 16 GB card. |
| `VICTORY_TEXTURE_NO_WAIT` | `1` | Texture refreshes do not wait for title memory. |
| `VICTORY_CARD_BUFFERS_MIN_MB`, `VICTORY_CARD_BUFFERS_MB` | `1`, `1024` | Same as the launcher. |
| `VKD3D_CONFIG` | `descriptor_heap` | The title asks for 1,245,184 views, more than vkd3d-proton's default cap. |
| `VICTORY_SKIP_PIPELINES` | 9 shader hashes | `7f05ddab771799a9`, `a53a7977fe762268`, `be14e72d86362031`, `32ed072b9f845048`, `a1b20bc47e3c9597`, `17829ac71aab4e8d`, `e8c962358d862e3f`, `559d9f0184e15fe3`, `afeea48bbb9913ad`. |
| `VICTORY_L7_SKIP` | `rt,gi,reflections` | With a local role table (`VICTORY_L7_ROLES`): those passes cost time and draw nothing while ray tracing does not run. |

Not measured: Windows. The launcher's Windows values (`VICTORY_LATE_WAIT=3`, `VICTORY_WIDE_BACK=1`) are unchanged and have not been run with these 57 commits; most of the new behaviours are on by default and apply on every host. The Windows target builds without warnings (`cargo build --profile diag -p victory --features diagnostics`, cross-built on Linux) and the `victory-gpu` tests pass under Proton (158 passed, 2 ignored).

## Previous pin: batch 8 plus the xg layout, offline precompilation and seven fork pull requests (25 November)

`third_party/victory` pointed to the branch `anyxboxx/fork-7b0-lot9-murs` of [LooperSalty/victory](https://github.com/LooperSalty/victory), at commit `80d58f8fe2769d6c857eb27abc159b6cf64530a3`: batch 8 (below) plus 15 commits, listed by `git log --oneline 19feca8..80d58f8` of the branch:

Since `80d58f8` (branch `anyxboxx/fork-7b0-lot9-murs`), the pin also carries two commits by Yorivel from the fork (`e49864a`, `bf870de`, fork issue #62): DirectStorage writes into physical pages are reported to the GPU side when `VICTORY_DSTORAGE_NOTIFY=1`, which the launcher sets. Measured by the maintainer (VIctory log, 25 November, about 02:41 to 02:50, with the launcher settings above): from a new save into the world in about 9 minutes, 5.3 to 6.9 fps, 4,200 to 6,300 draws per frame, no device loss; full cutscenes, judged "incredible" (observed); some graphics are still missing in places. Most dropped draws come from the ray-tracing structure builder, which is off (14,943 of 15,444 in one log; measured).

- Three commits of ours, `xg:` (`1f0fe1d`, then the versions taken over of fork commits `4466d2e` and `a16f040`): the HLE of `xg_xs.dll` now answers exactly like Microsoft's DLL (tail mips in parent mode with a position; 64 KiB tilings indexed by subresource, packed mips zeroed, count = standard mips in the given room). A `victory-hle` test compares 54 descriptions with the real DLL when `VICTORY_XG_DLL` is set. Measured in game by the maintainer: 0 difference with the DLL. The DLL is therefore no longer needed for this gain; the launcher keeps `-XgDll` as an oracle.
- Offline precompilation, port of pull request #282 of `fxceless/victory-fork` (t4l03): `victory translate --cache --pipelines [--used-pipelines] <shaders.rpf> <exe> <dlls>` translates shaders and builds pipelines ahead of the game. The cache key no longer depends on the executable. One adaptation commit of ours drops `shader_opt` from the key (commit `1684989` of the fork is not in our base). The launcher runs it with `-Precompile`.
- Seven pull requests of the fork, cherry-picked with `-x` (authors in [THIRD_PARTY.md](../THIRD_PARTY.md)): #349, #355, #396, #386, #358 (2 commits), #353, #374, plus one adaptation commit for the tests of #353.

| Fork pull request | Author | What it does |
| --- | --- | --- |
| #349 | AscilCH | `v_div_fixup_f32`: `0.0/x` and `x/Inf` no longer give an infinity |
| #355 | AscilCH | Signed flag passed to `wavePrefixOp.f32`, integer NaN check in `v_cvt_{u16,i16}_f16` |
| #396 | JordanFableFur | The rectangle fallback's f16 rounds to even and keeps NaN |
| #386 | JordanFableFur | A sampler used with and without comparison gets two slots |
| #358 | HailLucifer666 | Give back exactly the pages a placement locked; no in-place placement under late waits |
| #353 | AscilCH | MIMG 3D Z texel offsets, a16 offset truncation, cubemap `sample_d` LOD |
| #374 | davutsimsekk | BCPack: keep all 32 indices in the first BC7 dictionary entry |

Measured or observed by the maintainer (Windows 11, RTX 4070 Super, VIctory logs, night of 24 to 25 November), launcher settings of `main`:

- `-XgDll` in `compare` mode with the new HLE: 0 difference with Microsoft's DLL. Measured.
- Precompilation (`victory translate --cache --pipelines`): 25,689 shaders translated in about 4.5 min, 296 refused, cache of 13 GB in `%LOCALAPPDATA%\victory\cache`. Measured.
- World: 4.8 to 5.1 fps, about 12.7% of draws dropped per frame (11 to 14% before). Measured. The cutscene is judged better by the maintainer, nothing worse in game. Observed.
- Codex's analysis (read from the diffs): #374 (BC7) and #349 (`0/x` gave infinity) are the best leads for the corrupted squares and the exposure. Not yet verified in game.

Why objects and walls are missing in the world, found with `VICTORY_D3D12_DEBUG=1` (measured): 15 of the 16 pipelines refused by the driver (`0x80070057`) are wave64 compute shaders that NVIDIA refuses ("Shader wants wave lane count of 64 but maximum wave lane count for device is 32"); the 16th is a pixel shader that writes an integer into an R8_UINT target with blending. About 11 to 14% of the draws of each world frame are dropped for lack of a pipeline, in every run. Candidate fix: Marvisak/VIctory #1890 (wave64 split into two waves of 32), being ported; tracked in the issue "wave64 compute shaders refused on NVIDIA".

## Previous pin: batch 8 plus Microsoft's texture layout library (25 November)

`third_party/victory` pointed to the branch `anyxboxx/fork-7b0-lot8-xg` of [LooperSalty/victory](https://github.com/LooperSalty/victory), at commit `19feca8650c934913063076b612e87739d8d1912`: batch 8 (below) plus one commit of ours, `xg: serve or check the title from Microsoft's own xg_xs.dll (local copy)`. Without `VICTORY_XG_DLL` the code runs exactly as batch 8.

The title asks the console's texture layout library (`xg_xs.dll`: `XGSuggestSwizzleMode`, `XGCreateTextureComputer`) where each mip and slice lies. VIctory answers from its own reimplementation. The GDK ships an offline PC build of that library; with your own copy, `tools/lancer_victory.ps1 -XgDll <path to xg_xs.dll>` lets it answer instead. The DLL is a GDK file: it never goes into this repository, and the launcher only passes its path.

Measured or observed by the maintainer (Windows 11, RTX 4070 Super, VIctory logs, 25 November, about 00:37 and 00:42), launcher settings of `main`:

- `-XgMode compare` (VIctory's layouts, each difference logged once): 84 texture descriptions differ, all 64 KiB D textures (`layout` `0x10a`, some `0x105`). Tail mips are reported by the library in the parent mode (10) with a position in the tail; VIctory reports `256B_D` (2) and no position. Sizes, offsets and pitches match. The 64 KiB tiling lists every slice of an array with a mip tail (VIctory stops at two). No difference in suggested modes or in the tiled bytes of CPU copies. Measured.
- `-XgMode real` (default with `-XgDll`): 5.4 to 7.1 fps at the end of the run against 5.1 to 5.6, about 6,080 draws per frame, no device loss in about 3 min 40; judged the best run so far by the maintainer. Measured, then observed.

## Previous pin: batch 5 plus one pull request (25 November)

`third_party/victory` pointed to the branch `anyxboxx/fork-7b0-lot8` of the maintainer's VIctory fork [LooperSalty/victory](https://github.com/LooperSalty/victory), at commit `d138351de32f860ab9df1aa1aa736fce0a40365e`. It is batch 5 (below) plus one pull request of the fork [fxceless/victory-fork](https://github.com/fxceless/victory-fork), cherry-picked with `-x`:

| Fork pull request | Author | What it does | Commit |
| --- | --- | --- | --- |
| #11 | cxchhh | Shaders: parked lane-mask pairs a few instructions apart are restored; relative VGPR reads keep parked lanes | `d138351` (fork commit `90228a7`) |

Why: a PIX capture of one cutscene frame showed that `IrradianceCache_Shade` and `IrradianceCache_ShadeSecondary` did not run on batch 5 (0 dispatch). Their two shaders did not translate ("a wave reduction where not every lane runs"), so the irradiance cache was never lit and everything outside direct light stayed black (shaded side of faces, interiors). With #11 both shaders translate and pass DXIL validation, and their pipelines are created and bound in game.

Measured or observed by the maintainer (Windows 11, RTX 4070 Super), with the launcher settings of `main` (`tools/lancer_victory.ps1`), from the VIctory log and a PIX capture (trial of 25 November, about 00:01 to 00:05):

- Batch 5: PIX capture, 0 dispatch for both irradiance cache passes. Measured.
- Batch 8: both shaders translate and validate, pipelines created and bound in game. Observed.
- Untranslated shaders: 6 instead of 9. Pipelines refused by the driver: 16, as on batch 5. Measured.
- 4.8 to 5.4 fps, about 6,550 draws per frame, no device loss in about 4 min 20. Measured.
- Maintainer's opinion: "this version is very good".

Limits. The fork's issue #75 reports about 80 more refused pipelines on an RTX 3070: this was not seen on the 4070 Super, so expect more refusals on other cards. Ray-traced lighting is still absent (the BVH builder stays off, about 350 dispatches dropped per frame). With the `LATE_WAIT` setting of the launcher, some walls of the first room can be transparent (fork issue #168, fixes in progress: #191, #206). Long-run stability is measured over only about 4 min 20.

## Previous pin: batch 5, fork plus seven pull requests (24 November, night)

`third_party/victory` pointed to the branch `anyxboxx/fork-7b0-lot5` of the maintainer's VIctory fork [LooperSalty/victory](https://github.com/LooperSalty/victory), at commit `fd021bf26a7003749d668e5d9bf6b234bfab2e47`. That branch assembles the fork [fxceless/victory-fork](https://github.com/fxceless/victory-fork) (GPL-3.0, formerly `fxceless/victory-gta6`) at `7b0f736` and seven of its pull requests that were still open, cherry-picked with `-x`:

| Fork pull request | Author | What it does | Commit |
| --- | --- | --- | --- |
| #63 | arminkhaleghirod | Textures that changed lately are hashed at every check (fixes the startup flicker) | `d9770b9` |
| #71 | MartinIndra02 | D3D12 COPY queue for the relay card-copy fetches, parallel `CARD_COMPARE` | `5e758c7` |
| #32 | arminkhaleghirod | 16-bit float RGBA stands in for R9G9B9E5 render targets | `d214807`, `0e2c46a` |
| #82 | Kaivyyy | Depth bounds test passed through to host pipelines and command lists | `35cdf6f` |
| #83 | ryotdev | Keep empty-EXEC branches that can reactivate lanes | `194c736` |
| #81 | undeadhuman37 | A descriptor only read as data of eight dwords is a measured image | `6119c21` |
| #77 | m335671 | `v_fma_mixlo/mixhi` over an f16 first interpolation half keeps zero | `fd021bf` |

Measured or observed by the maintainer (Windows 11, RTX 4070 Super), from the VIctory log `victory-fork-lot5.log`, with the launcher settings of `main` (PR #172: no DRED, loop limit 256, `VICTORY_LATE_WAIT=3`, `VICTORY_WIDE_BACK=1`, card buffers) and **without** `VICTORY_RELAY`:

- Startup: no flicker. Observed.
- World after 2 min: 5.4 fps and 6,449 draws per frame (reference `main`: about 5.5 fps, about 4,410 draws; batch 4 with `VICTORY_RELAY=2`: 6.6 fps, 5,986 draws); 363 draws lost without a pipeline; 9 untranslated shaders (13 in batch 4); 16 pipelines refused by the driver (6 in batch 4); no device loss in about 2 min 30. Measured.
- Maintainer's opinion: "much better", the best lighting and graphics so far. Graphic problems remain on Lucia's face.

`VICTORY_RELAY` is no longer set by the launcher: the run judged best had it off, and the fork's issue #89 reports that it makes the characters' skin disappear and overlaps the subtitles. Known limits: ray tracing absent, shaders still untranslated, long-run stability not measured (our run lasted about 2 min 30). Progress of 24 November, from the VIctory logs: `d279e69` about 4 fps (13 min in the prison, then device lost) → `7b0f736` + `VICTORY_RELAY=2` about 4.5 fps → settings of PR #172 about 5.5 fps → batch 4 about 6.6 fps with `VICTORY_RELAY=2`, startup flicker fixed → this branch about 5.4 fps with more draws and the best image so far. Authors are credited in [THIRD_PARTY.md](../THIRD_PARTY.md).

## Previous pin: fork `7b0f736` (24 November, night)

`third_party/victory` points to `fxceless/victory-fork` at commit `7b0f736` ("Shader tests: recognise the software adapter from the device's own adapter"). It was first pinned at `d279e69` ("Make host pipeline waits configurable and prewarm pending requests (#1)", contributed by ZazaJr24); `7b0f736` adds `e9765a7` ("Fix the device loss introduced by the wave64 update") and `004bf69`. Newer commits, from `1684989` on, are not taken: the maintainer judged them a regression. The fork is not a GitHub fork attached to VIctory; its licence is GPL-3.0, compatible with AnyXboxX (GPL-3.0 or later). It replaces our former `anyxboxx/stable` integration.

Measured by the maintainer on 2026-11-24 (i7-13700KF + RTX 4070 SUPER, Windows 11) by running our own copy of the 1.0.2.1 build, from the VIctory log of the run (frame rate and draw counters as the program prints them), against our former `last` integration:

| | fork `d279e69` (first pin) | former `last` integration |
| --- | --- | --- |
| Frame rate in the world | 4.0 to 4.3 fps | 2.3 fps |
| Draws per frame | about 4,580 | about 2,860 |
| Draws lost without a pipeline | 428 | 400 |
| Indirect draws lost | 3 | 86 |
| Device lost | none for about 13 minutes, then lost (see known limits) | not recorded |

The world is reached and displayed. Nothing here comes from reading the game's code.

**Observed by the maintainer (Windows 11, RTX 4070 Super).** With the `fxceless/victory-fork` fork at commit `d279e69` and the launcher settings, the game was played at about 4 fps for about 13 minutes in the starting area (the prison): the character moves under the player's control and interacts with the first NPCs, then the host device is lost (known limit).

**Measured on `7b0f736` (maintainer, same machine, from the VIctory log of the run `victory-fork-7b0-relay.log`).** With `VICTORY_RELAY=2` and the launcher settings, pipeline cache already filled: world reached in about 1 min 25 s; 3.5 fps on the first frame of the world, then 4.5 fps after 2 min, about 4,975 draws per frame, 346 draws lost without a host pipeline; no device loss during about 2 min 40 s in the world, after which the maintainer closed the window. An earlier run of `7b0f736` without `VICTORY_RELAY`: 5.4 fps at about 3 min, no loss, window closed. The maintainer's opinion: "slightly better" than `d279e69`. Reference `d279e69`: 4.0 to 4.3 fps, about 4,580 draws, about 430 lost.

**Known limits (stability).** The device loss after about 13 minutes was observed on `d279e69`; the rest of this paragraph describes that run. `7b0f736` fixes a device loss tied to wave64; its long-run stability is not yet measured by us (our longest run on it is about 3 minutes). The fork's issue #26 reports 21.5 minutes stable on an RTX 4060 Ti with `VICTORY_RELAY=2` (third-party report, not reproduced here). The second run on `d279e69` held about 13 minutes in the world (first draws at 19:07:27 UTC), then lost the GPU device at 19:20:35 UTC: "host GPU device removed after 15 submissions sent ahead (VICTORY_LATE_WAIT=1) of 82 lists", the last list being a compute shader (`c1488f485ad606fb`). Measured, from the VIctory log of the run; the cause is not known. Plan on a few minutes to about 13 minutes in the world before a device loss, and relaunch. The message names `VICTORY_LATE_WAIT=1` while the launcher sets `3`: the setting actually in force in that run is to be checked. Checked since (read statically in `crates/victory-gpu/src/host.rs`, `late()`, same code at `d279e69` and `7b0f736`): `3` was in force. Stage 3 includes stages 1 and 2, and the `(VICTORY_LATE_WAIT=1)` and `(VICTORY_LATE_WAIT=2)` tags in log lines name the stage a counter belongs to, not the value set.

**First cold launch.** The first launch with an empty cache crashed at loading (pipeline compilation, then the game's internal crash tracked as VIctory #1464). The second launch, with the cache filled, held. If the first launch crashes, launch again.

The settings of the successful run are now the defaults of `tools/lancer_victory.ps1` (table in the Run section), except five variables that the pinned source never reads and DRED, which is now opt-in (see Run). They need the `diag` build (`--profile diag --features diagnostics`). Pass `-NoTunedSettings` to turn them off.

## What you get today

Measured on 2026-11-24 on an i7-13700KF + RTX 4070 SUPER, Windows 11, with the stable integration:

- Boot, profile screen, first-run settings, legal screens, loading and the pause menu work, at 30 fps in the menus.
- The first cutscene (opening credits of mission 1) is reached.
- The 3D scene behind the credits is black.
- In the world the game runs at 0.3 to 0.6 frames per second.
- It is not playable.

The section below describes a more advanced state, measured the same afternoon on a local integration branch that is not yet what `third_party/victory` pins.

## State on 24 November (afternoon)

Measured by running our own copy of the 1.0.2.1 build under VIctory, on a local integration branch (VIctory `main` + about 40 pull requests + local fixes not yet published), i7-13700KF + RTX 4070 SUPER. Nothing here comes from reading the game's code: these are run measurements (fps, timings, counts) and the output of `victory translate` on the shaders of our own copy.

- **World reached, character controllable (observed).** The tutorial "Move / Move the camera" (L / R) is displayed. The interface is composed over a 3D image that is still black.
- **Speed in the world (measured): 0.2 fps, then 2.3 to 2.5 fps.** Causes and fixes:
  - `VICTORY_D3D12_DRED=1` cost about 2.5x (text formatting and a `VirtualQuery` per draw). Only turn it on to investigate a device removed.
  - The copy back to the title spent 66% of the submission thread in `WriteProcessMemory`, itself in `NtQueryVirtualMemory` (about 2 ms per call). A direct `NtWriteVirtualMemory` brings it from 1.5 s to 0.13 s per frame.
  - Copies no longer call `VirtualQuery` first; the texture snapshot, comparison and hashing are spread over a thread pool.
  - What remains per frame (about 410 ms): GPU about 145 ms, copy back about 73 ms, hashing 20 to 60 ms, snapshot about 50 ms.
- **Untranslated shaders in game: 456 in the morning, about 170 in the afternoon (measured by the in-game count).** On the game's corpus (26,084 programs, `victory translate`) the emitted programs are: VS/PS 13,480 of 14,038, CS 2,212, MS 8,654 of 8,765 (counts as printed by the tool). Patterns solved during the day: descriptor read as data, scalar memory read as data, buffer of unknown stride, wave prefix in divergent flow (the InstanceCulling passes), mask kept in a VGPR lane, `readfirstlane` of a uniform address (ColorDownsample). What remains is mostly lane masks: "lane mask read as data" (586 programs) and "holding lanes elsewhere" (282) on the corpus.
- **Ray tracing.** The title's BVH builder causes a TDR (GPU timeout recovery); it is skipped with `VICTORY_SKIP_BVH_BUILDER=1`. The new local diagnostic option `VICTORY_RT_MISS=1` makes every ray miss: 36 of 47 ray-tracing containers are emitted instead of 18. A Codex analysis found that `ConstructBVH4` contains a bounded polling loop (256 iterations) on a value another pass produces.
- **Final composition.** As in VIctory #841, the scene exists at 2560x1440 but the presented target stays zero.

### Useful variables

All are diagnostics: they need the `diag` build (the `diagnostics` feature). Provenance, checked in the code of the integration worktree and against VIctory `main` on 2026-11-24:

| Variable | Effect | Where it comes from |
| --- | --- | --- |
| `VICTORY_KEYS` | Scripted key presses (see below). | VIctory `main` |
| `VICTORY_SHADER_LOOP_LIMIT=32` | Translated loops stop after that many iterations (decimal or `0x` hex, at least 1). | VIctory `main` |
| `VICTORY_D3D12_DRED=1` | Device-removed data. **Warning: any value turns it on, and it slows the world by about 2.5x; use it only to investigate a device removed.** | VIctory `main` |
| `VICTORY_SHOW_TARGET=busiest` | Shows the busiest render target instead of the final image. | Open pull request #638 (`Marvisak/VIctory`), in our integration |
| `VICTORY_CAPTURE_BUSY=<draws>[,<count>]` | Captures frames busier than that many draws (2 frames by default). | Open pull request #638 |
| `VICTORY_PRODUCERS=1` | Logs a pass that reads what only dropped (untranslated or skipped) work writes, with the dropped shader. | Open pull request #638 |
| `VICTORY_CAPPED_DYNAMIC=262144` | More dynamic descriptor slots on a capped heap (NVIDIA), at the cost of the title's highest slots. | Local experiment, published only on our fork (`anyxboxx/essai`, `anyxboxx/stable`) |
| `VICTORY_SKIP_BVH_BUILDER=1` | Leaves out the shaders of the title's BVH builder. | Former branch `anyxboxx/essai` only |
| `VICTORY_PROFILE=<ms>` | Sampling profiler of the whole process; logs the busiest call stacks every 5 s (sampling period in ms, 4 by default). | Local, not published |
| `VICTORY_RT_MISS=1` | Every ray misses (ray-tracing diagnostic, off by default). | Local, not published |

## State on 24 November (evening)

Measured by running our own copy of the 1.0.2.1 build under VIctory, on the local integration branch (VIctory `main` + pull requests + local fixes not all published), i7-13700KF + RTX 4070 SUPER. These are run measurements (fps, counts, traces) and the output of `victory translate`; nothing comes from reading the game's code.

- **Series S mode is faster (measured).** `victory run --console series-s ...` gives about 3.1 to 3.3 fps in the world, against about 2.0 to 2.4 fps in Series X mode (about +50%), with the same image symptom. The game then renders internally at 1280x720 and presents at 1920x1080. Recommended for iterating.
- **Black image diagnosis (measured).** The geometry exists in the G-buffer: the characters of the cutscene are visible in 1440p and 720p. The final colour is a uniform tint that darkens over time (exposure), with the subtitles drawn on top. Almost all deferred lighting passes are translated and bound. A per-pass trace (local option `VICTORY_INDIRECT_TRACE`, not published) shows that about 379 mesh `ExecuteIndirect` calls start with zero arguments, because the GPU passes that produce those arguments are not translated yet: the translator refuses them with "an address from a phi" and "a lane mask holding lanes elsewhere read as data". This is the current work item.
- **Translation (measured).** Untranslated shaders in game: about 130 to 170 (in-game count). On the corpus (26,084 programs, `victory translate`): VS/PS 13,780 emitted of 14,038, CS 2,212, MS 8,654 of 8,765 (counts as printed by the tool).
- **Fixes sent to `Marvisak/VIctory`**: #1215 (speed: direct `NtWriteVirtualMemory` write, copies without `VirtualQuery`, profiler), #1225 (`readfirstlane` of a uniform address: ColorDownsample), #1231 (uniform loop counter: light probes). Measurement comments were also added on #497, #320, #1074, #1198 and #841.
- **Community.** The project has a Discord server (link at the top of the README).

Variable added to the table above (diagnostic, `diag` build):

| Variable | Effect | Where it comes from |
| --- | --- | --- |
| `VICTORY_INDIRECT_TRACE` | Per-pass trace of `ExecuteIndirect` calls and their arguments. | Local, not published |

## Requirements

- Windows 10 or 11, x64.
- A CPU with AVX2. Intel is fine: the instructions specific to AMD are emulated.
- A Direct3D 12 GPU with shader model 6.6 and mesh shaders (NVIDIA RTX 20 series or newer, AMD RDNA 2 or newer).
- 32 GB of RAM recommended, and a large enough Windows paging file: the game commits several GiB at startup. If it does not, `crashes.log` reports Windows error 1455: close other applications or enlarge the paging file.
- Your own copy of the Xbox Series 1.0.2.1 build: a folder that contains `game_scarlett_master.exe`. Not provided, not explained here.
- Free disk space for the shader cache (it grows as you play; see the status page for the figures to plan for).

## Install

1. [Git](https://git-scm.com/).
2. Rust through [rustup](https://rustup.rs/). The exact toolchain is pinned by `third_party/victory/rust-toolchain.toml` and is installed automatically on the first `cargo` command.
3. Visual Studio 2022 or 2026 Build Tools with the "Desktop development with C++" workload (MSVC).
4. Windows SDK 10.0.26100 (provides `dxc`).
5. Optional: the Windows optional feature "Graphics Tools" (D3D12 debug layer, needed only for `VICTORY_D3D12_DEBUG`).
6. Python 3.12, only for the AnyXboxX test suite (see Tests).

Get the code (VIctory is included):

```powershell
git clone https://github.com/LooperSalty/AnyXboxX.git
cd AnyXboxX
```

## Build

```powershell
cargo build --profile diag -p victory --features diagnostics --manifest-path third_party/victory/Cargo.toml
```

The `diag` profile is required for the scripted keys, the frame captures and the diagnostic options below. The first build downloads and compiles all dependencies, so expect several minutes. The executable is `third_party\victory\target\diag\victory.exe` (or `%CARGO_TARGET_DIR%\diag\victory.exe` if you set that variable). `tools/lancer_victory.ps1` runs this build for you at every launch: cargo does nothing when the executable is current (about 0.1 s) and rebuilds it after a submodule update, so an old `victory.exe` never runs with settings it does not read. If the build fails, the launcher stops and reports cargo's exit code.

## Run

```powershell
.\tools\lancer_victory.ps1 -GameDir <game folder>
```

The script sets the variables below (unless you pass `-NoTunedSettings`, which keeps only `VICTORY_GAME_DIR`) and then runs `victory.exe run <game folder>\game_scarlett_master.exe --execute`:

| Variable | Value | Why |
| --- | --- | --- |
| `VICTORY_GAME_DIR` | your game folder | No `victory.toml` is needed. |
| `VICTORY_SHADER_LOOP_LIMIT` | `256` | Translated shader loops were almost endless, so they are cut. At 32 (the former value) hair is drawn as glowing strands and the scene is darker; at 1024 the device was lost on an RTX 3060 Ti with the default 2-second timeout; 256 held (fork issue #18, and observed on Linux in runs 3 and 4 below). |
| `VICTORY_LATE_WAIT` | `3` | Host pipeline wait setting of the fork's pull request #1. |
| `VICTORY_WIDE_BACK` | `1` | Fork option, on in the measured run. |
| `VICTORY_RELAY` | not set | Fork option, no longer set by default (fork issue #89: skin disappears, subtitles overlap); `2` was on in the measured runs of `7b0f736` and batch 4. |
| `VICTORY_CARD_BUFFERS_MIN_MB`, `VICTORY_CARD_BUFFERS_MB` | `1`, `1024` | Card buffer sizing, in MB. |
| `VICTORY_DSTORAGE_NOTIFY` | `1` | DirectStorage writes into physical pages are reported to the GPU side (fork commits `e49864a` and `bf870de` by Yorivel, fork issue #62): without it the first room's walls can read stale copies and look transparent. |
| `VICTORY_STAGING_POOL_MB` | `1024` | Staging buffers of the textures picked by index are kept for reuse, up to that many MB, instead of a new upload buffer made and mapped at every upload (`0` keeps none). Measured on an RTX 5080 (driver 621.01, VIctory logs, two runs each way, 25 November): at world entry the stretch below 2.5 fps lasts about 40 s instead of about 80 s, and the texture pass of the first 100 s takes 190 ms a frame instead of 560 to 730 ms; nothing changes once the queue is empty (3.4 to 3.5 fps either way). |

The former defaults `VICTORY_RT_MISS`, `VICTORY_WAVE32_READFIRSTLANE`, `VICTORY_WAVE32_CROSSLANE`, `VICTORY_SKIP_BVH_BUILDER` and `VICTORY_CAPPED_DYNAMIC` are gone: no file of the pinned commit reads them (the BVH builder is off unless `VICTORY_BVH_BUILDER=1`). `tests/test_lanceur_victory.py` now checks that VIctory reads every variable the launcher sets.

These values are unchanged since the 57 Proton and AMD commits: they are the maintainer's Windows settings, not yet measured with those commits. On Linux with RADV, `VICTORY_LATE_WAIT=3` loses the device (fork issue #184); the settings of the measured Linux run are in the Proton and AMD fixes section above.

`-Dred` sets `VICTORY_D3D12_DRED=1` (device-removed data) to diagnose a device loss. It is off by default: VIctory turns it on whenever the variable exists, and it notes every command list operation, which slows each frame.

`-XgDll <path to xg_xs.dll>` sets `VICTORY_XG_DLL`: your own copy of the GDK's offline PC texture layout library lays out the title's textures (see the batch 8 and texture layout library section above). `-XgMode compare` (`VICTORY_XG_MODE`) keeps VIctory's layouts and logs every difference with the library instead; `real` is the default. Never copy the DLL into this repository.

`-Precompile` first runs `victory.exe translate --cache --pipelines --used-pipelines <game folder>\shaders.rpf <game folder>\game_scarlett_master.exe <game folder>\XboxBvhBuilder.dll <game folder>\amd_fidelityfx_upscaler_xbox.dll` with the same environment variables as the game (the cache key depends on them), then starts the game. On the maintainer's machine this took about 4.5 min and filled 13 GB of cache (measured); later launches reuse it. It fails with a clear message if one of those files is missing.

The first cold launch may crash while the pipelines compile: launch again once the cache is filled.

`-SkipPipelines <hash,...>` is passed to `VICTORY_SKIP_PIPELINES` (see below).

Where things are kept:

- Saves, settings and `crashes.log`: `%LOCALAPPDATA%\victory\data`.
- Derived data (shader and pipeline caches): `%LOCALAPPDATA%\victory\cache`.

Both are outside the repository on purpose. Delete the cache folder to start from a clean cache; keep the data folder if you want to keep your save.

## Getting to the first cutscene

The window caption shows the frame rate. The window must have the focus.

1. Profile screen: press Enter.
2. First-run settings: press Enter on each screen.
3. Legal notices: move down to "Next", wait for the countdown of the photosensitivity warning to end, tick the two boxes, then "Confirm".
4. Loading, then the first cutscene (opening credits).
5. Escape opens the pause menu.

Keyboard or pad. With no XInput pad connected, the keyboard acts as a pad while the title's window has focus:

| Keys | Pad |
| --- | --- |
| Enter, Backspace, X, Y | A, B, X, Y |
| Arrows | D-pad |
| W A S D / I J K L | left stick / right stick |
| Q / E, Z / C | shoulders, triggers |
| F / H | stick clicks |
| Escape | Menu |
| Tab | View |

Hold each key for a moment: the game reads its input at 30 Hz, so a very short tap can be missed. Later launches resume directly from the save.

## Scripted runs and diagnostics

These need the `diag` build.

| Variable | Effect |
| --- | --- |
| `VICTORY_KEYS=40:Enter,55:Enter,70:Enter` | Presses the keys at those seconds, with or without focus. |
| `VICTORY_CAPTURE=<folder>` and `VICTORY_CAPTURE_FRAMES=<list>` | Saves the presented frames (those frame numbers). |
| `VICTORY_SHOW_TARGET=busiest` | Shows the busiest render target instead of the final image (for example the G-buffer). |
| `VICTORY_D3D12_DRED=1` | On a GPU failure, reports the commands in flight. It slows the world by about 2.5x: turn it on only to investigate a device removed. |
| `VICTORY_D3D12_DEBUG=1` | D3D12 debug layer (Graphics Tools required). |
| `VICTORY_GPU_TIMING=1` | Times each command list and logs the slow ones. |
| `VICTORY_SKIP_PIPELINES=<hash,...>` | Omits the draws and dispatches that use the listed 16-digit shader hashes. |
| `VICTORY_LOG=<target>=debug,...` | Debug and trace output. |

To stop the game: `taskkill /F /IM victory.exe`. The `timeout` command of Git Bash does not kill the executable.

To measure a run, send VIctory's log (stderr) to a file, for example `.\tools\lancer_victory.ps1 -GameDir <folder> 2> victory.log`, then summarise it with `python tools/resume_journal_victory.py victory.log`. The script keeps the frame lines of the world (100 draws per frame or more, `--draws-min` changes the threshold) and gives the median and range of each part of the frame (preparing, copies, textures, GPU, copying back, draws, draws dropped without a host pipeline), plus the count of warnings (shaders that do not translate, refused pipelines, device removed). With two logs (`before.log after.log`) it compares the medians column by column, so a change is judged on the part of the frame it targets. `--markdown` prints tables to paste into an issue. Only counts and times are printed.

## Former branches

The branches `anyxboxx/stable` and `anyxboxx/essai` of `LooperSalty/victory` are no longer the pin: the fork above replaces them. The sections above that mention them describe earlier measurements.

## Safety

- On NVIDIA, staying in the world for a long time can reset the display driver (every monitor goes black for a moment), and one tester got a blue screen (0x119). Stop the game after a few minutes in the world.
- Run one instance at a time.

## Tests

```powershell
cargo test -p victory-gpu -p victory-shader --manifest-path third_party/victory/Cargo.toml
```

AnyXboxX itself (see the Setup section of [AGENTS.md](../AGENTS.md)):

```powershell
py -3.12 -m venv .venv
.\.venv\Scripts\python.exe -m pip install -r requirements.txt -r requirements-dev.txt
.\.venv\Scripts\python.exe -m pytest
.\.venv\Scripts\python.exe -m ruff check --select W605,F,E9 PrototypeScarlett tests tools
```

## Where to report

Open an issue on [AnyXboxX](https://github.com/LooperSalty/AnyXboxX/issues) or on [VIctory](https://github.com/Marvisak/VIctory/issues) (issue #264 collects the NVIDIA GPU hangs). Describe the symptom, your GPU and driver, and the relevant lines of `crashes.log`. Never attach a game file.
