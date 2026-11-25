# CLAUDE.md

@AGENTS.md

## For Claude Code

- Follow AGENTS.md: it holds the layout, commands and rules of this repository.
- Talk to the maintainer in French; code identifiers, README and image text stay in English, research notes in English in `docs/`, with the French version in `docs/fr/`; update both together.
- Before a pull request: run the full pytest suite and the ruff check from AGENTS.md, rebuild `runtime/` with CMake when C++ changed, and `cargo build --release --manifest-path rust/Cargo.toml` when Rust changed.
- Regenerate `PrototypeScarlett/emu/rdna2_ops.py` and `rdna2_shapes.py` with the tools instead of editing them; the LLVM test files are downloaded locally from `llvm/test/MC/AMDGPU` and never committed.
- When a README table row changes, update all 9 languages in the same commit, and regenerate `docs/images/carte-avancee.svg` and `traduction.svg` with `scarlett.py --carte ..` and `--traduction ..` when the counts move.
- You may launch and analyse the build in the parent folder locally (run, measure, debug, static and dynamic analysis); never copy or commit its files into the repository. Only byte-pattern signatures, per-build offsets of hooked functions and the build identity may come from the executable, plus the maintainer-approved format tables listed in the exceptions register of THIRD_PARTY.md (model: VIctory's `docs/asset-policy.md`); never game code.
