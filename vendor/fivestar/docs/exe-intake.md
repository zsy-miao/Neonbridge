# Executable intake

What to run first when the game executable (`game_scarlett_master.exe`) becomes available. The goal of the first hour
is a report, not exploration: one command reads the executable and writes a readable summary of what the port still
needs. The tool runs on your own copy of the executable; it is not in this repository, and nothing it writes is.

```
fivestar exe analyze <exe> [--dump <folder>] [--gdk-lib <xgameruntime.lib>] [--out <dir>] [--loader <fivestar-loader>]
```

- `<exe>` is the executable to read.
- `--dump` is your dump folder; its `*.rpf` names seed the archive-reference search. Defaults to `FIVESTAR_DUMP`.
- `--gdk-lib` is the PC GDK `xgameruntime.lib` (`windows\lib\x64`); it names the GDK thunks, which are found and counted
  without it. Defaults to `FIVESTAR_GDK_LIB`.
- `--out` is where the report goes (default `fivestar-exe-report` in the current directory).
- `--loader` points at `fivestar-loader`; by default it is found next to this tool's build output, or through `FIVESTAR_LOADER`.

Two files are written to `--out`: `<exe>.report.md` (for reading) and `<exe>.report.json` (for tooling). Everything the
run produces stays in that folder.

## Reading the report

| Section | What it answers |
|---|---|
| PE basics | sections (with entropy, so a packed or encrypted section stands out), entry point, TLS callbacks, `.pdata` function count, exports, and any non-standard or console section such as `.xbld` |
| Imports against the loader | every imported function, how the loader resolves it, and how many call sites reach it. Gaps (`XboxStub`, `Missing`) first, by call sites |
| GDK call sites | the GDK functions the game calls through the `QueryApiImpl` thunks: (clsid, iid, slot), the thunk's RVA, its direct call sites, and the ones without a shim. Needs `--gdk-lib` to put names to the thunks |
| D3D12.X and graphics | imports from `d3d12*`/`dxgi`, the D3D12.X (`d3d12_xs.dll`) entry points, and how the loader resolves them |
| Archive file tables | where the executable uses the `RPF9` magic (0x39465052) and the archive names, as a pointer into the archive-parsing code |
| Shader linkage | references to `shaders.rpf` (the shader container and the pipeline cache entry it holds) |
| Day one gaps | an ordered list of what blocks the game from running |

The import classification is the loader's own, not a second copy of the list: `fivestar exe analyze` runs
`fivestar-loader --classify` over the import names and reads its verdicts back, so the report and the loader always
agree. If the loader cannot be found the report still prints, with the import classification left out. The folder the
executable is in counts as the package folder (`--search`).

| Class | Meaning |
|---|---|
| `Shim` | a loader shim answers it |
| `XboxStub` | a module the loader declares Xbox-only, function without a shim: a logged stub returning a fixed value |
| `Package` | a module in the package folder that the loader maps itself: the shipped CRT, or any module the PC does not have |
| `Host` | the PC has the module: a System32 file or an API set the OS implements |
| `Missing` | none of the above: an Xbox system module the loader does not know yet; its imports stay unresolved |

`Host` and `Missing` are facts about the machine the report runs on, so an import that is `Host` here can be
`Missing` on a PC without that DLL (Media Foundation on Windows N editions, for example).

## Why the loader library

The thunks are found without the library (see [platform.md](platform.md), "GDK thunk ABI"): `libHttpClient.GDK.dll`
shows 404 distinct thunks and `game_scarlett_master.exe` 161, reached from 358 direct call sites. The library puts
names to them, so the report can flag the ones the loader does not yet shim.

## Archive file tables

The report locates the archive code; it does not open the archives. The RPF9 table of contents and name table are
encrypted and that encryption is not solved (see [archives.md](archives.md), "Unknown"). The `RPF9` and archive-name
references are a starting point for reading the parser, nothing more: the tool recovers no keys and decrypts nothing.
