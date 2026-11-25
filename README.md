# Neonbridge: one launcher for every GTA VI (GTA 6) Xbox Series runtime on PC

![Neonbridge: GTA 6 Xbox Series launcher for Windows PC](assets/banner.svg)

Test them all in one convenient launcher.

Neonbridge is a Windows app that runs every project trying to bring the Xbox Series version of GTA VI to PC, from one window, on a copy of the game you dumped from your own console. **None of them is playable yet.** No game files are included.

## Status

Best first, by how far each project's own authors say it gets. The launcher lists them in this order too.

| # | Runtime | How far it gets (its authors' own results) | Verified here |
|---|---|---|---|
| 1 | [Flamingo](https://github.com/realblastex/flamingo) | 9–14 fps in the first scene (9–10 since its victory-fork merge, cold cache); shadows still dark | Yes: boots through the first-boot settings into the first mission ("Go with the prison guard", player in control) at 5–7 fps; the 3D scene renders black (dark lighting, upstream) and menu text is missing glyphs |
| 2 | [VIctory-fxceless](https://github.com/fxceless/victory-fork) | About 10 fps in the first rooms; 2–4 fps in the world | Yes: intro at 30 fps, then the 3D prison scene in the first mission at about 1 fps (2,681 draws a frame), washed out with grey flicker |
| 3 | [VItoriaBr](https://github.com/uiliannn/VItoriaBr) | Up to 6 fps in the open world | Yes: renders the 3D prison scene in the first mission at 2–3 fps (2,649 draws a frame, diagnostics build); orange flame-like glitches on characters. The best result verified here so far |
| 4 | [xcl](https://github.com/ivall/xcl) | About 5 fps in the 3D world (RTX 5090, NVIDIA branch only); menus at 30 fps | Builds; its first shader preparation runs |
| 5 | [VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD) | 3–5 fps in a lit world with vehicles (AMD cards) | No |
| 6 | [ScarlettPort](https://github.com/ZigZagBoi1/ScarlettPort) | About 4 fps in the prologue; gameplay starts | Yes: intro at 30 fps, 3D scenes at 2–3 fps |
| 7 | [ztrx-victory](https://github.com/Zettrox/ztrx) | 1–3 fps in the world; shadows black | No |
| 8 | [Five Star Recomp](https://github.com/ThatRecompDev/five-star-recomp) | The opening cutscene at 30 fps; the 3D world draws, lighting unfinished | No (needs .NET 10 and the PC GDK) |
| 9 | [TuxBox](https://github.com/brcly/TuxBox) | The story intro, on Linux; Windows support is still in progress | No |
| 10 | Neonbridge (own build) | Not tested yet | No |

## Get started

1. Download the **v0.2.0-alpha** zip from [Releases](../../releases).
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

Some runtimes need one more thing. Neonbridge greys them out until it's there, and its **Install...** button helps you get it:

| Runtime | Also needs |
|---|---|
| Five Star Recomp | The [.NET 10 SDK](https://dotnet.microsoft.com/download/dotnet/10.0) and the [Microsoft PC GDK](https://github.com/microsoft/GDK/releases) (**Install...** downloads Microsoft's signed GDK setup and runs it, after you say yes) |
| TuxBox | [Python 3.10 or newer](https://www.python.org/downloads/windows/) and the [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) (C++ tools) |

## Screenshots

| | |
|---|---|
| ![The launcher: runtimes, details, preset and the game](assets/picker.png) | ![Advanced: mix mode and the run options](assets/mix.png) |
| **The launcher.** Pick a runtime and a preset, then press Play. | **Advanced.** Mix parts of different runtimes, or set options for a run. |
| ![The loader preparing the shaders](assets/loader.png) | ![The controls overlay](assets/controls.png) |
| **The loader.** Shows what's happening before the game starts. | **On-screen keys and frame rate.** |

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

GPL-3.0-or-later ([LICENSE](LICENSE)). Each runtime keeps its own licence (`vendor/<name>/LICENSE`): xcl is GPL-2.0-only, the VIctory forks (Flamingo and VIctory-Belobug-AMD among them) and ScarlettPort are GPL-3.0, TuxBox is Apache-2.0 and Five Star Recomp is GPL-3.0-or-later. Grand Theft Auto, GTA and Rockstar Games are trademarks of Take-Two Interactive; Xbox is a trademark of Microsoft. Neonbridge isn't affiliated with or endorsed by them.

<sub>Keywords: GTA 6 PC, GTA VI PC, GTA 6 launcher, Grand Theft Auto VI Windows, Xbox Series X|S to PC, Scarlett, VIctory, Flamingo, Belobug, ScarlettPort, xcl, TuxBox, Five Star Recomp.</sub>
