# Flamingo (VIctory-family base)

The working log of Flamingo: every change on top of victory-fork, and every measured run (Series S
profile, RTX 4070). Scratch commit ids and local paths in it are from the machine it was made on.

This tree is fxceless/victory-fork (its main at `c96b4f2`: VIctory `ffe310e` plus the fork's
world-rendering and speed work). Its author tested it on an RTX 4070 like the one this build targets. The first Flamingo was
based on XCL (`E:\Games\Emulators\Flamingo`). On NVIDIA it stalled at the world entry and needed
about an hour of pipeline compiles after every translator change. This base renders the world
instead and compiles shaders on first use. The XCL work carries over here as ideas only: XCL is
GPL-2.0 and VIctory is GPL-3.0, so code is not copied between them.

## On top of victory-fork

| Change | Source |
|--------|--------|
| `VICTORY_NO_WATCHDOG=1` hooks out the title's own watchdog, which ended runs during cold shader compiles | upstream VIctory PR #454 (cherry-picked) |
| The window caption says Flamingo | Flamingo |
| Cheaper indirect draws: the clamped-arguments buffer stays readable (no barrier per draw), one budget charge per thread group | victory-fork PR #220 (working tree) |
| Copy-back reads scratch sized to the changed stretch; bounds scanned 32 bytes at a time | victory-fork PRs #240, #255 (working tree) |
| The card-memory hash passes use a group per piece | victory-fork PR #253 (working tree) |
| Queue and frame-wait fixes; sent-ahead submissions finish before detiling into their staging | victory-fork PRs #216, #281, #272 (working tree) |
| Intro and menu flicker: a guarded texture whose memory changed in the last 90 frames is hashed at every check, not every 8th frame. The title writes its intro video and menu animation frames through views the guard does not see, so they showed stale pictures between fresh ones (victory-fork #269) | Flamingo (`victory-gpu/src/texture.rs`) |
| Discord Rich Presence: the title's name, menus or world, the frame rate, the time played; looked at every second, sent when it changes (at most every 4 s, Discord's limit) | Flamingo (`apps/victory/src/discord.rs`) |
| Structure builder: its last three programs translate on 32-lane GPUs | victory-fork PR #294 (working tree) |
| Wave32 shared-memory synchronisation for the visibility dispatch; merged-stage shared memory size and GS vertex bound; lane masks through joins; lane selectors wrapped at the console's wave width; wave32 pixel and vertex wave operations per 32-lane half; literal select masks in local half-waves | victory-fork PRs #368, #367, #377, #298, #318, #277 (working tree) |
| FP16 interpolation and integer render-target pipelines; signed wave prefix and integer NaN checks in f16 conversions; refused shaders' containers saved (`VICTORY_DUMP_REFUSED`) | victory-fork PRs #280, #355, #266 (working tree) |
| Geometry stages: host pipelines with a geometry stage made from the console's merged ES+GS pair (stacked on #381, #373, #367, #309; replaces #367 here) | victory-fork PR #400 (working tree) |
| Mesh shaders whose 16-bit outputs pass D3D12's 32 KiB are linked with their pixel shader and packed (71 mesh shaders) | victory-fork PR #397 (working tree; merged by hand with #400 and #368) |
| Series S: multi-plane present compositor, R9G9B9E5 target stand-in, streaming budgets | victory-fork PR #423 (working tree) |
| The title's FidelityFX upscaler programs translate: their containers hold an `ACCC` part with an `FSPX` image instead of `DXIL`. Series S renders the world at 1280x720 and upscales it with them; without them its picture was black | victory-fork PR #258 (working tree; merged with #298's lane selectors) |
| Intro movie judder: a present with no flip wait since the last one is held to its flip tick | victory-fork PR #457 (merged with #281's frame wait loop) |
| Integer render targets get no blending (the mesh pipeline `f5e50eb2108c425b` NVIDIA refused); the heap buffer pass looks only at marked slots | victory-fork PRs #501, #467 |
| Ray tracing on 32-lane GPUs: shadow traces and the full structure builder translate (`VICTORY_HOST_WAVE=32`), an absent child is a miss, 46 more programs | victory-fork PR #487 (bundle of #156, #213, #203; merged) |
| The repository's asset-policy size limit is 1 MiB (the merged shader PRs grew `emit/lower.rs` past 512 KiB) | Flamingo (`xtask/src/policy.rs`) |
| Diagnostics: `VICTORY_PARTS=1` (translate lists each container's parts), `FLAMINGO_SAVE_FAILED=<dir>` (shaders that do not translate, saved as the title gave them) | Flamingo |
| Speed and safety: AVX2 relay merge, texture refreshes do not wait for remaps, pipeline wakeups isolated, BVH sorted hits, watch monotonicity, detile bounds, descriptor and DirectStorage reference fixes, subobject bounds | victory-fork PRs #384, #393, #398, #414, #410, #408, #405, #403, #409 (working tree) |

How this tree is put together: the scratch branch `flamingo-wip` (a worktree in the session's
scratch folder) holds one local commit per PR, applied with three-way merges in the order above,
plus Flamingo's own edits and the fields the stacked PRs need in each other's code. Its files are
copied into this tree as uncommitted changes. Since run 32 the branch is rebased onto fork main
`c96b4f2`. Whole suite on that tree: 606 passed, 0 failed, 10 ignored.

## Run recipe (RTX 4070, Series X profile)

Build: `cargo build --profile diag -p victory --features diagnostics`. Only diagnostics builds
have the scripted keys (`VICTORY_KEYS`) and the frame captures.

```
VICTORY_CONFIG=E:/Games/Emulators/VIctory/victory.toml
VICTORY_NO_WATCHDOG=1
VICTORY_PIPELINE_WAIT_MS=24          # a draw waits at most 24 ms for its pipeline, then drops
VICTORY_RELAY=1 VICTORY_LATE_WAIT=3  # the fork's fast path (its PR #234); relay stage 1: =2 loses character skin (victory-fork #89)
VICTORY_CARD_BUFFERS_MIN_MB=1 VICTORY_CARD_BUFFERS_MB=1024
VICTORY_INDIRECT_WINDOW=1           # indirect records clamped a window at a time (about 2x less GPU time)
VICTORY_SHADER_OPT=0                 # victory-fork #310: the shader clean-up passes draw black blocks
VICTORY_WIDE_MERGE=1                 # fetched pieces merged on several threads (with fork #665)
target\diag\victory.exe run <your dump>\<the title's executable> --execute
```

`E:\Games\Emulators\Play Flamingo.bat` starts the title with these settings (no scripted keys: play with a pad or the
keyboard).

The title cannot reach the network under VIctory: it runs offline by design.

**Caution:** one RTX 5080 tester blue-screened (0x119) after 22 minutes with the relay. That PC had
TdrDelay raised to 60 s. Here TdrDelay is the default 2 s. Keep test runs short.

## Results on this PC (RTX 4070, Ryzen 7 7800X3D, 32 GB)

| Run | Settings | World |
|-----|----------|-------|
| 2 | cold shader cache, defaults | prison prologue renders with HUD and gameplay prompts; 0.8-2.5 s per frame while shaders compile, then a steady 2.7 fps (370 ms: 99 preparing, 213 on the GPU, 52 copying back) |
| 3 | warm cache, fast path (relay, late wait, card buffers) | steady 4.5-4.8 fps (209-221 ms) in gameplay at about 5,000 draws per frame; the GPU is the limit now (127 ms per frame, 97% busy), and the card idles 59 ms per frame waiting on us |
| 4 | as 3, plus `VICTORY_DISPLAY_MODE=1920x1080` and `VICTORY_WIDE_BACK=1 VICTORY_WIDE_REFRESH=1` | slower: 3.6-4 fps (223-274 ms). The game still renders 4K targets, so the GPU time is unchanged; the multi-threaded copies cost more than they save here. Not used further |
| 5 | as 3, on the build with the six victory-fork PRs | about 5 fps in gameplay (5.3, 4.8, 5.1, 5.2, 4.8: 190-209 ms) at ~4,950 draws; the GPU time fell from ~127 to 98-114 ms a frame; the card is 58% busy and waits 48 ms a frame for our submitting thread |
| 7 | as 5, plus `VICTORY_INDIRECT_WINDOW=1` (one clamp pass per window of indirect records, no barrier per draw with PR #220) | **7.0-7.7 fps (129-158 ms)** in gameplay at ~5,000 draws; GPU time 35-52 ms a frame (was ~100); same dropped-draw counts as run 5. The limit is now the CPU's copying: preparing ~80 ms, copy-back ~106 ms a frame (relay thread) |
| 8 | as 7, plus `VICTORY_INPLACE=1` and its census | 5.6-6.9 fps: no gain (stage 1 placed one 4 MB window). The census: ~1,040 MB of title buffers have host copies, ~300 MB of them private memory that could be placed now, ~420 MB in sections made `SEC_RESERVE` (refused as heaps until made `SEC_COMMIT`). The captures show black blocks (a big black rectangle over the cell window at world frame +600) |
| 9 | as 7, plus `VICTORY_SHADER_OPT=0`, the flicker fix and the Discord module (cold cache for the new shaders) | the black blocks are gone at the same world frames; black screen while ~1,000 pipelines compiled, then **6.9-7.2 fps (140-150 ms)** at ~4,750 draws. Per frame the relay thread waits ~75 ms for the card to send back the changed pieces of card copies (the "copying back") |
| 10 | as 9 (warm), plus `VICTORY_WIDE_BACK=1` alone | 5.2-6.7 fps (150-190 ms): copy-back up to 117 ms. Not used |
| 11 | as 9, plus `VICTORY_WIDE_MERGE=1 VICTORY_CARD_REHASH_RUNS=1` | 5.8-6.7 fps; copy-back 89-99 ms. No clear gain, not used |
| 12 | as 11, with `VICTORY_RELAY=1` | **characters render whole**: the player character's hair, skin and jumpsuit, the inmates in the yard (with `=2` their skin was missing: victory-fork #89); 7 fps at first, 4.3-6.8 fps later while Windows paged. In gameplay the process held 15-16.6 GB of RAM (24 GB committed); 3.3 GB stayed available, with 322 hard page-ins a second |
| 13 | the build with the ten PRs above, plus `VICTORY_BVH_BUILDER=1` | the structure builder ran (all 53 of its pipelines made) and the title crashed at the world load reading a garbage pointer: the builder's output is not right yet. Off |
| 15 | debug layer (`VICTORY_D3D12_DEBUG=1`) | all 4 refusals: "Shader wants wave lane count of 64 but maximum wave lane count for device is 32". `VICTORY_WAVE_WHY=1` with `translate`: LDS shared within a console wave in a group over 32 threads, read and written in one lane-branch region; needs two console lanes per host thread (XCL's split-wave64) |
| 18 | merged build (+ #400, #397, #423, stability PRs), Series X | 6.5-7.1 fps; same 4 refusals, 7 untranslated |
| 19 | the same, `--console series-s` | **10.6-11.7 fps** (86-94 ms; the world at 1280x720, GPU 15 ms), but the picture is black: 20 more shaders do not translate there, the FidelityFX upscaler (`ACCC`/`FSPX` containers) |
| 14 | the same build, builder off (the launcher's settings) | stable, **7.1-7.4 fps** (136-141 ms) in gameplay; 4 refused pipelines (was 5), 7 untranslated shaders (was 8) |
| 22 | Series S with fork #258 | **the picture is back, 9.3-10 fps** (105 ms): the player character, the cell window, the yard; 11 untranslated shaders (was 27) |
| 23-24 | + fork #448, #432 (and `VICTORY_LATE_WAIT_LISTS=256` in 23) | stalls of 1-8 s a frame (0.1-1.6 fps): one of the two PRs; both dropped |
| 25 | Series S, + #501, #467 | steady 8.5-10.4 fps (102 ms): preparing 68 ms, copy-back 62 ms, GPU 15 ms |
| 26 | + #457, #487 (RT bundle), `VICTORY_BVH_BUILDER=1 VICTORY_HOST_WAVE=32` | the builder runs and the title crashes at `+0x27c195d` as in run 13: off |
| 27-28 | builder off; + #513 | 9.6-9.9 fps; untranslated 8 |
| 29-30 | Flamingo's cold write-watch skip (`FLAMINGO_COLD_ASKS`) | left out 168 of ~2,600 askings a frame, no measurable gain (an asking costs its call, 7 microseconds): off by default |
| 31 | + `VICTORY_WINDOW_KB=1024 VICTORY_WINDOW_TAILS=1 VICTORY_INPLACE=1` (fork #580) | 8.1-9.4 fps, 19.2 GB working set: not used |
| 32 | the whole stack rebased onto fork main `c96b4f2` (#397 left out: NVIDIA takes the big mesh outputs) | **9.3-10.3 fps** (97-107 ms); untranslated 4 (was 8); refused 6, all wanting 64-lane waves |
| 33 | + fork #589 (dropped heaps let go), #592 (a lane-branch region that only raises an LDS flag runs as two host waves) | 9.3-9.9 fps; refused 5 |
| 34 | the stack rebased onto fork main `310c6eb`, + #648 with `VICTORY_CARD_FETCH_INFLIGHT=1` | 0.6-2.7 fps, then the host GPU device was removed. Another tester saw 1.6 fps on `310c6eb` (fork #652). This tree went back to the run 33 one; experiments since build into `E:\Games\Emulators\Flamingo2-exp-target`, so the launcher's build stays the good one |
| 35 | run 33's tree + #648 with `VICTORY_CARD_FETCH_INFLIGHT=1` (experiment build) | host GPU device removed ("no host list for a state prelude"): #648 is not used |
| 36 | run 33's tree + fork #670 (no kernel write-watch asking for memory the guard watches) | 8.9-10.3 fps; write-watch 15.6 ms a frame (was ~17.8): kept, kept |
| 37 | + fork #665 (relay fetches on a copy queue of their own; merges on several threads with `VICTORY_WIDE_MERGE=1`) | **10.4-11.3 fps** (89-96 ms); copy-back 28.6 ms a frame (was ~62), the relay's wait for fetches 4.9 ms (was 38-63): now the launcher's build |
| 38 | experiment: Flamingo's plain copies that recover from a fault (`watch::try_copy`, `FLAMINGO_PLAIN_COPIES=0` turns them off) in place of `ReadProcessMemory` for title copies | 9.9-11.8 fps; the submitting thread's kernel time 31.5 -> 16.7 ms, but its lock waits 15 -> 22 ms and its waits for the card 9.5 -> 18.6 ms took the time back; no faults reported |
| 39 | experiment: + the plain copies in `read()` and `write_mapped` (the relay's reads and writes of title memory) | 8.8-11.7 fps; copying back 27.7 ms (was 28.6): the relay's time is comparing and merging, not kernel calls |
| 40 | experiment: + a line for each fill of upload memory that waits for the card | the wait (17 ms, once a frame) is a 480-byte constant buffer of a fresh descriptor table inside the title's 174 MB buffer, which shaders in the air may write |
| 41 | experiment: + fresh tables' constant views read their buffer's host copy, brought up to date like any buffer view, when 256-byte aligned (`FLAMINGO_DIRECT_CBV=0` copies as before) | 10.5-12.0 fps (11.1 on average, was 10.7); fills waiting for the card 1.0 -> 0.0 a frame; picture right. Left: buffer copies waiting for the card (10.6 ms a frame) and 18 ms of submission lock waits |
| 42 | run 41's build + 200 stack dumps (`VICTORY_STACKS`), read with `stack_profile.py` (dbghelp) | the submitting thread: 30% in the title's own kernel waits, 15% refreshing host copies, 11% write-watch askings, 11% settles and present, 8% copies, 5.5% letting host textures go (the once-a-second sweep), 2.5% reading the clock. The relay thread is idle half the time |
| 43 | + retired objects dropped on a thread of their own (`FLAMINGO_DROP_THREAD=0` drops in place) + `run_deferred` reads the clock only for steps that use it | 10.4-12.0 fps (11.1 on average): no change on average |
| 44 | + a line for each title buffer copy that waits for the card | once a frame, a 20 KiB copy out of the 174 MB buffer (shaders may write it) to a fixed destination: about 18 ms of waiting for the card |
| 45 | experiment: such copies made on the card (`CopyBufferRegion` in the title's list, the destination declared written) | worse: 8.7 fps. Fills of upload memory then waited instead (3.8 a frame, 43 ms): one reads the copy's destination as a misaligned constant buffer. Taken out again |
| 46 | run 43's build + the stats overlay (`FLAMINGO_OSD`, owned by the game's window) | **10.6-12.1 fps (11.3 on average)**, picture right: now the launcher's build (scratch commit `7aafd29`, branch `flamingo-wip-46`) |
| 47 | timing run (`VICTORY_GPU_TIMING=1 VICTORY_D3D12_DRED=1`, relay and late wait off) | 6.1 fps; only 25-88 commands a frame carry notes, so the card's time by command says little. Per frame: 693 full waits for the title's barrier groups, 2,410 indirect commands each bounded by a pass of ours between two full waits |
| 48 | launcher build + `VICTORY_BARRIER_TEST=indirect` (timing only: the bounding pass without its two waits) | 10.82 fps, the card still about 65% busy: those waits are not what holds the frame |
| 49 | launcher build + `VICTORY_INPLACE=1` | 10.74 fps: only 81 windows (384 MB) went in place; 1,145 were turned down as view memory (the page pool, made with `SEC_RESERVE`) or not private memory committed whole. victory-fork #772's `VICTORY_INPLACE_POOL=1` is aimed at exactly that |
| 50 | launcher tree + `FLAMINGO_READ_SETTLE_TEST=1` (timing only: reads of shader-written memory do not wait) | 11.13 fps: the settles for reads are not what holds the frame either; the submitting thread then spent longer in the title's code between submissions |
| 51 | the launcher build + 200 stack dumps, every thread (`thread_busy.py`) | in the world: the submitting thread is busy 71% (write-watch askings and their journal 23%, copies 6%, the diagnostics' counting allocator 4.5%, hashing 5.5%), waiting 29% (24 of 32 waits in `Host::settle`); the five recording threads are busy 40-46%, nearly all of it in our list hooks (clock reads, SipHash, the counting allocator) |
| 52 | **a build without the diagnostics feature** (victory-hle's new `keys` feature keeps the scripted keys for runs) | **11.9-12.7 fps (12.3 on average)**, from 11.3: the counters, clock reads and counting allocator cost about 9%. Now the launcher's build (`cargo build --profile diag -p victory`, no features; scratch commit `e78c633`) |
| 53 | lean build with the shader clean-up passes on (no `VICTORY_SHADER_OPT=0`) | 9.6 fps (with a cold pipeline cache): kept off |
| 54 | lean build + victory-fork #799 (64 KiB constant view clamp, loose-LDS fallback, translator fixes) | 12.28 fps, refused 5 -> 2, but the world grainy and smeared where the three programs the loose fallback let run draw |
| 55 | #799 with the loose-LDS fallback opt-in (`VICTORY_WAVE_LDS_LOOSE=1`, keyed in the shader cache) | 11.96 fps, refused 5, picture clean |
| 56 | + victory-fork #812 (unmapped view blocks parked, not asked about at every check) | **12.6-13.2 fps (13.0 on average)**; preparing 44 ms (was 54), copies 16.9 ms (was 27). Now the launcher's build (scratch commit `52eca1a`) |
| 57 | run 56's build with `VICTORY_VIEW_SET=0` | 12.65 fps; the dark rectangles on the right wall are the same as in run 56: the scene's (papers on the wall), not #812's |
| 58 | diagnostics build of run 56's tree + 200 stack dumps | the submitting thread busy 54% (was 71%); its waits: `Host::settle` 25% of its time, the title's own 19% |
| 59, 60 | captures of the opening cutscene every 8 s; run 60 with the loose-LDS fallback on | the cutscene: faces and hair dithered, eyes white, much of it dark. With the loose fallback the cutscene got darker and some shots washed out: kept off. victory-amd's notes: the dark areas are the game's ray-traced bounce light and shadows, which do not run yet |
| 61 | + victory-fork #798 (Series S keeps the title's own upscaler, not FSR 4; two code patches by signature) | window 13.97 fps, turned 13.70 fps (from here: a turn run, `f2turn.sh`, fps per view); untranslated 4 -> 1 |
| 62 | run 61's build with `VICTORY_TITLE_PATCHES=0` (the comparison) | window 11.95 fps, turned 11.68 fps: #798 is worth 2 fps |
| 63 | + victory-fork #823 (the wave-wide form for four refused compute programs by their hash; the structure builder's last programs) | **window 14.14 fps, turned 13.05 fps**; refused 5 -> 2. Now the launcher's build |
| 64 | + `VICTORY_BVH_BUILDER=1` (all 53 builder pipelines exist since #823) | the title crashes 67 s in, as the world loads (reads of 0x8, 0x28 and of all-ones pointers): the builder's raw-address stores to pages not made ready are lost |
| 65 | `FLAMINGO_BVH_MISS=1`: every ray-tracing node test misses (builder off) | the cutscene much brighter, faces lit; the title hung 180 s in (frames stopped, no device loss): left off |
| 66 | ScarlettPort's stand-ins (`VICTORY_AMBIENT_STUB=1`, `VICTORY_SHADOWMASK_STUB=1`, ported, GPL-3.0) | the cutscene as dark as before, the player character's body gone in the world: left off |
| 67 | diagnostics build + stack dumps | the submitting thread waits 41% of its time in `Host::settle`: one 20 KiB title buffer copy a frame out of the shader-written 174 MB buffer, 23-34 ms a frame |
| 68 | `VICTORY_RELAY=2` (flights waited for by what they declared) | 13.37 / 12.56 fps: no gain |
| 69 | `FLAMINGO_CARD_FILLS=1`: that copy and the misaligned constants of shader-written memory copied on the card | 6.86 / 7.85 fps, GPU wait 57-61 ms: a copy and two barriers per draw cost the card more than the wait: left off |
| 70 | the launcher build, the camera held right for 18 s (fully round, to the cell door) | window 13.4 fps; turned 8.6-12.3 fps with the card 80-85% busy (GPU-bound) and 4,700-6,200 draws; while turning 3.7-3.9 fps as texture checks reach 33 ms and 127 MB a frame |
| 71, 72 | diagnostics build with `VICTORY_BARRIER_TEST=both`, and without | window 13.1 vs 12.4 fps: the waits for every shader write cost about 5% |
| 73 | + victory-fork a26859f (byte-copied descriptor records mirrored), without 8dd30ab's view guard | 7.8 fps: the pass looks at 320,000 slots a frame. Kept on a scratch branch, not used |
| 74-76 | + DualSense support, + #768 (`VICTORY_WIDE_TEXTURES=1`) | a connected DualSense becomes pad 1 and the scripted turn does nothing: test runs set `FLAMINGO_DUALSENSE=0`. With #768: window 13.65 fps, no dip below 11.7 fps while turning. Both now in the launcher |
| 77 | + victory-amd bddfb4a (a texture with only its smaller mips detiles from its start), 4d64efe (DirectStorage page arrays read when a request runs) and c6dc4d8 (a write through one view of a page seen at its other views) | the title stopped at its own breakpoint in the opening cutscene (0x80000003) |
| 78 | as 77, `VICTORY_VIEW_GUARD=0` | no stop; window 13.65 fps, turned 13.08. The cell's wall and the cutscene's lamps show where run 76 had black. c6dc4d8's guard is now off by default (`VICTORY_VIEW_GUARD=1` on) |
| 79 | + Belobug's ambient pass, `VICTORY_AMBIENT=0.6`: the indirect-light resolve's output replaced by a constant | worse: the cutscene washed out grey, and the player's body did not draw. Off by default |
| 80, 81 | + Belobug 03629f3, `FLAMINGO_QUAD_HELPERS=1`: in pixel shaders, reads of the quad's other lanes reach helper lanes | the cell window's glass shows its dirt and reflections, faces look smoother. Run 80 translated every shader again (12.65 / 12.15 fps); run 81 is the warm rerun |
| 82, 83 | quad helper reads in one quad operation for fixed patterns (across X, Y, the diagonal, one lane for all) | the cell window's wire grate draws sharp, hair shows more detail; warm (83): 11.2 fps at the window against 13.65 without (run 78), about 18% for thin geometry sampling its full-detail levels. Stays opt-in |
| 84 | merged victory-fork main (f85101b: a submit thread, a copy queue for fetches, safer indirect windows, the first room's descriptors mirrored, the shader cache keyed on the translator instead of the executable), victory-fork #847's test change, victory-amd bddfb4a and 4d64efe again on top, Belobug's opt-ins | the grate draws sharp with no quad change (the fork's a26859f); 9.57 fps at the window, 9.38 turned, cold cache: preparing 41 to 58 ms, copy-back 23 to 35 ms. The fork's view guard is on by default here |
| 85 | + the mesh primitive edge-flag fix (pull request #1 to this repository, from upstream VIctory #1718): a corner's index is 9 bits, the 10th is its edge flag | the pipeline cache had been cleared before it: every pipeline was made again during the run, 1,113 still waiting in the world, and the picture stayed black (not the fix) |
| 86 | a cutscene run: Enter only through the menus (to 74 s), a capture every 10 s | the opening cutscene plays from about 85 s to 145 s, then the cell; its first outdoor shot is nearly white and the one at 140 s washed out; faces and rooms near black |
| 87 | diagnostics build, cutscene run | what is dropped "without a host pipeline" (2,078 a frame in the cutscene, 339 in the world) is the ray-tracing structure builder's stages, which are off: the missing light, not missing geometry |
| 88 | + `VICTORY_BVH_BUILDER=1` | the builder's 53 pipelines are made, then the title reads a pointer of half-float infinities (0x7c00fc00fc08) at +0x27c195d, as before the merge: what the builder writes through raw addresses does not reach title memory. victory-fork #814 (live ray tracing, 0.7-1.4 fps on an RTX 2070 Super) addresses it; 68 conflicts with this tree, parked |
| 89 | the merged tree warm, `VICTORY_VIEW_GUARD=0` | 12.1 fps at the window and turned (the launcher's older tree: 13.6 / 13.1); the grate draws |
| 90 | + `VICTORY_ASYNC_SUBMIT=1 VICTORY_QUEUE2=1` | played with a DualSense during the run: other rooms, not comparable |
| 91 | + victory-fork main a253706 and #916 (only a shader that can store by address declares its copies written) | 12.1 / 11.8 fps; preparing 39 ms, GPU wait 12 ms. The frame line: the heap buffer pass 29.7 ms a frame (15,286 slots looked at) against 10.9 ms (3,206) in the older tree: the cost of mirroring the descriptor records the title copies as bytes (a26859f), which is also why 22 buffer views are left null against 1,696 |
| 92 | + victory-fork #921's switches (`ASYNC_SUBMIT QUEUE2 TEXTURE_NO_WAIT MERGE_GAP=64 MERGE_RUNS=4096`), victory-fork main 0b0074e | the walls round the window gone and white specks on the bunks: shader writes lost. 7.95 fps at the window with 0.5 GB of the PC's memory free. Not used |
| 93, 94 | a sampling profile of the measured build (the new `stacks` feature: `VICTORY_STACKS` without the diagnostics) | the submitting thread: 72% in submissions, 22% in the descriptor sync (`sync_heaps`), most of that in `HostHeap::mirror`: the driver making descriptors (8.6%) and the slots' lock, which the recording threads take for their draws (7%). The heap buffer pass shows in 1-2 samples of each recording thread |
| 95 | the descriptor sync on a thread of its own, beside the submission's copies (joined before the lists go) | worse: 9.8 fps at the window, preparing 39 to 60 ms, black rectangles over the cell and walls missing: slots made from copies not yet brought up to date. Taken back |
| 96 | cutscene run on the Series X profile | the diner and the faces lit and detailed where Series S shows them dark and coarse; a piece of one character's body missing, a black triangle in the cell; 8.7 fps in the world against about 12 |
| 97, 98 | the slots' descriptors made outside the slots' lock (`FLAMINGO_MIRROR_UNLOCKED`, on), against the same build with it off | 12.40 / 12.44 fps against 12.06 / 11.93, lows 12.0 against 10.4-11.7; the picture the same |
| 99, 100 | the title buffers' registry under a reader-writer lock (a lookup that finds a copy reads) | 12.30 / 12.13 fps in run 100 (run 99 had one long dip): no gain over run 97. Taken back |
| 101 | the heap buffer pass's slots looked at on 4 threads, the ranges noted after | much worse: 5.0 fps at the window (lows 0.6), the pass 82 ms (textures 51 ms), blocks and specks on the picture: making views and copies for draws is not safe side by side. Taken back |
| 102 | a play session by the user (24 minutes, DualSense, the old save moved aside so a new game starts) | the whole opening played (the save from an early test run had sent every start to the prologue). The world: 8.6 fps on average, median 7.8, a tenth of the time under 3.5; up to 11,522 draws in the city. Driving through the city looks right in daylight; black rectangles, missing shadows and dark rooms elsewhere; the first boat mission failed every time |
| 103 | + the user's pull request #2 (guard-page faults in the plain copy), victory-fork #959's eight audio fixes, vx-layer eee5667d (textures shaders write that the title reads on the CPU are copied back), failed programs named | the four programs that fail are `CS_ShadowRenderTiledCascadesAlphaCBPPPC` (does not translate: a lane mask exchanged where not every lane runs), `CS_AlphTransmissionPrefilterMip`, `CS_DefragHeap_ComputeRelocations` and `CS_CollectTriangles_Meshlets` (refused) |
| 104 | + the last two in the wave-wide form | both made; the boat floats and can be driven (the start now loads the boat mission) |
| 105 | + `VICTORY_WAVE_DEEP_SKIPS=0`, the shadow program in the wave-wide form | worse: both refused again, `CS_TransmissionVolumetricsSubmerged` no longer translates; the shadow program still fails |

## Stats overlay

`F9` shows or hides a RivaTuner-style overlay at the top left of the game window. It shows fps and frame time, frames, draws per frame, CPU (preparing submissions), GPU (waiting for the card) and back (copying back) in ms per frame, VRAM in use / budget, the process's RAM, and a frame-time graph with lines at 15 and 30 fps. It updates ten times a second on its own thread and is click-through. The game window owns it, so it shows only while the game is in front.

- `FLAMINGO_OSD=1` shows it from the start (the launcher sets this).
- `FLAMINGO_OSD=0` turns it off completely.

## Memory

In gameplay the process holds about 15.5 GB of the PC's RAM. The title's own memory is about
5.3 GB of that: its heaps 1.3 GB, the pages it maps 4 GB. The rest is the layer's:

- Copies of title memory that shaders read by address ("windows", a 4 MiB grid): 3.7 GB lying over
  3.6 GB of title memory.
- Other private blocks of 1 MiB and more: 4.5 GB, among them five of 1 GiB.
- Process heaps: 0.9 GB. Mapped files, the pipeline library among them: 1.3 GB (can be dropped and
  read again).
- Write-combined buffers shared with the card: 11 GB committed, but only 0.8 GB of it in RAM.

On 32 GB, with a browser and Discord open, about 3 GB stays available and Windows pages. The windows
are the biggest piece the layer could give up: shaders reading title memory in place, as XCL does,
would need no copies.

## Discord Rich Presence

Discord shows "Playing <application name>", the title's name, whether it is in the menus or the
world, the frame rate and the time played. It needs the Discord desktop app running, and a Discord
application of your own. Making one takes a minute:

1. Open https://discord.com/developers/applications, click **New Application** and name it
   **Flamingo**. The name is what Discord shows after "Playing".
2. Copy its **Application ID** from **General Information**.
3. Under **Rich Presence**, then **Art Assets**, upload `assets/flamingo-512.png` with the name
   `flamingo`.
4. Put the id in `%LOCALAPPDATA%\flamingo\discord_app_id.txt` (the digits alone), or set
   `FLAMINGO_DISCORD_APP_ID=<id>`.

`FLAMINGO_DISCORD=0` turns the presence off. `FLAMINGO_DISCORD_HIDE_TITLE=1` leaves the title's name
out. Your Discord friends see whatever the presence shows.

## Diagnostics worth knowing

- `VICTORY_GPU_TIMING=1`: GPU timestamps per command. Reports the card's time by command and the
  longest commands. It waits for every list, so its fps is not representative.
- `VICTORY_SHADER_LOOP_LIMIT=n`: translated loops stop after n iterations (default 2^20). Shaders
  get their own cache entries, so this costs one cold compile.
- `VICTORY_INPLACE=1|2`, `VICTORY_INPLACE_PROBE=1`: title memory opened as a GPU heap, used in
  place (XCL's approach) instead of copied.
