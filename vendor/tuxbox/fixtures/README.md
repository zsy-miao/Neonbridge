# Route Game

Route Game is an original x86-64 Windows console fixture for the Project TuxBox launcher. It walks a tile map, counts blocked moves, collects two independently identified coins and reports whether the last tile is the exit. It uses the host clock through Win32 performance counters. Timing does not affect the simulation result.

## Build

From the project root, run `python build.py route` to produce `build/route-game.exe`. Linux uses LLVM and the Windows SDK/CRT staged via xwin. Set `XWIN_SDK_ROOT` to the directory containing `sdk/` and `crt/` for a custom installation. The standalone route target also supports x86-64 MinGW-w64 GCC. On Windows use an x64 Visual Studio Native Tools prompt with Python, MSVC and the Windows SDK. The existing `fixtures/build.sh` and `fixtures/build.cmd` call the same builder.

## Run

```text
route-game.exe ROUTE [NEW_RESULT_FILE]
```

Supply one to 256 moves using uppercase `L`, `R`, `U` and `D`. Each move advances one tick, including collisions. Coin-mask bit 0 corresponds to the upper route and bit 1 to the lower route. Exit state reflects the final tile.

| Route | Expected output |
| --- | --- |
| `RRRRDD` | `route-game x=5 y=3 coin_mask=1 ticks=6 bumps=0 exit=1` |
| `LDDRRRR` | `route-game x=5 y=3 coin_mask=2 ticks=7 bumps=1 exit=1` |
| `U` | `route-game x=1 y=1 coin_mask=0 ticks=1 bumps=1 exit=0` |

By default the guest writes one ASCII line to its standard output. Supply a new output path to exercise Win32 file creation, writing and closing. This option also works with Proton installations that do not relay Windows console output to a Unix pipe. An existing output file is preserved. The exit codes are 0 for successful simulation, 2 for invalid arguments and 3 for a clock or output failure.

This fixture uses the standard desktop Win32 ABI. A successful run demonstrates native x86-64 guest execution and basic platform calls. Retail Xbox ABI, graphics, input and audio each require separate evidence.

## CPU replay benchmark

The optional `--bench` mode runs 65,536 routes of 128 moves through the same simulation used above. A fixed seed produces 256 input routes, and each route is replayed twice from each of its 128 starting offsets. Two Win32 performance-counter reads bracket the replay loop and checksum calculation. Prefix initialization, process startup, route generation and receipt writing occur outside that interval.

```text
build\route-game.exe --bench
```

On Linux, set `PROTON` to the Valve Proton executable. The result file must be new.

```sh
export STEAM_COMPAT_CLIENT_INSTALL_PATH="${STEAM_COMPAT_CLIENT_INSTALL_PATH:-$HOME/.steam/root}"
export STEAM_COMPAT_DATA_PATH="${STEAM_COMPAT_DATA_PATH:-$PWD/build/prefix-valve}"
mkdir -p "$STEAM_COMPAT_DATA_PATH"
receipt_dir=$(mktemp -d)
"$PROTON" run "$PWD/build/route-game.exe" --bench "Z:$receipt_dir/replay.txt"
cat "$receipt_dir/replay.txt"
```

Run `python3 fixtures/benchmark_reference.py` to compute the expected result with an independent graph model. The guest validates the number of routes, moves and final checksum before publishing a measurement. Exit code 4 means its result differed from the reference.

```text
routes=65536 moves=8388608 checksum=24316bf89788468d
```

Milliseconds equal `1000 × elapsed_ticks / frequency`. On a Ryzen 7 9800X3D, five Valve Proton Experimental runs took 53.82–56.12 ms, with a 54.95 ms median. For a Windows comparison, run the same guest binary on the same hardware with matching power settings and report repeated measurements on both systems. Retail rendering and frame-time measurements require title workloads.
