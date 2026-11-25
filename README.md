# Neonbridge: one launcher for every GTA VI (GTA 6) Xbox Series runtime on PC

![Neonbridge: GTA 6 Xbox Series launcher for Windows PC](assets/banner.svg)

Test them all in one convenient launcher.

Neonbridge is a Windows app that runs every project trying to bring the Xbox Series version of GTA VI to PC, from one window, on a copy of the game you dumped from your own console. **None of them is playable yet.** No game files are included.

## Status

| Runtime | Builds | Starts | Intro | 3D scene | Playable | Notes |
|---|---|---|---|---|---|---|
| ScarlettPort | ✅ | ✅ | ✅ | ⚠️ | ❌ | Intro at 30 fps; 3D scenes at 2–3 fps |
| VItoriaBr | ✅ | ✅ | ✅ | ⚠️ | ❌ | Stops in 3D scenes; a fix is coming |
| victory (fxceless) | ✅ | ❌ | ❌ | ❌ | ❌ | Not tested yet |
| ztrx-victory | ✅ | ❌ | ❌ | ❌ | ❌ | Not tested yet |
| xcl | ✅ | ❌ | ❌ | ❌ | ❌ | Not tested yet |
| Neonbridge (own build) | ✅ | ❌ | ❌ | ❌ | ❌ | Not tested yet |
| TuxBox (upstream) | ⚠️ | ❌ | ❌ | ❌ | ❌ | Its authors say Windows support is still in progress |
| Five Star Recomp | ❌ | ❌ | ❌ | ❌ | ❌ | Not in the download yet; needs .NET 10 and the PC GDK |

## Get started

1. Download the **v0.2.0-alpha** zip from [Releases](../../releases).
2. Unzip it anywhere, for example `C:\Games\Neonbridge`.
3. Run `neonbridge.exe`.
4. If it doesn't find your game folder by itself, click **Change** and choose it. Then pick a runtime and press **Play**.

The first start of a runtime takes a while: it prepares the game's shaders. Later starts are quicker.

## What you need

- **Your own copy of the game:** the Xbox Series X|S version of GTA VI, dumped from a console you own. It's the folder with `game_scarlett_master.exe` in it, about 210 GB.
- **Windows 10 or 11**, 64-bit, with a processor that supports AVX2.
- **A graphics card** that supports DirectX 12 with mesh shaders (Shader Model 6.6), with an up-to-date driver.
- **Memory:** 32 GB of RAM is recommended.
- **Disk space:** the game, plus up to about 20 GB per runtime for its files and shaders.

Some runtimes need one more thing. Neonbridge greys them out until it's there, and its **Install...** button helps you get it:

| Runtime | Also needs |
|---|---|
| TuxBox (upstream) | [Python 3.10 or newer](https://www.python.org/downloads/windows/) and the [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) (C++ tools) |
| Five Star Recomp | The [.NET 10 SDK](https://dotnet.microsoft.com/download/dotnet/10.0) and the [Microsoft PC GDK](https://github.com/microsoft/GDK/releases) |

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

- **The game crashed or closed.** Neonbridge shows the log's location. Send that file from `%LOCALAPPDATA%\6ixpack\logs` (the newest one) when you report the problem.
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

Every runtime and every project Neonbridge borrows from is named in [CREDITS.md](CREDITS.md), with its authors, licence and version.

## Licence

GPL-3.0-or-later ([LICENSE](LICENSE)). Each runtime keeps its own licence (`vendor/<name>/LICENSE`): xcl is GPL-2.0-only, the VIctory forks and ScarlettPort are GPL-3.0, TuxBox is Apache-2.0 and Five Star Recomp is GPL-3.0-or-later. Grand Theft Auto, GTA and Rockstar Games are trademarks of Take-Two Interactive; Xbox is a trademark of Microsoft. Neonbridge isn't affiliated with or endorsed by them.

<sub>Keywords: GTA 6 PC, GTA VI PC, GTA 6 launcher, Grand Theft Auto VI Windows, Xbox Series X|S to PC, Scarlett, VIctory, ScarlettPort, xcl, TuxBox, Five Star Recomp.</sub>
