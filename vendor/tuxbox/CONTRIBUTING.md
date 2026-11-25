# Contributing

TuxBox is licensed under the Apache License 2.0. A contribution in a pull request is licensed under the same terms. Third-party materials keep their own terms. [License](LICENSE) · [Third-party notices](docs/REFERENCES.md#shader-compiler-attribution)

Open pull requests against `main`.

## Prepare a checkout

Use Python 3.10 or newer and Rust 1.95 or newer. CI uses Python 3.12 and Rust 1.95.0. From the repository root, install the Python dependency and the Windows target used by the D3D12 typecheck:

```sh
python -m pip install -r requirements.txt
rustup target add x86_64-pc-windows-msvc
```

Building Lanes or a provider needs the desktop compiler and Proton setup in the [Lanes build requirements](examples/lanes/README.md#build-requirements).

## Every change is verified on Lanes

A change is merged only after [Lanes](examples/lanes/README.md), the project's own test game, passes on a GPU with that change. This applies to every pull request. Run from the repository root:

```sh
python tuxbox.py check
```

It runs the code checks below, builds every provider and guest, and then runs Lanes and the other guest fixtures on your GPU. The Lanes part covers:

| Group | What runs |
| --- | --- |
| Replay | The game itself: course generation, input, presentation and board readback |
| [Self-test](examples/lanes/README.md#self-test) | Xbox graphics behaviour retail titles depend on, each scenario failing if the GPU does not finish in time |
| Shader cache | A console shader loaded from a cache prepared before launch, translated while running, then reused without compiling again |
| [Benchmark](examples/lanes/README.md#benchmark) | Submission, fence handover, copy and pipeline timings. Add `--baseline` with an earlier result to compare. |

On Linux it also runs the [D3D12 regression controls](tools/check_d3d12_regressions.py), which cover buffer imports, page remapping, descriptor tables and other provider behaviour in isolated copies of the prefix. They need `--proton`, `--prefix`, `--windows-dxc` pointing at Microsoft's x64 `dxcompiler.dll` and `dxil.dll`, plus Bubblewrap and `xvfb-run`:

```sh
python tuxbox.py check --proton PROTON --prefix PREFIX --windows-dxc DXC/bin/x64
```

With the same options it runs the [shader translator checks](tools/check_translator_gpu.py): synthetic console programs translated, run on the GPU and compared word for word with a reference model. They also need `llvm-mc` from LLVM. When you add or fix an instruction, add it to one of these checks.

Put the printed summary, your GPU, driver and backend in the pull request. Pass `--backend`, `--proton`, `--prefix` or `--adapter` as for `tuxbox.py run`. Results are saved in `build/check/results.json`.

If your change adds or fixes behaviour, add or extend a Lanes scenario that fails without it. If it cannot be exercised from Lanes, say why in the pull request.

CI has no GPU, so it cannot run Lanes. A reviewer should not merge a pull request without a passing GPU run, from the author or from someone who has the hardware. `python tuxbox.py check --no-gpu` runs only the first group while you work.

## Code checks

GitHub runs these on every pull request and on every push to `main`. [Workflow](.github/workflows/ci.yml)

```sh
python -m unittest discover -s tests -q
cargo test --locked --target x86_64-unknown-linux-gnu --manifest-path tools/shader-compiler/Cargo.toml
cargo test --locked --target x86_64-unknown-linux-gnu -p tuxbox-dxbc --features native-bindings
cargo test --locked --target x86_64-unknown-linux-gnu -p tuxbox-xg-layout
cargo check --locked --target x86_64-pc-windows-msvc -p tuxbox-d3d12
```

The D3D12 command typechecks the Windows provider. The Python tests check that `docs/progress.svg` and `docs/RESULTS.md` match the recorded evidence.

## Evidence

When a change adds or edits a receipt under `evidence/`, regenerate both views and include them in the pull request:

```sh
python tuxbox.py status > docs/RESULTS.md
python tuxbox.py status --svg docs/progress.svg
```

The results table and the progress map are generated. Editing either file by hand makes the unit tests fail.

## Files that stay on your machine

Retail games, Proton prefixes, build output, crash dumps, and packaged binaries stay outside the repository. `.gitignore` excludes `build/`, `games/`, `prefixes/`, `target/`, and the usual package and binary suffixes. The unit tests fail when a tracked file matches that ignore list.

Measured homebrew receipts in `evidence/` belong in the repository. Keep title dumps, captures, and native libraries on your machine.
