> Français : [shaders-embarques.md](../fr/recherche/shaders-embarques.md)

# Shaders embedded in the package: measurement without llvm-mc

Measurement of 22 November 2026, done locally; only counts appear here (no shader, disassembly or
SPIR-V, rule 1 of AGENTS.md).

`XboxBvhBuilder.dll` (a Microsoft library shipped in the package) contains 52 DXBC containers, each with
a native RDNA 2 program (`XBCH` → `FSSM`), according to [Leonida's note](https://github.com/ItsNotRexed/Leonida/blob/main/docs/embedded_shaders.md).
These are the only real console shaders available before the game executable.

| Step | Result |
|---|---|
| Disassembly by AnyXboxX (without llvm-mc) | 54,197 instructions; 1,431 words without text, data between the programs of one blob (up to 5 programs) |
| RDNA 2 → SPIR-V translation (Leonida's translator, fed by our disassembly) | **51 programs out of 52** |
| Remaining failure | `buffer_store_short`, a rule missing from the translator |

Two fixes produced this result (before: 0, then 41 out of 52):

1. **Compute shaders**: the translator reads the dispatch state (`rsrc2`, group size) from the FSSM header.
   `rdna2_spirv.translate` now accepts the blob (`fssm=`) or this state (`dispatch=`); with neither,
   a compute shader is refused with a clear message.
2. **Data between programs**: with `code_start`, `disassembly` skips a word without text like llvm-mc and
   reports it; the translator realigns on the FSSM blob.

To reproduce (extraction with Leonida's GPL-3.0 tools, in a working folder outside the repository):

```powershell
python -I pe_shader_containers.py <paquet>\XboxBvhBuilder.dll --json <travail>\containers.json --extract <travail>\dxbc
python -I fssm_unpack.py <travail>\dxbc
python tools\fssm_mesure.py <travail>\dxbc
```

The same tool will work on the game's shaders once the executable and its PSOs are available.
