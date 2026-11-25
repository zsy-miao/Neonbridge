# Contributing

Thanks for helping. This page explains what the project accepts, how the code is written and how a change gets in.

## Ground rules

These protect the project. A change that breaks one of them is closed, however good it is otherwise.

- **No game content.** No game files, keys, executables or data extracted from the game enter the repository: not in
  code, tests, fixtures, screenshots or docs. Tools take the path of your own console dump as input.
- **Your own work.** Contribute only what you wrote or have the right to contribute. Never copy code from another
  project, leaked material or decompiler output; facts about a format are fine, code is not.
- **No protection bypass.** The project reads data from a copy you own. It does not ship or explain ways to get keys,
  decrypt protected content or work around platform security.

## Getting started

1. Install the .NET 10 SDK. Shader work also needs the DirectX shader compiler (`dxc`, Windows SDK). Adding or
   changing a shader test program, and `fivestar shader decode-check`, also need LLVM 18+ with the AMDGPU target
   (`llvm-mc`, `llvm-objdump`).
2. `dotnet build FiveStar.slnx` and `dotnet test`. Tests that need game files are skipped until you set
   `FIVESTAR_DUMP` to the folder of your own dump. Tests that need a tool skip with the reason when it is missing:
   `FIVESTAR_LLVM` names the LLVM `bin` folder, `FIVESTAR_DXC` the `dxc.exe` to use.
3. The shader test programs are kept assembled in `tests/FiveStar.Shaders.Tests/Assembled`. After adding or changing
   one, run the shader tests once with LLVM: the new `.bin` file appears there; commit it with the test.
4. The `docs/` folder explains each format and tool; `README.md` lists where to start.

## Code

The build enforces most of this (`build/FiveStar.globalconfig`, StyleCop layout rules, warnings as errors), so a
change that builds is already close. `tests/FiveStar.Architecture.Tests` checks the rest, one test case per rule, and
lists each violation as `path:line: [Rule] message`.

- Every code file (C#, HLSL, scripts) starts with these two lines in its own comment style:
  `Copyright (c) 2026 Five Star Recomp contributors.` and `SPDX-License-Identifier: GPL-3.0-or-later`.
- One type per file, named like the file. The namespace is the project's root namespace followed by the folder path.
- File-scoped namespaces, `System` usings first, no unused usings.
- PascalCase for types and methods, camelCase for parameters and locals, `_camelCase` for private fields, `readonly`
  when they can be.
- Classes are `sealed`, `abstract` or `static`. Data is a `record` or `readonly record struct`; absence is a nullable
  member, not a sentinel.
- Braces on every block. Rejected conditions come first and return early.
- A conditional expression only as one simple assignment, never nested; anything more is an explicit `if`/`else`.
- No property patterns (`x is { } y`). Put the value in a local and test it plainly.
- A method spans at most 60 lines. Longer work is split into named private steps.
- Methods on per-frame paths carry `[HotPath]`: no allocation, no locks. Each project declares its own `internal sealed
  class HotPathAttribute : Attribute`; the check matches it by name.
- An `[UnmanagedCallersOnly]` method in `src` that calls, creates or indexes anything is one `try` statement that
  catches `Exception` (the loader's call `UnmanagedGuard.Caught`) and returns its failure value: an exception that leaves
  it ends the process without a managed stack.
- The loader and the libraries it uses are trim- and NativeAOT-safe (the analyzers run on every build): no member
  reflection, no runtime code generation. A shim type is `partial`; its shims are registered at compile time
  ([docs/aot.md](docs/aot.md)).
- A dispatch over commands or opcodes is a `switch` with one private handler per case.
- No magic numbers. A value gets a name once (a `const`, a `static readonly` field or an enum member). A value that
  comes from a hardware or file format cites where it comes from in a comment (an ISA section, a header field).
- Collaborators arrive through the constructor; a class has at most one public constructor. No static holds state.
- Vertical spacing: a blank line above every block statement and every `return`/`throw` unless it is the first
  statement of its block; at most 6 statements in a row without a blank line.

## Tests

- `dotnet test` passes before a change is submitted, and it stays quick.
- Tests are concrete and few: each one checks behaviour that matters to the project (a format decodes, a shader
  computes the right value, a module loads). No tests of individual strings or small calculations unless a mistake
  there would break the project. Coverage is not a goal; code that is easy to test is.
- Unit tests use small hand-made inputs. Tests on real game files read `FIVESTAR_DUMP` and skip without it.
- Shader translator tests run on D3D12 WARP and compare against a reference model of the RDNA instruction set,
  never against the translator's own output.
- CI (`.github/workflows/ci.yml`) builds and tests every push and pull request. Windows runs every test that needs no
  game files, the WARP tests included (dxc from the Windows SDK; the test programs come assembled, so no LLVM).
  Linux builds the solution and runs every test project except the loader's, which needs Windows.

## Documentation

Short, factual and to the point.

- One page per subject in `docs/`: what it is, the exact layout or behaviour (tables for binary layouts and other
  data, a Mermaid diagram for a structure or flow), how to use the tool. Nothing else.
- Facts only. Mark what is unconfirmed once, in an "Unknown" section.
- No status notes, history or task lists. There is no changelog file: the git history is the changelog, so commit
  subjects must say what changed.
- Status, roadmap and research notes go in `docs/wiki/`, which is published to the GitHub wiki on every push to `main`.
  Edit them there, not on the Wiki tab (the next publish overwrites it). Link pages as `Page.md`.
- A doc changes in the same pull request as the code it describes.

## Submitting a change

1. Open an issue first for anything larger than a fix, so the approach can be agreed before you spend time on it.
2. Fork, branch from `main`, and keep the change to one subject.
3. Commits are small and of one kind (a fix, a feature, a refactor, a test, docs), with a subject line
   `Area: what changed`, for example `Shaders: translate image_bvh_intersect_ray`. Areas: `Archives`, `Textures`,
   `Pso`, `Platform`, `Shaders`, `Loader`, `Cli`, `Docs`, `Build`, `Tests`.
4. Sign your commits (SSH or GPG) so GitHub shows them as verified.
5. Open a pull request that says what changed, why, and how you checked it (which tests, which real files). A change
   to a format reader or the shader translator states which files or programs it was checked against.
6. A maintainer reviews it; expect questions. Pull requests are merged when the build, the tests and the review pass.

By submitting a change you agree that it is your own work and is licensed under the GNU General Public License,
version 3 or later (see `LICENSE`).
