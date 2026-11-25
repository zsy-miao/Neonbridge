> Français : [plan-compatibilite-paquet.md](../fr/recherche/plan-compatibilite-paquet.md)

# Preparing compatibility for the studied Scarlett package

This note is for the agents who continue AnyXboxX. It describes a research and testing strategy, not a recipe that can currently launch the game. It relies on the local copy and on the [package analysis notes](package/README.md).

## Verified state

- The layout and `MicrosoftGame.Config` point to `game_scarlett_master.exe`, which is absent from the copy. From the AnyXboxX root, `python PrototypeScarlett/scarlett.py --rapport ..` explicitly reports this absence. The imports, the entry point, the interfaces and the shaders actually used by the executable are unknown. The resources do not allow its code to be reconstructed.
- The DLL initialisation documented in the [progress notes](../progress.md) and the [SimpleTriangle and SDL](../test-game.md) trials are partial tests. The two examples that were run are PC versions: they do not demonstrate the compatibility of a Scarlett executable. A resolved import or a successful `DllMain` does not prove that all the functions of the DLL will work.
- Native mode runs code with the rights of the user process. Use trusted test programs; Unicorn mode is for isolated diagnostics.
- The local data and their provenance have not been authenticated. The results below concern the files examined, not necessarily all versions of the game.

## Recommended order of work

1. **Keep a reproducible reference.** Run the repository tests with `python -m pytest` from its root. Keep the documented PC examples and, when available, add an authorised, known Scarlett test program. For each result, note the host version, the mode, the command, the binary and its hash. Do not extrapolate PC results to the missing game.
2. **Loader and system calls.** Check sections, relocations, import resolution, TLS, initialisation and unloading with controlled test PEs. Extend the HLE functions step by step from traces of test programs; for each API, respect its calling convention, its structures, its return type and its errors. Avoid counting a merely resolved import as a working implementation. The current limits on `E_NOTIMPL`, exceptions, threads and dynamic loading are described in the [progress notes](../progress.md) and the README.
3. **Graphics and compute.** Define the D3D12.X interfaces that are really required from traces or from an appropriate test binary; build minimal tests for resources, synchronisation and commands before generalising. The compatibility of console shaders, of their format and of their execution remains to be established. The FidelityFX strings in this copy do not prove which features the game enables. See [REA_RESEARCH.md](package/REA_RESEARCH.md).
4. **Storage and resources.** Develop the readers outside the emulator with bounded test files and length checks. The RDS5 and ETPF tables are described in [GAME_ANALYSIS.md](package/GAME_ANALYSIS.md) and [ETPF_ANALYSIS.md](package/ETPF_ANALYSIS.md). The match between groups and blocks was verified on this copy; the pixel formats, the codec of the blocks without a zlib flag and the internal RPF structure remain open. Do not assume that these reading tools reproduce the game's file system.
5. **Services.** Add audio, input, saves, files and GDK interfaces through independent test scenarios, comparing the errors and the observable effects with an appropriate reference. Network calls and online services are not characterised by the files studied.
6. **If an appropriate executable becomes available**, start with its static inventory and its hash, then with controlled reports and trials. Record the first concrete divergence, add a reduced test and a fix, then repeat. The availability of an executable does not guarantee compatibility.

## Useful local commands

From the package root:

```powershell
python .\analyse_paquet.py inventaire .
python .\inspect_etpf.py .\data\ttdb\graphics\ttdb_graphics.ttpf --debut 485 --limite 1
python .\stats_etpf.py .
```

From the AnyXboxX root:

```powershell
python -m pytest
python PrototypeScarlett\scarlett.py --rapport ..
```

These inspection commands do not launch the game executable. The repository tests run their own test programs and fixtures.

For resources, the size rounding assumptions of the ETPF document can be checked automatically but do not make up an image decoder. Most blocks whose bit 29 is not set are smaller on disk than their logical size; copying their bytes is not enough. Treat their transformation as unknown until a full decoding and verifiable outputs have been obtained.

The spatial continuity trials documented in [ETPF_ANALYSIS.md](package/ETPF_ANALYSIS.md) make Morton paths candidates for some small four-bytes-per-pixel resources; results vary by block and do not prove a universal layout. For an implementation, look for a known reference image and add a test with dimensions, bytes and expected result before adopting a transformation.

The sizes of the last blocks of most groups of three or four follow a progression by four. They are compatible with several scales of a texture, but the content of the first block remains unknown and exceptions exist. Do not deduce a full mipmap table from these sizes alone; keep the block boundaries and test the assumptions on reference resources.

For a specific category of 153 groups with candidate dimensions of 512 × 512, content comparisons suggest that the first block gathers small resolutions in five intervals separated by zeros. The `comparer_niveaux_etpf.py` test and its assumptions are described in the [ETPF analysis](package/ETPF_ANALYSIS.md); it needs NumPy. This observation does not establish a layout that holds for the other categories.

The comparison of 16 layouts for these 153 groups gives the best score to Morton 128 tiles in 142 cases; 11 groups favour linear variants. Use this result as a test lead, not as a universal conversion. The check of the indices on synthetic data only confirms that the tool applies valid permutations.

The ETPF index also contains a candidate table of sorted 64-bit values. Comparisons between files support their association with the rows of the same rank, but three pairs of identical values gave different zlib contents. If a future reader uses them for lookup, keep the file context and do not deduplicate resources on this value alone. The command `python ..\comparer_cles_etpf.py .. --comparer-blocs` reproduces the comparisons from the AnyXboxX root.

To examine a value in a specific file, `inspect_etpf.py --cle` gives its rank and the matching candidate row, after checking the order of the table. See the example and the limits in the ETPF analysis; this tool does not yet implement the loading of game resources.

Microsoft's public documentation describes steps of [DirectStorage decompression and rearrangement on Xbox Series](https://learn.microsoft.com/en-us/gaming/gdk/docs/features/console/storage/directstorage/directstorage-overview), including BCPack. This justifies evaluating these possibilities for textures, without assigning a codec to the blocks of this package before independent verification.
