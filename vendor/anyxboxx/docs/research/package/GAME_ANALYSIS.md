> Français : [ANALYSE_JEU.md](../../fr/recherche/paquet/ANALYSE_JEU.md)

# GTA VI on Xbox Scarlett — what the 1.0.2.1 package reveals

The package separates the common resources, the story mode data and those of a mini-game. The limits of the resource blocks can be recovered and part of them can be decompressed. The mission logic remains unknown. The layout names a main executable that is absent at its destination, and no readable mission script has been rebuilt from the two script archives.

## Reproducing the results

From the folder that contains `MicrosoftGame.Config`, with Python 3.10 or later. Verified locally with Python 3.12.

```powershell
# JSON inventory per file, with its chunk, its presence and its size
python .\analyse_paquet.py inventaire .

# Examine the ETPF index block, then the first data block of the interface
python .\analyse_paquet.py bloc .\data\ttdb\ui\platform.ttpf 0
python .\analyse_paquet.py bloc .\data\ttdb\ui\platform.ttpf 1

# Optional: save the decompressed bytes to a new file
python .\analyse_paquet.py bloc .\data\ttdb\ui\platform.ttpf 1 --sortie .\platform-bloc-1.bin

# Find the candidate headers of the two script archives
python .\analyse_paquet.py reperes-rpf .\script.rpf .\pack\story000\story000_script.rpf
```

## Game layout on disk

Matching the layout destinations against the disk gives **103 files present out of 104**, spread over **25 chunks**. The only missing file is `game_scarlett_master.exe` in chunk 1001. Both manifests name this same file and declare version `1.0.2.1`. `MicrosoftGame.Config` specifies the `Scarlett` target. These identifiers describe the local package. Its origin has not been authenticated.

| Resources | Local observation | What can be deduced |
| --- | --- | --- |
| Common world | `levels.rpf` takes 70.92 GiB. `navmeshes.rpf` has its own chunk. The TTDB subfolders contain eight fragments `ttdb_maps_0_7` to `ttdb_maps_7_7` and eight fragments `ttdb_terrain_0_7` to `ttdb_terrain_7_7`. | The distribution separates levels, navigation and terrain. The mapping between fragments and districts remains unknown. |
| Story | Chunk 3000 groups `story000.rpf` (18.65 GiB), `story000_script.rpf` and three TTPFs dedicated to maps, terrain and visual effects. Chunk 1014 contains `script.rpf`. | The story scripts have an archive separate from the common archive. The grouping describes the installation. The in-game loading order remains unknown. |
| Mini-game | Chunk 3003 groups `shared_minigame000.rpf` with its own map and terrain TTPFs. | The mini-game has dedicated world resources. Its name, rules and flow remain undetermined. |
| Audio and video | `audio.rpf` takes 26.58 GiB and `movies.rpf` 8.23 GiB. The export table of `Bink2Scarlett.dll` contains `BinkOpen`, `BinkDoFrame` and `BinkNextFrame`. | The package ships a library able to open and decode video sequences. |
| Rendering | The two FidelityFX DLLs export `ffxCreateContext` and `ffxDispatch`, among others. The exports of `XboxBvhBuilder.dll` include `BuildRaytracingAccelerationStructure` in their decorated C++ names. | Interfaces for upscaling and for building acceleration structures are shipped. The settings active in game remain unknown. |

## Decoding the TTPF containers

The details of the ETPF fields and the commands of the dedicated reader are in [ETPF_ANALYSIS.md](ETPF_ANALYSIS.md).

The **57 TTPFs** declared in the layout total **62,217,061,888 bytes**. The reader verified **1,026,890 intervals** on their tables, including those of the `pack` subfolders. The check covers every bound. The decoding of the data was sampled separately.

| Position | Interpretation verified on this copy |
| --- | --- |
| `0x00` | ASCII signature `RDS5`. |
| `0x04` | 32-bit little-endian integer, number of rows in the table. Two successive rows delimit an interval. |
| `0x08` | The 56 low-order bits of the 64-bit word give the final logical size, in bytes. |
| `0x10` | Start of the table. For each 64-bit little-endian word `w`, the position on disk is `(w & 0xffffff) * 256`. The position in the logical data is `((w >> 30) & 0x3fffff) * 4096`. |

In the 57 files, both series of positions increase strictly. The first physical position follows the table rounded up to a multiple of 256 bytes. It points to the `ETPF` signature. The last physical position matches the file size exactly. The interval with index 0 contains the ETPF header. The following data starts at index 1.

