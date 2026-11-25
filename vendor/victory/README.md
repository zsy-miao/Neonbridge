# VIctory fork (work in progress)

A work-in-progress fork of [VIctory](https://github.com/Marvisak/VIctory), a
compatibility layer that runs an Xbox Series title natively on Windows from
files the user supplies. This fork follows upstream
`main` and adds fixes aimed at one goal: getting the game's world to render
correctly and playably on one particular PC.

**This repository contains no game files, code or assets.** You need your own
copy of the game. It is not affiliated with or endorsed by the game's
developer or publisher.

## Credits

- **[VIctory](https://github.com/Marvisak/VIctory)** by Marvisak and its
  contributors is the base of everything here: the loader, the runtime
  services, the Direct3D 12 back end and the shader recompiler. Licensed
  GPL-3.0-or-later; so is this fork. Upstream's history is kept intact below
  the single commit that carries this fork's changes.
- **[XCL](https://github.com/ivall/xcl)** by ivall and its contributors
  (GPL-2.0-only) was used as a reference for how the same title behaves under
  another layer. Fixes inspired by it were written anew in VIctory's code.
- The guard over every view of a memory page is code by **myokex**
  ([victory-amd](https://github.com/myokex/victory-amd), commit `c6dc4d8`), which
  adapts pull request #115 here by Spexivus, its correction by joroyt and a part of
  VIctory #1122 by tightfall. The read-beside fix for kernel reads into title
  memory is by **Joshua Odom** (commit `10ea38e` of the same fork family). Both
  are carried here unchanged apart from one merge conflict.
- This fork's changes were developed with [Claude Code](https://claude.com/claude-code).

## Status of this fork (2026-11-25)

**Not playable yet: about 8 to 10 frames per second in the first rooms and 5 to 8.5 in the
opening cutscene (the console runs 30), and shadowed areas are black.**
Tested on one machine: Windows 11, Intel i7-14700KF, NVIDIA RTX 4070 12 GB,
32 GB RAM. The Series X profile is used throughout.

| Stage | State in this fork |
|-------|--------------------|
| Boot, menus, start screen | works, 30 fps (as upstream) |
| Opening cutscene | plays from start to finish, in order, lit: title cards, credits cards, night and dusk shots |
| Saves | the game autosaves a checkpoint and loads it again |
| First gameplay scene (the prologue) | renders: characters, textures, sky, sunlit exterior; HUD, pause menu and pad input work |
| Light in shadow | missing: the game's ray-traced bounce light and ray-traced lamp and sun shadows do not run yet, so anything the sun does not hit directly is black |
| Frame rate in the world | 8 to 10 fps in the first rooms: 97 to 104 ms a frame at about 3,850 draws, 122 ms in a heavier view (6,774 draws). 5 to 8.5 fps through the opening cutscene, by stretch (runs 271 to 273, with `VICTORY_CARD_WINDOWS_MB`). All measured with the switch set of run 253, which is off by default: `VICTORY_LEVELS_ON_DEMAND=1 VICTORY_ASYNC_SUBMIT=1 VICTORY_QUEUE2=1 VICTORY_INDIRECT_WINDOW=1 VICTORY_TEXTURE_NO_WAIT=1 VICTORY_MERGE_GAP=64 VICTORY_MERGE_RUNS=4096 VICTORY_STAGING_POOL=0 VICTORY_COPY_PLAIN=1 VICTORY_JOIN_PARTS=1 VICTORY_BACK_OWN_VIEW=1 VICTORY_INDIRECT_GATHER=1 VICTORY_CARD_REHASH_RUNS=1`. The plain defaults were last measured at 420 to 450 ms a frame and not since. It started at 3 to 6 seconds a frame |
| Stability | no driver hang or device loss in normal runs (a hang introduced by the wave64 update was found and fixed: its barrier form for 64-lane programs is now off unless `VICTORY_WAVE_WIDE=1`) |

### Newest (2026-11-25, night UTC)

**Small buffers into the card's memory: about 18 ms of card time a frame gone, 5 ms of frame so far.**
Tested on the machine above, first rooms, at like draw counts (runs 274 and 275).
- **Why some compute passes were slow.** A title buffer's host copy under 1 MiB stayed in the PC's
  memory (about 3,700 of 3,800 copies, 30 MB in all), so every load, store and atomic a shader did
  on it crossed the bus. The passes that were slow by the thread were small dispatches on such
  buffers. `VICTORY_CARD_BUFFERS_MIN_KB=64` (with `VICTORY_CARD_BUFFERS_MB=1280`; off by default)
  puts every copy in the card's memory: the title's lists went from 68.0 to 49.8 ms of card time a
  frame; single passes from 6.7 to 1.6 ms, from 4.1, 2.7 and 2.0 ms to under 0.3 each. The frame:
  114.9 -> 109.7 ms. The layer's own send-and-hash lists rose from 11.6 to 23.0 ms (each small copy
  is hashed over a whole 64 KiB); `VICTORY_CARD_HASH_LEN=1` hashes the buffer's own bytes instead
  and has not run in a game yet. The card now idles 12 to 40 ms a frame: the thread that prepares
  submissions is the limit.
- **`changed_runs` by masks** (the default; `VICTORY_CHANGED_RUNS=0` the old walk, `=alt` in turn):
  64 bytes at a time as one mask of differing bytes. 3.8 ms less work a frame on the submitting
  thread at equal draws; the frame's gain is inside the noise. A first form (64 equal bytes, then
  eight at a time) was a fifth slower than the walk on sparse changes and never ran.
- **Scalar buffer loads, four dwords a call** (`VICTORY_SHADER_OPT=127`; not in the default 63):
  the dwords of one console load instruction are read by one call. Whole pack: with the bit off
  25,635 of 25,635 shaders are byte-identical to the build before; with it on 20,154 change and
  buffer loads fall by 38 %. Tested on the software adapter, also across a view's end. Whether a
  card is faster for it is not measured yet.
- **Pull request #999 merged** (cxy0072019-sketch, porting a patch by Geckomacabre):
  `VICTORY_TEXTURE_BACK=1` copies textures of the ocean's kinds that shaders write back into title
  memory, where the title's own code reads them. Off by default. Without it a boat sank here and
  its mission failed at once (run 275). With it (run 276, both shader caches wiped first): 22,900
  read-backs landed in six minutes of play and the tester got past that point: the largest step
  of the day. The same run then ended with the card lost (the driver logged two errors, then
  "device removed", with nothing of the layer's running on the card at that moment). Why is not
  known; run 275, the same set without this switch, played twenty minutes without an error.
- **`VICTORY_BACK_READS=1|2|alt`** (diagnostics builds): counts who touches what the copy-back
  stored (the measuring step before any lazy copy-back). Not run yet.
- The texture pass's time budget: never reached in the first rooms (about 5 ms a frame); 15 to
  30 % of the frame in an open-world session. The translator emits no stack slots to promote:
  registers are SSA values already.
- Not run in this tree: the unit tests after the merge (the same changes passed them in the
  working tree they came from: 182 of the graphics crate's, the new shader test both ways).

### Earlier (2026-11-25, evening UTC)

**Update, 2026-11-25 evening: a frame capture built into the layer, and a
card time that does not add up yet.** No frame-rate gain is claimed; the
headline numbers stand. (A heavier view of the first room: 122 ms a frame
at 6,774 draws in run 266. The 97 to 104 ms below are at about 3,850
draws.) The code of this entry is in the tree.
- **An outside frame debugger did not work here.** RenderDoc 1.45, loaded
  through a hook (`VICTORY_RENDERDOC=<path of its DLL>`), ended run 263
  after about 10 s of loading: an access violation inside its library,
  then "device suspended" from every host call (its wrapped device; no
  driver event). The stack showed a streaming program's capture hook
  making a Direct3D 11-on-12 device on the wrapped device. A MISTAKE of
  that run: the driver refused the saved pipeline library and the layer
  saved an empty one over it (366 MB of 3,385 pipelines became 32 bytes;
  the next run drew black frames at 0.7 to 1.5 frames a second while
  building them again). Fixed: a rejected library is kept as
  `host-pipelines.rejected.bin`, never saved over, and with RenderDoc
  loaded the library is neither read nor saved.
- **DirectStorage reads wait for the title's Submit** (after upstream
  VIctory eddde734: its hold and two tests, not its diagnostics). The
  title fills a batch's page list only right before its Submit; a queue
  that sent itself when half full started reads into physical pages
  before that. Run 266: 3 such cases (2,079 reads held of a queue of
  4,159), no stall, 0 failed page writes, frame time not worse. Faces
  were right in that one launch; face faults come and go between
  launches, so a cure is not shown yet.
- **Processor use, per core and per thread (runs 266, 267): nothing is
  full.** Of 28 logical processors the machine keeps 6.7 busy in the
  cutscene and 4.5 in the room, the process about 3.6 and 3.1. The
  busiest thread, the layer's submit thread, is 42 to 58 % busy; the
  title's most crowded pinned group is 25 to 29 % full. No look in the
  room found the render, the submit or a recording thread on a slow core
  (0 of 1, 0 of 8, 0 of 120). `VICTORY_SPREAD=alt` in the cutscene: 262 ms
  a frame off, 262 on. The frame is a chain of waits: more cores alone
  give nothing. Splitting the submit thread's work would use them: it
  works 83 ms of the room's 122 ms frame (bound: a frame of about 100).
- **`VICTORY_WIDE_REFRESH=alt`: no gain, dropped** (copies of title memory
  refreshed on several threads, twenty seconds on, twenty off). Cutscene,
  run 268: copies 31.2 -> 26.0 ms a frame (17 %) where 10 to 15 ms were
  predicted. Room, run 269, 10 readings each way: copies 30.6 ms off, 34.1
  on; submit thread 78.6 against 78.4 ms.
- **Frame shot: a capture of one frame, built into the layer**
  (`VICTORY_SHOT=<folder>`; taken when a file named `take` appears there,
  or at `VICTORY_SHOT_AT=<frames after the first frame of 500 draws>`). A
  shot holds every command of the frame in order (draws, dispatches,
  copies, submissions, with list and thread), the card's clock before
  each command, and a picture of each texture as a pass leaves it. It
  loads no DLL and wraps no device: only texture read-backs and
  timestamps, which every run uses already. Hence built in, not outside.
- With little memory free, a shot takes at most half of what is free
  above 0.7 GB, and pictures that do not fit are taken in parts from
  later frames. A shot is exactly one frame of the title: the title
  records its next frame while the layer still runs this one, and the
  first form (run 269) held both, so every pass showed twice. It also
  took 0 pictures: the title uses enhanced barriers and the layer moves
  textures between states itself; the pictures are now taken there.
- First shots, run 270 (300 s, 0 errors, no driver event), both in the
  opening cutscene. Frame 1864: 6,348 lines, 3,618 draws and dispatches,
  54 pictures of 82 places, then ended early with 685 MB free. Frame
  2849, complete: 10,334 lines, 7,120 draws and dispatches, 77 of 78
  pictures. Those looked at are right pictures of the frame.
- **What the shots say, and what is NOT explained.** Run 270's two
  frames: the card's time for the title's own commands is 20.1 ms
  (compute 13.3, drawing 6.3) and 22.0 ms (drawing 11.3, compute 10.1).
  The older per-list card meter gives the title's lists 57 to 78 ms a
  frame at that time: 35 to 55 ms more. The shot then got a stamp at
  every list's start and around the layer's own lists. Run 271's two
  frames: the title's commands 25.2 and 15.4 ms; the layer's
  send-and-hash lists 35.0 and 17.8 ms (14 and 8 lists), its compare
  lists 12.1 and 6.3 ms (13 and 7). The lists it runs before title lists
  took 36.1 ms in the second frame, but they also hold the shot's own
  picture copies, not separated yet: the gap is not accounted for. If it
  is the layer's, the card's ceiling is higher than the "near 20 frames
  a second" given below, which rests on the per-list meter.
- **The staging-buffer pool, on and off in turn** (run 270,
  `VICTORY_STAGING_POOL=alt`, 128 MB kept). On by default; off in test
  runs since run 224, its first long run, lost the device (cause never
  found; that driver error had also come up before the pool existed).
  Predicted: page faults per texture upload from about 100 to well under
  50, long frames 60 to 115 ms shorter at equal uploads. Measured: 6.27
  frames a second off, 5.77 on (different scenes; the 7 pairs go both
  ways). Long frames average 391 ms with 204 uploads off, 421 ms with 310
  on: 1.92 ms an upload off, 1.36 on, but the texture pass fills its 250
  ms time budget either way, so frames are no shorter. Page faults per
  upload: 132.7 off, 128.9 on: prediction not met. No gain shown: not
  adopted. The frame log's "staging made" counter read 0 with the pool
  off; it counts now.
- **Memory, measured.** The process holds about 24 GB, 16 GB of it in
  memory, on a 32 GB PC. Free: 3.6 GB in the cutscene and 0.9 to 1.8 GB
  in the room in run 269; 3.6 to 5.3 GB late in run 270 (why they differ
  is not known). The card is nearly full: 9.0 of a 10.2 GB budget. Title
  buffers of 1 MiB and more are in its memory in these runs (1,024 MB at
  most); the 872 copies of what the title's own allocator hands out
  (vertices, indices, constants: 3.6 GB) are in the PC's memory and read
  over the bus at every draw. The card has no room for all of them: it
  would come out of the textures' share.
- **Some of those copies in the card's memory: fewer long frames, in two
  launches of two** (`VICTORY_CARD_WINDOWS_MB`, off by default). Run 271
  asked for 512 MB (124 to 137 copies went to the card), run 272 for
  2,048 MB (355 copies, 1,356 MB, went there; the layer's own line at 80 %
  of the card's budget turned the rest down). Both: 300 s, 0 errors, no
  driver event, pictures right. Frames a second in three stretches of the
  opening cutscene (20 to 110 s, 110 to 200 s, 200 to 290 s after its
  first frame), and the share of the last stretch's time spent in frames
  over 250 ms:

  | run | on the card | first | middle | last | long frames, last stretch |
  |---|---|---|---|---|---|
  | 269 | none | 3.95 | 7.49 | 7.59 | 21 % |
  | 270 | none | 3.93 | 7.99 | 6.68 | 26 % |
  | 271 | 512 MB | 5.44 | 7.26 | 8.48 | 4 % |
  | 272 | 1,356 MB | 5.42 | 8.16 | 8.32 | 6 % |

  Textures made again after being let go for lack of room: 5.1 and 6.9 GB
  in the runs without, 1.9 and 1.1 GB with. That is what the long frames
  are made of. Why fewer textures are let go is not known: the title's
  draws cost the card the same per command with and without (+1 %, 37
  shader sets found in both captures). The price: passes that made no
  texture for lack of room rose (2,067 and 1,280 without; 2,504 and 3,699
  with). Four launches, two each way, and earlier launches without it
  reached 3.4 to 5.1 in the first stretch: a result, not yet a default.
- **The same copies written straight into the card's memory**
  (`VICTORY_WINDOW_UPLOAD=1` with `VICTORY_CARD_WINDOWS_MB`, off by
  default). It needs GPU upload heaps: a resizable BAR, and a driver and
  a Windows that offer them. The layer asks when it makes the device and
  says so in the log, and it tries the first such buffer with a plain
  copy and the kernel's copy and reads it back before any is used. Run
  273 (closed at 155 s; 0 errors, no driver event, picture right): 368
  copies (1,400 MB) were one buffer each, with no side in the PC's memory
  and no pass that sends them up. The PC's memory shared with the card:
  3.8 GB against 5.7 GB in run 272; free memory 3.2 to 3.6 GB against 1.1
  to 1.9 GB. Frame rate as run 272, as far as one shortened launch can
  tell (4.99 and 7.86 frames a second in the first two stretches against
  5.42 and 8.16). The send-and-hash lists did not fall as predicted (12.3
  against 13.6 ms a frame at like draws). A memory gain; no speed gain
  shown.
- A review of the open pull requests for frame rate found no single large
  gain. The nearest is #1014 (the card-side compare hashes a piece with
  64 threads; a tester's claim on an RTX 4080: send-and-hash lists 14.0
  -> 2.5 ms, compare lists 5.6 -> 1.3 ms). Not measured here.
- **Next, none of it measured yet:** (1) the translated shaders: each
  shot ranks them by card time, and the title's own commands are 15 to 25
  ms of the card's frame; (2) the shot's own copies taken out of the
  lists the layer runs before title lists, so the card's frame adds up;
  (3) what shaders wrote left on the card until the title's processor
  reads it, where today every changed piece is fetched back every frame
  (42 to 70 MB): being studied, nothing built; (4) a faster byte compare
  for the submit thread (8 and 64 bytes at a time; its test of 20,000
  cases against the byte-by-byte walk passes; not in a run yet); (5) the
  texture pass's time budget, 250 ms against less. Title buffers in
  place (`VICTORY_INPLACE`) were listed as the large step; a closer look
  at this PC says otherwise: both ways of opening title memory to the
  card pin it in a PC with 1 to 5 GB free, and the buffer that carries
  the most traffic cannot be opened at all. A memory step and a modest
  time step, not the road to 30 frames a second by itself.

### Earlier (up to 2026-11-25 afternoon)

**Update, 2026-11-25 afternoon: what a frame is made of, measured, and two
changes that hold.** First rooms, about 3,850 draws a frame: 104 ms a frame
(9.6 frames a second) with the first two changes below, 97 ms (10.3)
with all of them, from about 120 ms in the morning. Still far from the console's 33 ms.
- How the title paces itself (new meters in `fence.rs` and
  `console_insn.rs`): it never asks for an event. Each frame begins with
  one small submission and a signal, and a title thread then waits for
  that fence value by reading it in memory with `monitorx` / `mwaitx`
  (AMD-only: about 280,000 emulated exceptions a second on an Intel
  processor, all from this wait). The value is set when the layer has
  finished the whole frame before. So a frame is the time the layer needs
  for one frame's burst of submissions plus about 20 ms of the title's
  own; putting presents behind the line, or giving the title's threads
  more processors, changes nothing (both measured).
- Two walls stand side by side and both are full: the layer's submit
  thread (about 95 ms of a 120 ms frame in the morning) and the card
  (about 102 ms busy, by a timestamp pair around every host list:
  `cardmeter.rs`). Taking time off one alone moved the frame by 0 to 7 %.
- `VICTORY_JOIN_PARTS=1` (off by default): the title's lists wait for
  memory values, which cut its ~15 submissions a frame into ~29 parts;
  parts that follow one another now go to the host as one submission
  (9 a frame), the same lists in the same order. About 6 %.
- `VICTORY_BACK_OWN_VIEW=1` (off by default): what shaders wrote goes
  back into title view memory through a view of the layer's own, so the
  copy that wrote it is not read in again. About 9 % (submit thread 93 ->
  81 ms a frame, 104 -> 80 MB a frame checked). A CORRECTION: an earlier
  run seemed to show washed-out frames with this switch; the same frames
  turn up in about 6 of 21 runs with no switch at all (cause not known),
  and `VICTORY_BACK_OWN_CHECK=1` counted 0 stores gone astray in 1.36
  million.
- Where the card's time goes now (`VICTORY_CARD_METER_PIPELINES=1`, a
  table by pipeline): no pass is large. About 82 ms of the title's lists
  are spread over ~3,400 pipeline bindings a frame; ~2,400 of the
  indirect commands still get a clamp pass of the layer's with draw
  windows on (960 are compute dispatches; about 1,000 are mesh records
  the title keeps scattered through a 100 MB argument buffer, which a
  window of records at one step cannot serve).
- `VICTORY_INDIRECT_GATHER=1` (off by default): a single indirect record
  that misses its window gets a slot in a scratch buffer of its list, and
  the list's prelude clamps all such records in one pass by a table of
  their offsets (the clamp shader with a gather bit), where each draw had
  a compute pass of the layer's in front of it. Proved by clamping every
  gathered record again at its draw and comparing on the card
  (`VICTORY_INDIRECT_WINDOW_CHECK=1`): 7.9 million records, none differed.
  About 600 of the ~1,600 draws that missed a window are served; the
  others come after a dispatch or a storing draw in their list. With
  `VICTORY_CARD_REHASH_RUNS=1`: 104 -> 97 ms a frame (10.3 frames a
  second at 3,800 draws).
- What the card's time is now, added up: of ~87 ms busy in a 97 ms
  frame, the layer's own lists are ~19 ms and its clamp passes 10-20 ms;
  about 50 ms are the title's own ~30 compute passes and its geometry at
  2560x1440 as translated. With every pass of the layer's gone, this
  scene would still be near 20 frames a second on this card: the
  translated shaders have to get cheaper too. (Open again: see the evening update above.)
- The frame rate is drawn into the top left corner of every frame shown
  (`fpsbox.rs`; `VICTORY_FPS_COUNTER=0`: not): the window's caption has
  it too, but a capture of the picture does not carry the caption.
- `VICTORY_COPY_CHANGED=1` (off by default): a refresh of a copy in the
  card's memory sends on only the 256-byte chunks that differ from the
  copy's CPU side. The layer's sync lists on the card fall from 12 to 8
  ms a frame, the submit thread pays 3.6 ms for the compare and is then
  the limit: no net gain yet.
- HONEST LIMIT OF ALL THE ABOVE: those numbers are the scene standing
  still. While the camera moves the title streams assets, and about half
  the time goes to frames of ~490 ms in which the card is idle for ~330
  ms: some 850 remaps of title memory, 260 texture uploads and 450 disk
  reads in one frame. That is the next thing to measure and cut.
- The dips while moving, measured (a sampling profile of the opening
  cutscene, and a 370 s play session): the mean is 4.5 frames a second,
  with half the time in frames of ~490 ms in which the card is idle for
  ~330 ms. In those frames the title's render thread waits for its own
  jobs, not for the layer. One of those jobs is the title's page-mapping
  copies (`CopyPageMappingsBatchX` -> `xmem::map_alias`): the title maps
  every streamed texture page into its tiled heap, a range of one page
  each (over 4,096 different pool pages in a minute, about 850 pages in
  a frame that streams), and each is a view taken away and a view mapped
  in the kernel: 32 ms of every cutscene frame, 246 ms of a streaming
  frame, nearly all of it waiting on the process's address space.
- Two attempts on that, both taken back out: leaving alone pages that
  already show what is asked (counted: there are none, every remap is a
  real change), and mapping pool pages in a row as one view (1.8 to 4.2
  pages a view, remap time 140 -> 79 ms a frame, BUT faces came out black
  or red and one run drew garbage in 3 launches of 3, against none in 6
  launches without it). Reverted; why a longer view breaks texture
  streaming is not known yet.
- Measured and found to give nothing (switches left in, off):
  `VICTORY_REFRESH_ONCE`, `VICTORY_ASYNC_PRESENT`, `VICTORY_VIEW_KNOWN`,
  `VICTORY_SPREAD`, `VICTORY_QUEUE2=3`, `VICTORY_WINDOW_JUMP`. Each has an
  `=alt` form (twenty seconds on, twenty off, each change said in the
  log) so that one run measures both in the same scene.

**Update, the same night: the first room now renders right on the test
machine** (walls, textures, people, lamps), at 190 to 450 ms a frame (2 to 5
frames a second): the picture work has caught up, the frame rate has not.
What did it, on top of the list below:
- Descriptor records the title copies into its heap as plain bytes are
  mirrored before every submission (pull request #305 by Flyander, after
  upstream VIctory 86a5ed6). Measured here: about 40,000 records a frame
  that no call had announced; their host slots were empty or stale.
- A buffer view that does not start on an element of its buffer's host
  copy gets a copy of its own bytes instead of being refused (after
  upstream 86a5ed6): 3 to 6 such views a frame.
- Room on the card is kept for the render targets and buffer copies a
  world makes in its first frame: 98 buffer copies are made (945 MB) where
  the runs before made 1, and the processor no longer compares 550 to
  710 MB a frame looking for shader writes (25 MB now).
- Levels on demand ran on the card for the first time
  (`VICTORY_LEVELS_ON_DEMAND=1`, still off by default; with the fixes of
  pull request #747 by AscilCH): 3.1 GB of the card not allocated for
  texture levels the title does not hold, no device loss.
- Guarded blocks under storage writes are opened by default; 14 more
  shader programs translate; a program is translated with first-half
  interpolation pairs only if it was refused for one (with that on for
  every program, 8,551 working pixel programs came out changed).
- Frame rate, first cuts on the processor side: the descriptor sync uses a
  cheap fingerprint and leaves draw-table slots to their draws (the pass
  over the heap went from 70-220 ms a frame to about 3), and memory the
  guards already hold is not asked of the write watch (pull request #670
  by Jonirulah). Card copies' pieces can come down inside their own
  flights (pull request #648 by Jonirulah, `VICTORY_CARD_FETCH_INFLIGHT=1`,
  off by default: the copy-back thread's wait went from 181 ms a frame to
  0 here, but the card got busier, so no gain is claimed yet). The frame is
  still 250 to 380 ms: the card is the limit now, most of it the
  per-record clamp before each of some 6,700 indirect draws a frame.
- Pull request #220 (cheaper indirect draws) is IN this tree now, its
  windows off by default (`VICTORY_INDIRECT_WINDOW=1`). History, so nobody
  repeats it: with windows on it lost the device three times here. Its own
  checker (`VICTORY_INDIRECT_WINDOW_CHECK=1`) then showed one list drawing
  90 records of 2^24 vertices from a window, where the per-draw path lets
  eight through; a record of 2^23 vertices an instance or more now runs
  empty. Since that fix: eight runs with windows on, seven without a
  device loss and one that lost the device 44 s into the world (cause not
  found; see the driver watch below). One earlier loss after the fix had
  the in-flight fetch (#648) on as well; do not combine the two. The
  in-flight fetch itself measured slower here in two runs and stays off.
- Frame rate, second night (an RTX 4070; the first room went from about
  3.5-4 to 6-7 frames a second with ALL of the switches below; the
  defaults are unchanged and give about 4.5-5):
  - `VICTORY_ASYNC_SUBMIT=1`: submissions are prepared and sent by a
    thread of the layer, not by the title's render thread, which a
    profile showed inside the submit path for 257 of 292 ms a frame.
  - `VICTORY_QUEUE2=1`: fetches of what shaders wrote go to a copy queue
    (brought back, with the fix for a piece fetched beside a later
    flight); the copy-back thread's wait fell from about 125 to 7 ms a
    frame.
  - `VICTORY_INDIRECT_WINDOW=1`: see above.
  - `VICTORY_TEXTURE_NO_WAIT=1`: a texture refresh no longer waits while
    the title remaps memory; the loading phase had 5 readings over a
    second where it had 18.
  - `VICTORY_MERGE_GAP=64 VICTORY_MERGE_RUNS=4096`: title bytes put in
    under flights' shader writes are joined over small gaps (NOT exact:
    read the comment at `merge_gap` before using it).
  - On by default: fills of upload memory and read-only buffer copies no
    longer wait for flights and are checked against what the flights then
    wrote (`VICTORY_FILL_WAIT=1` waits as before; no mismatch in any run
    so far); up to 512 idle host lists and texture staging buffers are
    reused instead of made and freed every frame.
  - `VICTORY_PROFILE=1` (diagnostics builds): a sampling profiler inside
    the layer; it is how the above was found.
- Frame rate, third session: measuring first (same RTX 4070, first room).
  - A frame log (`VICTORY_FRAME_LOG`, on in diagnostics builds): every
    frame's length, and for the usual frame and each long one what it was
    made of (the render thread's time in the present call, the submit
    thread's work, copies, copy-back, recording, texture work, faults,
    disk). A card meter (`VICTORY_CARD_METER`) adds the card's time by
    kind of list from a timestamp at each list's start and end.
  - What they showed, windows off: a usual frame of 136 ms; the title's
    render thread waits 77 ms of it in the present call for the submit
    thread, which works 118 ms (copies of title memory 41, copy-back 40);
    the card is 85-98 % busy, of about 130 ms a frame 16 ms in our lists
    that bring card copies up to date, 10 ms in our compare lists and 102
    ms in the title's own lists with the per-record clamp. Frame time is
    linear in draws (about 32 microseconds a draw all-in): the title draws
    about twice as much on some frames, which are the long ones.
  - The regular dips of earlier test runs were the test harness: a saved
    frame (`VICTORY_CAPTURE_WORLD`, every 8th world frame) is read back
    and written as a PNG inside the present, 235 ms each.
    `VICTORY_CAPTURE_WORLD_EVERY=64` for runs that measure speed.
  - `VICTORY_INDIRECT_WINDOW=1` measured again with those instruments. A
    CORRECTION: the first figures given here (6.0 against 8.9 frames a
    second) compared two runs that stood in different scenes during the
    measured window (about 4,300 against 1,550 draws a frame) and are
    withdrawn. Compared at equal draws, by a line fitted to each run's
    frame times against its draws per frame: windows off 145 ms at 4,500
    draws and 271 ms at 10,000; windows on with `VICTORY_COPY_PLAIN=1`
    124 ms and 211 ms. On screen: 7-8 frames a second in the first rooms,
    10-12 in their lighter views.
  - A driver watch (on by default; `VICTORY_DRIVER_WATCH=0` off): the
    layer subscribes to the system's event log and logs every entry of a
    display driver as it is recorded; the first one turns draw windows
    off for the rest of the run. Every device loss here had such entries
    2.7 to 55 seconds before the device was removed, and no run without
    them lost its device.
  - A copy between two title textures that share one host texture is no
    longer recorded (the host does not allow a texture copied onto
    itself): 402 of them in a five-minute run.
  - The list of shader-visible heaps is a reader-writer lock (recording
    threads waited 48 ms a frame, summed, behind the heap sync); the heap
    sync looks at a changed page's records under one lock, fingerprint
    first; guarded view memory is left out of a submission's write-watch
    askings ahead (after triippywhale).
  - `VICTORY_COPY_PLAIN=1`: copies of title memory made in place by this
    process, with a handler that falls back to the kernel's copy on a
    missing page. Its first form also took the layer's one-byte "is this
    page there" reads, 1,000 to 1,300 faults a frame, and won nothing;
    reads under 256 bytes now stay with the kernel and a source that
    faulted is left alone for two seconds (4 to 9 faults a frame). The
    submit thread's kernel time fell from 44 to 20 ms a frame; the
    picture is the same.
  - The layer's submit and copy-back threads ask for the fast cores of a
    hybrid processor (`VICTORY_FAST_CORES=0`: not): the submit thread was
    found on a slow core in 6 % of its looks, now in none of 21,059.
    `VICTORY_DISK_CORES=slow` (off, not measured) asks the slow cores for
    the DirectStorage workers, which are as many as the logical
    processors and free to run anywhere.
  - Behind switches, built and unit-tested, not yet shown to pay:
    `VICTORY_REFRESH_ONCE=1`, `VICTORY_VIEW_KNOWN=1`,
    `VICTORY_BACK_OWN_VIEW=1` (picture to be checked),
    `VICTORY_ASYNC_PRESENT=1` (the present goes behind the queue's line;
    no gain while the card is the limit).
  - Tried and refused: `VICTORY_BVH_HOLD=1` holds back the 64 programs
    that walk ray-tracing structures nothing has built; the picture loses
    its indirect light with it, so it stays off.
  - The process opts out of the system's efficiency mode
    (`VICTORY_EFFICIENCY_MODE=1` leaves it); on a hybrid processor the
    frame log found the title's render and recording threads on fast
    cores in every look of a run, and the layer's submit thread on a slow
    core in 6 % of its looks.
  Still open: the per-record clamp is most of the card's time, and the
  copies between the PC's memory and the card's are most of the submit
  thread's; the cause of the one device loss with windows is not known.
  A first play session past the first rooms reached the open world and
  lost the device there with no driver entry (reason "device removed"):
  the card had no room for new textures from the opening cutscene on
  (9.2 of a 10.3 GB budget on a 12 GB card; 2,073 passes made no texture
  for lack of room; 44,613 textures let go and made again in 13 minutes).
  Cause of that removal not known either.
- Known and counted, not fixed yet: an indirect compute pass asks for
  about 77,500 thread groups in x where one dispatch runs 65,535 (the
  rest is cut off).

Memory sync between the title's memory and the copies on the card:
- **A write through one view of a physical page is seen by the copies taken
  through its other views** (`VICTORY_VIEW_GUARD=0` turns it off). The title
  shows one page at several addresses and a copy was guarded at one of them
  only. Measured on the test machine: 0.8 to 1.0 million such writes in a
  five-minute run, every one of which used to leave a copy stale. Since it
  went in, the first room had its walls in the later frames of 4 runs out of
  4 (2 of 5 before); the room's very first shot still comes up without walls
  in some runs.
- Kernel reads into title memory are read beside and copied in, so a block
  guarded again in between no longer fails the read.
- Writes of the storage layer into physical pages are counted where the title
  has those pages mapped (10,000 to 50,000 guarded blocks of copies lay under
  them in every run) and can open the guards there:
  `VICTORY_PAGE_WRITES=guards` (off by default; `1`, `slots`, `all` do more).
- Failed page-destination reads are retried (`VICTORY_DSTORAGE_RETRIES=0` = as before).

Card memory (the test card is full from the first frame of the world: 85 to
99 % of its budget for a whole run, about 26,000 texture copies let go and
3,400 made again in five minutes, 2.8 GB of render targets that are all in
use):
- Levels on demand, **off by default** (`VICTORY_LEVELS_ON_DEMAND=1`): a
  texture the title fills takes card memory only for the levels the title
  holds (2.2 to 3.0 GB of the copies were levels it does not hold). Its test
  runs on the software adapter; it has not run on a card yet.
- Refused texture slots are held back for a few seconds so that a pass
  reaches the slots behind them (`VICTORY_FULL_CARD_PASS=0` = as before);
  room can be kept for the card copies of title buffers
  (`VICTORY_CARD_ROOM_FOR_BUFFERS=1`, off).

Tools and diagnostics:
- `victory translate <shader file> --warm all --jobs N` translates every
  shader ahead of a run, on the processor only (about 4.5 to 6.5 minutes
  here); the shader cache is keyed on the translator's source, so a new
  build that does not touch the translator starts warm.
- Counters, all off or diagnostics-only: `VICTORY_MERGE_RACE=1` (title bytes
  a copy-back is about to put back to older values: none found in 138,000
  copy-backs), `VICTORY_WALLS_TRACE`, `VICTORY_CARD_ACCOUNTING`, and a census
  of the copies the texture sweep never lets go.

### What this fork changed (summary)

Rendering:
- Meshlet culling read its scene buffers as the wrong kind of view, so the world drew almost nothing; fixed.
- Per-draw descriptor tables could run out, after which draws used slots never made for them and the card hung; fixed.
- Light packed by the game's compute shaders was read back through empty copies (sky, probes, fog); fixed for every mip level and layer.
- The fog reconstruction, shadow-mask, hair, blend-shape skinning, fire and several material programs did not translate; fixed.
- Gathers at a named mip level (with and without an offset) read level 0; fixed.
- Implicit-level samples outside pixel shaders produced invalid DXIL in 730 compute programs; fixed.
- Rectangle lists, depth and stencil twins for depth textures written by compute, texture read-backs.
- Ray tracing, in progress and off by default: the game's own structure-builder programs translate, title memory can be read by raw address from shaders, the hardware ray/box and ray/triangle test is emulated in DXIL (checked against a reference on the GPU), and 59 of the 64 trace programs translate. Nothing is lit by it yet. See [docs/fork-ray-tracing.md](docs/fork-ray-tracing.md).

Speed and memory:
- Waits for memory values, write-watch queries and page queries no longer walk page tables per call.
- Large title buffers get a copy in card memory, with shader writes found by hashing on the card.
- Submissions can be sent ahead of the wait for the card (`VICTORY_LATE_WAIT`), with a relay thread that lands results as the card finishes (`VICTORY_RELAY`): in a first five-minute run the card's idle time behind the submitting thread fell from about 136 to 30 ms per frame.
- Title memory can be opened as a graphics heap so that buffers are used in place instead of copied (`VICTORY_INPLACE`, first stage only; a probe confirmed the approach on the test GPU).
- The bindless texture queue no longer uploads stale views; the view table no longer grows without bound; the pipeline library is mapped instead of read into memory.
- An intermittent device loss was traced to the pipeline library file being replaced while mapped; fixed. A library the driver rejects is kept under another name and never saved over.

- Translated shaders get clean-up passes over the finished module (dead code, constant folding, reuse of values, branch facts); the pack's host instruction count falls by about 43 %, worth about 20 ms of GPU time per frame on the test machine (`VICTORY_SHADER_OPT=0` turns them off).

Most of the newer speed changes sit behind environment switches that are off
by default (`VICTORY_LATE_WAIT`, `VICTORY_RELAY`, `VICTORY_WIDE_BACK`,
`VICTORY_CARD_BUFFERS_MIN_MB`, `VICTORY_INPLACE`, `VICTORY_BVH_BUILDER`, ...).
Each is described in a comment where it is read.

### Known problems

- Everything in shadow is black (see above).
- The interior of the first gameplay room is sometimes not drawn after the cutscene hands over to gameplay.
- Around 24 GB of memory is in use during play, about 16 GB of it in memory: the 32 GB test PC has about 1 to 5 GB free, and the 12 GB card is nearly full, so textures are let go and made again for lack of room.
- A few style lints are switched off for this fork in the workspace `Cargo.toml` until the newer code is tidied.
- Many changes here are specific to this title and this GPU, and some are marked `shortcut:` in the code where they approximate what the console does.

Everything below this line is upstream VIctory's own README, unchanged; its
status section describes upstream, not this fork.

---

# VIctory

Runs the title (Xbox Series X|S) natively on Windows, from files
the user supplies.

The title's x86-64 code runs unchanged inside our process; VIctory supplies
the platform underneath it and converts the title's data, ahead of time where
it can and on first use where it must (shaders). See
[docs/architecture.md](docs/architecture.md).

## Status (2026-11-23)

**Not playable yet.** The title boots, runs its menus at its own 30 fps and
loads the world, where rendering fails.

| Stage | State |
|-------|-------|
| Boot, first-boot screens (accessibility, language, settings, legal) | works, 30 fps |
| Intro video with sound | works |
| Start screen, profile and saves | works |
| Loading into the world (about 50 s) | works |
| The world (first mission's intro) | reached and runs with indirect draws; its title card renders, the scene after it is still dark; 0.5-1.8 s per frame |

Tested on one machine: Windows 11, AMD RX 6700 XT.

> **Warning.** Entering the world on a real GPU can currently hang it, and
> Windows then resets the display driver (every monitor goes black for a
> moment). To look further, use a diagnostics build with
> `VICTORY_D3D12_WARP=1`, which renders on the software adapter.

What each part does today and what it still needs:

| Subsystem | Today | Needs |
|-----------|-------|-------|
| Loader | Runs the title's executable and its DLLs in our process; crash reports name the faulting code and are kept in `crashes.log` | – |
| Runtime services | One signed-in offline user, licences, packages, saves that survive a kill at any step, text entry dialog | Speech is silent; the dialog and relaunch are untested live |
| Network | Offline by design: the title cannot reach the network | – |
| Files and streaming | Archives and streamed data read, zlib and BCPack decoded in parallel | Loading is limited by writes into title memory |
| Input | XInput pads, keyboard, keyboard stand-in pad | Trigger rumble |
| Audio | Menu, video and loading sound; XMA voices and Opus streams decode | Reverb is silent; Opus results unverified against the title |
| GPU: menus | Draws, compute and presentation on host D3D12; only changed title memory is copied | Frame pacing drifts on one screen in release builds |
| GPU: world | Mesh pipelines, shader-written buffers, descriptor lifetimes, indirect execution (arguments clamped), pipelines made by worker threads and kept on disk | Most of the scene is still missing (untranslated shaders drop draws); frames take seconds; real queries, depth bounds, HDR and scaling are missing |
| Shaders | Console GPU code recompiled to DXIL at first use, cached on disk. Menus: all. World: mesh 8,191 of 8,506, vertex/pixel 10,617 of 12,346, compute 1,643 of 2,539 | 64-bit image atomics, merged geometry stages, the ray-tracing builder's shaders; some compute programs need a GPU with 64-lane waves |
| Textures | Console tiled layouts and detiling for the modes the title uses | Render-target metadata planes |
| Series S mode | Runs through the menus to the loading screen | The world, as above |

## Getting started

Requirements: Windows 10/11 x64, a CPU with AVX2, a Direct3D 12 GPU with
shader model 6.6 (and mesh shaders for the world), Rust (pinned in
`rust-toolchain.toml`).

The title also needs several GiB of available Windows commit at startup.
Check **Committed** under **Task Manager → Performance → Memory**. If
`crashes.log` reports Windows error 1455, close other applications or increase
the Windows paging file size, then retry.

```powershell
cargo xtask install-hooks                # asset-policy check on every commit
copy victory.example.toml victory.toml   # then set your paths (outside the repo)
cargo test --workspace                   # needs no game data
cargo run -p victory -- config           # check your paths
```

Run the title:

```powershell
cargo build --release -p victory
target\release\victory.exe run <game dir>\<the title's exe> --execute
```

The window caption shows the framerate. Saves, settings and `crashes.log`
are kept under the data folder (default `%LOCALAPPDATA%\victory\data`), derived
data under the cache folder.

Input: XInput pads. With none connected the keyboard is a pad while the
title's window has focus:

| Keys | Pad |
|------|-----|
| W A S D / I J K L | left / right stick |
| arrows | D-pad |
| Enter, Backspace, X, Y | A, B, X, Y |
| Q / E, Z / C | shoulders, triggers |
| F / H | stick clicks |
| Esc, Tab | Menu, View |

## Commands

```
victory inspect <exe|dll> [--imports] [--exports]
victory census [paths...] [--missing] [--strict]
victory lib-exports <import.lib> [--list]
victory shader-info [paths...] [--verbose] [--disasm]
victory translate [paths...] [--validate] [--pipelines] [--list] [--jobs N]
victory run <exe> [--execute] [--unimplemented trap|zero] [--strict]
            [--console series-x|series-s] [-- <title arguments>]
victory config
```

Logging: `VICTORY_LOG=victory_loader=trace,info` (a release build logs
info and above).

## Debugging

A release build has no debugging aids. Build them in with

```powershell
cargo build --profile diag -p victory --features diagnostics   # target\diag\victory.exe
```

| Variable | Effect |
|----------|--------|
| `VICTORY_LOG=<target>=debug,...` | debug and trace output |
| `VICTORY_STACKS=20,60` | every thread's stack at those seconds |
| `VICTORY_WATCH=<rva>,...` | logs a title u32 when it changes |
| `VICTORY_KEYS=30:Enter,35:X` | presses keys at those seconds, focus or not |
| `VICTORY_CRASH_SAVE=<rva>:<len>,...` | saves title memory to the cache folder on a crash |
| `VICTORY_CAPTURE=<dir>` | saves presented frames (powers of two, or `VICTORY_CAPTURE_FRAMES=<list>`) |
| `VICTORY_DUMP_TARGETS=1` | with the two above, saves every render target of those frames |
| `VICTORY_D3D12_DEBUG=1` | host debug layer |
| `VICTORY_D3D12_GBV=1` | with the above, GPU-based validation (bad descriptor use reported, not faulted; slow) |
| `VICTORY_D3D12_DRED=1` | on a GPU failure, the allocation hit and the commands in flight |
| `VICTORY_PIPELINE_WAIT_MS=24` | reduce the per-frame wait for background pipeline creation from the default of 500 ms. A draw can be omitted until its pipeline is ready |
| `VICTORY_SKIP_PIPELINES=<hash>,...` | omit draws and dispatches, including indirect ones, whose pipeline contains a listed 16-digit shader hash |
| `VICTORY_GPU_TIMING=1` | with the above, each list of a submission run and timed on its own; slow lists logged with their slowest commands |
| `VICTORY_D3D12_WARP=1` | host device on the software adapter (slow; cannot reset the driver) |
| `VICTORY_SHADER_LOOP_LIMIT=<n>` | translated shader loops stop after `n` iterations per lane instead of 2^20 (own shader cache entries); if a lower `n` makes world frames much faster, loops hitting the limit are what makes them slow |
| `VICTORY_AUDIO_CAPTURE=<file.wav>` | records what the title plays (our process only) |
| `VICTORY_XMA_DUMP=<dir>` | saves the first 32 XMA voices' context and input |

Every build appends crashes and host GPU failures to `crashes.log` in the
title's data folder.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
