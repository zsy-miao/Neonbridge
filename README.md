# Neonbridge: one launcher for every GTA VI (GTA 6) Xbox Series runtime on PC

![Neonbridge: GTA 6 Xbox Series launcher for Windows PC](assets/banner.svg)

Test them all in one convenient launcher.

Neonbridge is a Windows app that runs every project trying to bring the Xbox Series version of GTA VI to PC, from one window, on a copy of the game you dumped from your own console. **None of them is playable yet.** No game files are included.

## Standings

Read from the [GTA VI PC race tracker](https://github.com/vuyp/gta6-pc-race) ([live page](https://vuyp.github.io/gta6-pc-race/)), which ranks every open-source project by its own public claims. **Nothing is playable yet**: the tracker's playable bar is 20 fps and 30 minutes without a crash. 🟢 = in Neonbridge (pick it in the launcher).

Lanes, as the tracker reads them: **Picture** (1 mostly dark, 3 lit with faults, 5 the console's picture), **Speed** (fps reported in play), **Stability** (longest session without a crash).

- **1.** 🟢 **[victory-fork](https://github.com/fxceless/victory-fork)**  
  shadows black · 4.5 and 7.6 fps, averages of two play sessions · 30 minutes, in a pull request
- **2.** 🟢 **[AnyXboxX](https://github.com/LooperSalty/AnyXboxX)**  
  even stand-in light indoors, some parts missing · about 12 fps median on its new base · 45 minutes, relayed, on its earlier base
- **3.** 🟢 **[klbf-lab](https://github.com/PastiiJay/klbf-lab)**  
  lit, with tints and faint lines · 5 to 9 fps by scene · 77 minutes, ended by choice
- **4.** 🟢 **[VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD)**  
  lit and whole, stand-in light follows the sky · 5 to 8 fps, less while driving · a 15 minute drive
- **5.** [ztrx](https://github.com/Zettrox/ztrx) (repository gone)  
  shadows black · 1 to 3 fps · over 90 minutes
- **6.** 🟢 **[VItoriaBr](https://github.com/uiliannn/VItoriaBr)**  
  lit with a shaded stand-in light · 3.5 to 6 fps · no run length reported
- **7.** [victory-perf](https://github.com/vitutu1/victory-perf) (repository gone)  
  shade lit by stand-ins · 4.5 to 14 fps by scene, unconfirmed · no run length reported
- **8.** 🟢 **[ScarlettPort](https://github.com/ZigZagBoi1/ScarlettPort)**  
  shadows dark · about 4 fps · no run length reported
- **9.** omonkey  
  lit, major effects missing · about 5 fps · no run length reported
- **10.** suri  
  close to the retail game · about 3 fps · no run length reported
- **11.** CGBJ  
  unlit · about 4 fps · no run length reported
- **12.** [victory-amd](https://github.com/myokex/victory-amd)  
  walls or lamps missing at times · 2 to 3 fps · 8 minute runs
- **14.** 🟢 **[Flamingo](https://github.com/realblastex/flamingo)**  
  shadows missing, rooms dark · 7.8 to 14 fps by scene, unconfirmed · a 24 minute play session
- **21.** 🟢 **[victory-unified](https://github.com/Itsbatt/victory-unified)**  
  shade lit by a stand-in light · 2 to 3 fps · about 58 minutes, no crash
- **23.** 🟢 **[VI-BR-PORT](https://github.com/edsuuu/VI-BR-PORT)**  
  shadows black, some skin black · 4 to 7 fps at 1080p · sessions of an hour

25 projects are in-game (stage 6 of 7) and 12 more show a recognisable picture (stage 5); the full list is on the tracker.

### Verified here

Tested on our PC: Intel Core i5-14400F (10 cores), NVIDIA GeForce RTX 4060 8 GB (driver 616.92), 16 GB RAM, Windows 11. Most authors test on faster cards (RTX 4070 to 5090, RX 9070 XT), so expect lower frame rates here than theirs. The launcher lists the runtimes in this order.

1. **[KLBF Lab](https://github.com/PastiiJay/klbf-lab)**: No (not yet tested on NVIDIA by its author)
2. **[Flamingo](https://github.com/realblastex/flamingo)**: Yes: boots through the first-boot settings into the first mission ("Go with the prison guard", player in control) at 5–7 fps; the 3D scene renders black (dark lighting, upstream) and menu text is missing glyphs
3. **[VIctory-fxceless](https://github.com/fxceless/victory-fork)**: Yes: intro at 30 fps, then the 3D prison scene in the first mission at about 1 fps (2,681 draws a frame), washed out with grey flicker
4. **[VI-BR-PORT](https://github.com/edsuuu/VI-BR-PORT)**: **Does not work on our PC yet**, with or without Neonbridge: run alone exactly as its README says, the game still stops itself about 20 s in, every time (it asks for graphics-device method 102, which VI-BR-PORT does not have). Its author tests on an RTX 4060 Ti; ours is an RTX 4060
   - The launcher's crash reason (latest run):
     - **The game stopped itself:** breakpoint `0x80000003` at `victory-image.dll+0x56abfa5`, i.e. its own check failed.
     - **Just before:** it called graphics-device method 102 (`slot 0x330`, from the game's code at `+0x75962`), which the runtime does not implement.
     - The game then tried to upload its crash report; the runtime blocked it (no network).
     - Upstream has no fix yet (no fork implements method 102).
5. **[VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD)**: No
6. **[VItoriaBr](https://github.com/uiliannn/VItoriaBr)**: Yes: renders the 3D prison scene in the first mission at 2–3 fps (2,649 draws a frame, diagnostics build); orange flame-like glitches on characters. The best result verified here so far
7. **[ScarlettPort](https://github.com/ZigZagBoi1/ScarlettPort)**: Yes: intro at 30 fps, 3D scenes at 2–3 fps
8. **[victory-unified](https://github.com/Itsbatt/victory-unified)**: **Testing now** (just added; its author: about 58 min without a crash on an RTX 3070 8 GB)
9. **[AnyXboxX](https://github.com/LooperSalty/AnyXboxX)**: **Testing now** (just added; its author: about 12 fps in the world on an RTX 4070 SUPER)

xcl is archived (28th on the race): `git checkout 5a8260eeef4a0308a5048d5d382b10564e58a3cf -- vendor/xcl` brings it back. ztrx-victory (Zettrox/ztrx, upstream now gone) is archived too: `git checkout 2210a25024c0ffedc232661f7b312c06e88c86f2 -- vendor/ztrx-victory` brings it back.

## Get started

1. Download the **v0.3.0-alpha** zip from [Releases](../../releases).
2. Unzip it anywhere, for example `C:\Games\Neonbridge`.
3. Run `neonbridge.exe`.
4. If it doesn't find your game folder by itself, click **Change** and choose it. Then pick a runtime and press **Play**.

The first start of a runtime takes a while: it builds the runtime and prepares the game's shaders. The loading screen shows real counts for each step (for example "translating shaders: 1,000 / 26,503"), and a moving pulse where a runtime gives no count. Later starts are quicker: a runtime that hasn't changed isn't built again.

## What you need

- **Your own copy of the game:** the Xbox Series X|S version of GTA VI, dumped from a console you own. It's the folder with `game_scarlett_master.exe` in it, about 210 GB.
- **Windows 10 or 11**, 64-bit, with a processor that supports AVX2.
- **A graphics card** that supports DirectX 12 with mesh shaders (Shader Model 6.6), with an up-to-date driver.
- **Memory:** 32 GB of RAM is recommended.
- **Disk space:** the game, plus up to about 20 GB per runtime for its files and shaders.


## Screenshots

**The launcher.** Pick a runtime and a preset, then press Play.

![The launcher: runtimes, details, preset and the game](assets/picker.png)

**Advanced.** Mix parts of different runtimes, or set options for a run.

![Advanced: mix mode and the run options](assets/mix.png)

**The loader.** Shows what's happening before the game starts.

![The loader preparing the shaders](assets/loader.png)

**On-screen keys and frame rate.**

![The controls overlay](assets/controls.png)

How each runtime is built, prepared and started: [WIRING.md](WIRING.md).

## Mix and match (help wanted)

**Advanced** in the launcher builds one runtime from parts of several (VIctory-fxceless, VItoriaBr, ScarlettPort, Flamingo, KLBF Lab, Belobug). These mixes **compiled** in our own build check (made against older copies of each runtime). **None has been run in the game yet**, by us or by the runtimes' authors, so please try them and open an issue with what you see.

| Main program | Part swapped in | From |
|---|---|---|
| VIctory-fxceless | console system layer (`victory-hle`) | VItoriaBr |
| VIctory-fxceless | game file loader (`victory-loader`) | VItoriaBr or ScarlettPort |
| VIctory-fxceless | program loader (`victory-pe`) | VItoriaBr |
| VIctory-fxceless | `victory-core`, `victory-game` or `victory-formats` | ScarlettPort |

The shader translator, shader cache reader and pipeline builder (`victory-shader`, `victory-gpu`, `victory-pipeline`) always come from the main program, so its shader cache keeps working. **No mix has been tested yet.** Mixes with Flamingo, KLBF Lab or Belobug parts haven't even been compile-checked. A mix that fails to compile is remembered and refused next time.

## Watch on your phone (Tailscale)

Hand this to your AI agent (Claude Code, Codex and the like), or run it yourself in a terminal in the Neonbridge folder:

```powershell
.
eonbridge.exe --feed-only
```

1. It prints `Tailscale sign-in needed: open <link>`. Open that link, sign in to your own free Tailscale account and approve the PC (once; nothing to install).
2. It prints `live at https://neonbridge.<your-tailnet>.ts.net`. Open `http://127.0.0.1:8788` on this PC first and set the stream password.
3. Install the Tailscale app on your phone, sign in to the same account, then open the `ts.net` address.
4. Leave that window open and start games with `NEONBRIDGE_FEED=1` set (or skip `--feed-only` and launch with it set: the stream starts with the game). `NEONBRIDGE_FEED_SCREEN=1` streams your whole left monitor instead of the game window.

## Keys

| Key | Does |
|---|---|
| **F1** | Shows or hides the controls |
| **F2** | Hides the controls |
| **F3** | Shows or hides the frame rate |

Each runtime has its own keyboard and mouse layout. F1 shows it in game.

## Where things are

Everything Neonbridge keeps is in `%LOCALAPPDATA%\6ixpack`. Paste that into the Explorer address bar to open it.

| What | Folder |
|---|---|
| Saves | `6ixpack\save` |
| Shaders and other caches | `6ixpack\cache`, and `6ixpack\<runtime>\cache` |
| Logs | `6ixpack\logs` |

## Troubleshooting

- **The game crashed or closed.** Every run is logged, so there is always a log to send. Neonbridge shows its location. Send that file from `%LOCALAPPDATA%\6ixpack\logs` (the newest one) when you report the problem.
- **A runtime is greyed out.** It needs something that isn't installed yet. Select it to see what, then click **Install...**.
- **The first start is slow.** That's the shader preparation. It's only slow the first time.
- **It says no game was found.** Click **Change** and choose the folder that holds `game_scarlett_master.exe`.

## FAQ

- **Can I play GTA 6 on PC with this?** Not yet. Every runtime here is still a work in progress.
- **Is this a download or a crack?** No. There are no game files, keys or protection bypasses. You need your own dump.
- **Whose work is this?** Mostly the authors of the projects it runs. Neonbridge is the launcher around them. See [CREDITS.md](CREDITS.md).
- **Does it go online?** Only when you say yes, for example to an **Install...** download.

<details>
<summary><b>For developers</b></summary>

Building from source, the command line, runtime settings, patches and environment variables are in [DEVELOPERS.md](DEVELOPERS.md).

</details>

## Credits

Every runtime and every project Neonbridge borrows from is named in [CREDITS.md](CREDITS.md), with its authors, licence and version. Thanks especially to [realblastex](https://github.com/realblastex) for [Flamingo](https://github.com/realblastex/flamingo) and to [Belobug](https://github.com/Belobug) for [VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD).

## Licence

GPL-3.0-or-later ([LICENSE](LICENSE)). Each runtime keeps its own licence (`vendor/<name>/LICENSE`): xcl is GPL-2.0-only, the VIctory forks (Flamingo and VIctory-Belobug-AMD among them) and ScarlettPort are GPL-3.0, and Neonbridge's own TuxBox-based code is Apache-2.0. Grand Theft Auto, GTA and Rockstar Games are trademarks of Take-Two Interactive; Xbox is a trademark of Microsoft. Neonbridge isn't affiliated with or endorsed by them.

<sub>Keywords: GTA 6 PC, GTA VI PC, GTA 6 launcher, Grand Theft Auto VI Windows, Xbox Series X|S to PC, Scarlett, VIctory, Flamingo, Belobug, ScarlettPort, xcl, ztrx.</sub>