**Bit 29** of a row identifies the zlib blocks in the examined sample. On block 1 of each of the 57 files, the bit is set 52 times. In all 52 cases, the decoder reaches the end of the zlib stream and gets exactly the size predicted by the logical positions. The other five blocks have a still undetermined encoding. The reader then returns their prefix and their bounds. It applies a limit of 64 MiB per block to decompress.

A second reproducible sample, drawn with `random.seed(42)` and up to 40 data intervals per TTPF, covered 2,160 blocks whose stored and logical sizes do not exceed 2,000,000 bytes. The 1,889 blocks whose bit 29 is set produced a complete zlib stream of the expected size. The other 271 were not recognized as zlib. This result describes the sample, not the whole set of blocks.

**Verifiable example.** In `data/ttdb/ui/platform.ttpf`, interval 0 goes from `0x100` to `0x1100` excluded. Interval 1 starts at `0x1100` and ends at `0x36700` excluded. Of its 218,624 bytes, 218,411 belong to the zlib stream and 213 follow it. Decompression produces **524,288 bytes**, whose SHA-256 is `4887713f84d6184cc6b4ded7e90d3d6495a66c173840f6972a2d70100d9b61d6`. The fingerprint allows verifying the export independently. The visual format of the block remains to be identified.

## Script archives

The survey of the RPF fields and the header reader are in [RPF_RESEARCH.md](RPF_RESEARCH.md).

The 22 RPF files of the layout start with the bytes `39 46 50 52`, which is `9FPR` in reading order. Inside the two script archives, positions aligned on 4,096 bytes reproduce this signature and the eight bytes `00 7a 04 00 50 01 00 00` at relative position `0x18`.

| Archive | Candidate positions in bytes from the start of the file |
| --- | --- |
| `script.rpf` | 0, 4,096, 802,816 |
| `pack/story000/story000_script.rpf` | 0, 4,096, 98,304, 51,539,968, 77,819,904 |

Position zero is the external header. The internal repetitions are compatible with nested containers. The filter provides points where the analysis can resume. Deducing a number of missions would require identifying the directory entries, then the bytecode associated with each.

## What the Windows file actually launches

The static disassembly of the .NET code of `Lancer GTA VI.exe` shows three methods that explain its behavior.

- `Main` reads a path stored in `jeu.txt`. The Shift key allows going back to the file picker. The method can save the new choice there.
- `CanLaunch` checks that the target exists and has the `.exe` extension. It excludes the launcher itself and any file named `game_scarlett_master.exe`. Name comparisons ignore case.
- `RunGame` prepares the working directory and calls `System.Diagnostics.Process.Start` at IL instruction `0033`. The role of the launcher is therefore to start a Windows executable selected by the user.

## Binary configuration and resources

`microsoft_gameconfig.bin` starts with `regf` and its structure could be read as a Windows registry hive with `python-registry`. It contains, among others, the same name, publisher, version, StoreId, Scarlett target and executable as `MicrosoftGame.Config`. The fields `PersistentLocalStorage\\SizeMB` and `VirtualMachine\\SMTEnabled` are 2048 and 1 respectively. These data describe the declared configuration of the package.

`resources.pri` starts with `mrm_pri2` and contains the UTF-16 strings `ms-appx://Jim_vesz1v3mcwykm/` and `Jim_vesz1v3mcwykm`. These strings differ from the identity name present in the manifests. Their presence signals a metadata discrepancy whose cause and consequences have not been established. `Catalog000.bin` has an x64 PE structure with one section and a null entry point. Its internal content remains to be identified.

## Method and role of REA

Once network access was restored, REA 6.10.1 was run with Node 24.22.1 in a temporary environment. It inspected the folder and the .NET methods of the launcher. The results and the coverage are in [REA_RESEARCH.md](REA_RESEARCH.md). The block measurements come from the local Python reader, and the library exports from their PE table. [REA guide for installations](https://github.com/morluto/rea/blob/main/docs/desktop-applications.md) · [REA guide for .NET assemblies](https://github.com/morluto/rea/blob/main/docs/guides/managed/managed-code-analysis.md)

With the REA CLI installed, these commands documented by the repository allow resuming the inventory and the inspection of the launcher methods from the package folder.

```powershell
rea inspect-desktop-application . --detail complete --json
rea inspect-managed-members ".\Lancer GTA VI.exe" --json
```
