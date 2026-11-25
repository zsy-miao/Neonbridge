# Contributing to AnyXboxX

Thanks for your interest. These rules apply to everyone except the maintainer.

Discussion, help and test reports: [Discord](https://discord.gg/TcX42vRyPU)

**Write issues, pull requests and commit messages in English.**

**Your copy of the game is welcome locally.** You may launch, measure, debug and analyse your own copy of the game's executable and build to steer your pull requests. What stays forbidden: adding game or console files to the repository (see "What is refused").

## Main rule: everything goes through a pull request

The `main` branch is protected. **Nobody can push to it directly.** To propose a change:

1. Fork the repository.
2. Create a branch from `main`: `git checkout -b feat/my-feature`.
3. Add your commits, with messages in the form `type: description` (`feat`, `fix`, `docs`, `test`, `refactor`, `chore`).
4. Run the tests: `python -m pytest`, then `python -m ruff check --select W605,F,E9 PrototypeScarlett tests tools`. Everything must pass.
5. Open a pull request against `main` and fill in the template: the commands you ran and their exact result.

A pull request is merged only after the maintainer's review and approval, and once the CI checks pass:

- `lint` (Linux, ~1 min): ruff and consistency of the READMEs in the 9 languages;
- `pytest` (Windows): build of the C++ runtime and the Rust code, then the whole suite. It also checks the test count stated in the READMEs: the hundred below, followed by "+" (806 tests → "800+"). Only change it if your PR crosses a hundred.
- `politique VIctory` (Linux): VIctory's asset policy (`cargo xtask policy`) on `third_party/victory`.

### Good practice

- **One PR = one behaviour.** Two independent fixes go in two PRs, which are easier to review.
- **Conflict with `main`**: rebase (`git fetch && git rebase origin/main`), keep both versions when they complement each other, rerun the tests, then `git push --force-with-lease`.
- **A failing check unrelated to your PR** (a GPU test on WARP, for example): say so in the PR, with the test name and the error line; the maintainer will rerun the job.
- **AI assistance**: say so in the PR description.

## Issues

Before writing code, you can open an issue:

- **Bug**: what you ran, what you expected, what happened, the log or the `--rapport` report.
- **Function to emulate**: the exact function name (`dll!Function`), its GDK documentation and the program that calls it.
- **Idea**: the problem the idea solves.

For a large change, open an issue first to discuss it.

## Sources and use of your copy of the game

The implementation relies on public sources: AMD's RDNA 2 ISA documentation, Microsoft Learn (GDK, Win32, D3D12), LLVM's tests, and open-source code under a compatible licence, credited in [THIRD_PARTY.md](THIRD_PARTY.md).

You may use your own copy of the game's executable and build: launch it (native, VIctory through `tools/lancer_victory.ps1`, AnyXboxX), measure, debug, analyse it statically and dynamically, and use what you learn to steer your PRs. Measurements and findings drawn from running it (summarised logs, counts, technical identifiers such as shader hashes or pass names, test results) may be cited in PRs and in the docs, with how they were obtained (measured, read statically, assumed). The limit: no file, code or asset of the game or the console in the repository. Three things drawn from the game are allowed, as in VIctory's policy (VIctory's `docs/asset-policy.md`): byte-pattern signatures (short patterns written by us, with masks) that locate functions or data in the executable, the per-build offsets of the functions we hook (under names we choose), and the build identity (hash of the executable, CodeView GUID/age).

## Taking code from another repository

The repository depends on no other repository to build: no new submodule, no pinned fork. `third_party/victory` is our code, with no upstream. To take a change from elsewhere (a commit, a pull request, a fork's branch):

1. Commit it yourself: `git fetch <repository> <branch>` then `git cherry-pick -Xsubtree=third_party/<project> <commit>`, or a port by hand.
2. End the message with the source repository and the author:

   ```
   Source: <repository> <PR or commit>
   Co-authored-by: Name <email>
   ```

3. Add or update that repository's row in the sources table of [THIRD_PARTY.md](THIRD_PARTY.md). Its licence must be compatible (GPL-3.0 or more permissive).

## What is refused

- Any game or console file added to the repository: executables, DLLs, `.rpf` archives, extracted shaders, textures, sounds, saves, dumps, keys, even partial ones and even in tests.
- Game code (sections, routines, decompiled or disassembled code beyond the allowed signatures) or assets copied from the game, even taken from a legitimate copy.
- Proprietary data tables, unless the maintainer decides an exception: only constant tables without which a format cannot be decoded and that cannot be recomputed (like VIctory's BCPack tables) may be added, each one named in the exceptions register of `THIRD_PARTY.md`, and the repository licence does not cover them.
- Links to illegal copies of games.
- Code without a test for a new emulated function.

## Adding an emulated function

Add a handler decorated with `@hle(...)` in `PrototypeScarlett/emu/hle/` and a test in `tests/test_hle.py`. Quote the function's public signature (Microsoft GDK or Win32 documentation) in a comment.
