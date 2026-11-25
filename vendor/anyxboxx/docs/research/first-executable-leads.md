> Français : [pistes-premier-executable.md](../fr/recherche/pistes-premier-executable.md)

# Leads for the first Scarlett executable

State as of 22 November 2026. This note goes with the [compatibility plan](package-compatibility-plan.md). It contains no binary and no excerpt of game resources. The provenance of the studied package is not authenticated.

## Verified starting point

The two package manifests point to `game_scarlett_master.exe`, which the layout places in chunk 1001. The file is absent from its destination, and the local recursive search by exact name did not find it. This does not rule out a renamed file or one inside a container. From the AnyXboxX root, `python PrototypeScarlett/scarlett.py --rapport ..` currently fails and reports the missing executable. Its imports, its execution paths and the shaders it actually uses remain unknown.

The [progress notes](../progress.md) report the isolated initialisation of 16 package DLLs, with 876 imports resolved out of 1,337 in the measured configuration. This measurement covers their import tables and their initialisation, not the missing executable and not every function of the DLLs. PC demonstrations do not prove that a Scarlett game works.

## Diagnosis when an executable becomes available

1. First check its provenance, its version and its hash; compare its configuration with the package, without treating the name as proof. Archive the command, the hash and the project version locally so the diagnosis can be reproduced.
2. Make a **static** inventory of the PE: machine, sections, entry point, TLS and imports. `tools/jour_j.py` does this inventory but then also runs diagnostics in Unicorn: it is not an exclusively static command. The full report stays outside the repository; the repository rules forbid adding game files to it.
3. Then use the path of the [day-one guide](../day-one.md) on an appropriate copy. Unicorn mode makes guest calls easier to diagnose, but must not be taken as a guarantee that untrusted code is contained. Native mode runs the program directly with the user's rights; keep it for a trusted program.
4. Record the **first observable failure**: mode and versions, step, module, call or instruction, useful arguments, error, and if possible a minimal reproduction. Tell apart a missing export, a function that returns `E_NOTIMPL`, an incorrect ABI and an incorrect result. Add a matching synthetic or public test before any fix.

### Limits of the current provider table

Reading `tools/jour_j.py`, functions `inventory`, `provider` and `coverage`, shows that this table is a **static hint**, not proof of support:

- An import by ordinal (`#...`) is assigned to a DLL that was found, without checking that this ordinal really exists in its exports.
- `api-ms-` and `ext-ms-` names are assigned to Windows without checking the individual export; a DLL considered a system DLL but absent from `System32` can also be assigned to Windows by assumption.
- The existence of a named export proves neither that its implementation is complete, nor that its conventions, structures and return values are correct. Dynamic and delayed imports are not covered by the list of ordinary imports of `inventory`.

Before using the total as a progress criterion, test these cases on synthetic PEs, tell verified results apart from assumptions, and compare the report with a controlled execution trace. The current tests in `tests/test_jour_j.py` cover a simple synthetic package; they do not demonstrate the cases above.

## Resources: assumptions to check against the engine

- The local research on RDS5 and ETPF documents block boundaries and zlib decoding on a sample. The handling of the other blocks and the full RPF format remain unknown; do not assume that the research readers reproduce the engine's storage operations.
- On a specific subset, the local experimental viewer reads four bytes per pixel and can export linear or Morton variants. No reference image has validated the layout or the channel order. For a future implementation, build a synthetic data set and compare it with a known reference; do not add extracted images to the repository.
- The [TomBox](tombox.md) notes report an external study of shaders and limits around mesh shaders and root signatures. These are reported observations, not proof that the future executable will call these paths; establish the real dependencies from the executable and from authorised tests.

## Local check of this note

On 22 November 2026, `python -m pytest tests/test_jour_j.py tests/test_launch.py -q` gave **9 passed tests** with `PYTEST_DISABLE_PLUGIN_AUTOLOAD=1`. Without this variable, the third-party `xonsh` plugin installed globally failed at pytest startup for lack of a console; no test had run at that point. These tests cover the synthetic cases of the repository, not the missing game.
