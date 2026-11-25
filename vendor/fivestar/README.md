<p align="center"><img src="docs/assets/banner.svg" alt="Five Star Recomp" width="640"></p>

Five Star Recomp is an effort to run the Xbox Series version of **Grand Theft Auto VI** natively on a Windows PC,
from a copy of the game you own. It targets the Xbox Series release, version 1.0.2.1 (title ID `7952987A`).

The Xbox Series uses a regular x86-64 processor, so GTA VI's own code can run on a PC as it is. What does not
carry over is everything around it: the console's operating system services, its graphics API, its GPU shaders and
its data formats. This project rebuilds those pieces:

- **Shaders.** The game ships its GPU programs only as native AMD RDNA machine code. A translator turns each one
  into HLSL, which the standard DirectX compiler builds for any PC graphics card.
- **Platform.** A loader maps the game's modules on Windows and answers their calls to console-only system
  libraries with PC implementations.
- **Assets.** Readers for the game's archives, textures and metadata, so the data can be inspected and used.

No game files, keys or extracted data are part of this repository. Every tool runs on your own console dump.

## Status

GTA VI does not run yet. Its main executable is now available: under the loader it starts, runs through its memory
set-up, archive opening and graphics device set-up, presents frames to its own window, shows its logo, intro art and
first-run screens, loads story mode and plays the opening cutscene's art, credit cards, dialogue and sound at 30 frames
a second. The cutscene's 3D world now draws into the G-buffer (geometry is in place), but lighting and composition do
not reach the screen yet. Details and numbers are
in the
[wiki](https://github.com/ThatRecompDev/five-star-recomp/wiki/Status).

| Area | State |
|---|---|
| Shader translation | Every shader stage the game uses is translated (vertex, pixel, compute, mesh, geometry, ray intersection), and all 25,908 programs compile. Branches become structured `if`/`else` and loops, 39% smaller than the older dispatch loop and checked to give the same results on real programs. Instruction behaviour is checked on D3D12 WARP. Shaders are translated when the game first uses them and compiled by a DXIL backend of our own, 5 to 20 times faster than dxc, in a separate compile process; dxc remains the fallback. |
| Archives | Contents can be extracted without the encrypted file table (no names yet). Metadata files parse completely. |
| Textures | Every texture decodes exactly with the BCPack tables, which you supply yourself (`fivestar setup --bcpack-tables`); without them 99.44% are readable and BC7, BC6H and BC3 (about 2,000 textures) are not. |
| Models | Drawable geometry (meshlets, packed vertices) decodes and exports as OBJ ([docs/models.md](docs/models.md)). |
| Loader | The game executable and its support libraries load with all imports resolved; the game boots through its start-up, answers its first-run screens through a scripted gamepad ([docs/first-run.input](docs/first-run.input)) and starts loading story mode. The console runtime, audio decoding and system libraries are implemented for offline use. |
| Graphics API | The game's D3D12.X device runs on desktop D3D12: its pipelines build from the translated shaders (with a disk cache), its descriptors and barriers are translated, and it presents its logo, intro video and menus with complete UI text. Shaders that read memory through console GPU addresses reach the game's memory through an address map. The GPU hang after the story load (shader loops that never ended) is bounded; open: the story load itself, which stops on corrupted game memory ([docs/d3d12x-shim.md](docs/d3d12x-shim.md)). |

The [wiki](../../wiki) has the details, research notes and the current progress report.

## Getting started

You need the .NET 10 SDK. Shader work also needs the DirectX shader compiler (`dxc`, part of the Windows SDK). The tools
disassemble RDNA code in-process; LLVM 18 or newer with the AMDGPU target (`llvm-mc`, `llvm-objdump`) is needed only
to change a shader test program and for `fivestar shader decode-check`.

```
dotnet build FiveStar.slnx
dotnet test
```

### Preparing your dump

```
fivestar setup <your dump folder>
```

One command prepares everything the port needs: it checks the dump, extracts and translates the shaders, reads the
console pipeline cache and exports the textures into `%LOCALAPPDATA%\FiveStar\<titleid>-<version>\` (`--work` picks
another folder). The first run takes one to several hours, depending on the processor, and needs over 100 GB free; a
re-run skips finished steps, and a run that was stopped continues where it left off. `fivestar setup <dump> --status` shows what is done. See
[docs/setup.md](docs/setup.md).

### Running the game

```
fivestar doctor <your dump folder>
fivestar run <your dump folder>
```

`doctor` checks what a run needs (the PC GDK, dxc, disk space, the loader). `run` finishes any setup step still to do,
then starts the game under the loader and prints a summary when it exits. The game does not get past its first screens
yet. See [docs/running.md](docs/running.md).

### Single tools

The `fivestar` command holds all tools; `fivestar <group> help` lists a group's commands.

```
fivestar archive carve <archive.rpf> <out-folder>     extract an archive's contents
fivestar audio export <audio.rpf> <out-folder>        export sounds as Ogg Opus and WAV
fivestar texture extract <file.ttpf> <out-folder>     export textures as DDS
fivestar exe analyze <exe> [--dump <dir>] [--gdk-lib <lib>]   report what the port needs (docs/exe-intake.md)
fivestar shader extract <shaders.rpf> <out-folder>    extract the GPU programs
fivestar shader translate <programs> <out-folder>     translate them to HLSL and compile
fivestar shader plan <out-folder>                     which DXIL each of your GPUs runs, and what it cannot run correctly
```

The loader is a separate program, `fivestar-loader`; see [docs/loader.md](docs/loader.md).

## Repository layout

| Folder | Contents |
|---|---|
| `src/FiveStar.Formats` | archive, texture, model, metadata and platform file readers |
| `src/FiveStar.Shaders` | shader containers and the RDNA to HLSL translator |
| `src/FiveStar.Loader` | the module loader and the console runtime replacements |
| `src/FiveStar.Graphics` | D3D12 devices, the root signature of translated programs, shader variant selection, pipelines and rendering |
| `src/FiveStar.Cli` | the `fivestar` command |
| `tests/` | tests per project, plus the code-rule checks |
| `docs/` | one page per format and tool |
| `docs/wiki/` | the source of the project wiki: status, roadmap and research notes |
| `vendor/` | third-party test data, each folder with its provenance and licenses |

## Contributing

Contributions are welcome. [CONTRIBUTING.md](CONTRIBUTING.md) explains the ground rules, the code style and how
to submit a change.

## License

GNU General Public License v3.0 or later. See [LICENSE](LICENSE). Parts based on MIT-licensed projects are listed in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

Five Star Recomp is a fan project. It is not affiliated with or endorsed by Rockstar Games, Take-Two Interactive or
Microsoft. Grand Theft Auto and Grand Theft Auto VI are trademarks of Take-Two Interactive. You need your own copy of
the game; this repository contains none of its content.
