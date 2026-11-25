> Français : [tombox.md](../fr/recherche/tombox.md)

# TomBox and its continuation: what we reuse

[TomBox](https://github.com/drycheetah84/TomBox) (drycheetah84) and its continuation [omonkeyz/xbox-emulator](https://github.com/omonkeyz/xbox-emulator) (GPL-3.0-or-later licence since 22 November 2026, MIT or Apache-2.0 before) reimplement the Xbox Series X|S platform layer in Rust. Each console DLL (`d3d12_xs`, `xg_xs`, `xmem`, `xma`…) becomes a replacement DLL there, and `tombox-xshader` translates RDNA 2 shaders to HLSL. AnyXboxX reuses their code and their published observations, and credits them; no game or console file is reused.

State recorded on 21 November 2026 (commit `fef9a6e` of omonkeyz/xbox-emulator), from their files `README.md`, `docs/MILESTONES.md`, `docs/BLOCKERS.md` and `docs/DEVELOPMENT_LOG.md`.

## What they established

| Area | Their state | For AnyXboxX |
| --- | --- | --- |
| Imported system functions (Fortnite) | 671 of 671 covered | model for our #12 and #13 |
| Console Direct3D 12.x methods | 185 of 200, vtable slots measured | unblocks our #18 (device) and #19 (PresentX) |
| RDNA 2 instructions verified | 573 of 721 | to cross-check with our bench of 34,948 LLVM encodings |
| Shader stages | 6 of 8 (mesh and amplification incomplete) | beyond our compute recompiler (#21) |
| Shaders of one title translated | 105,506 of 105,506 (Fortnite) | to validate with our reference interpreter |
| Console libraries | 2,076 of 2,141 exports | our #13, #14, #26 |
| Title launched | Fortnite: startup, sign-in, mode selection, Battle Royale lobby | we have no console title |

## Observations on the GTA VI 1.0.2.1 package

They examined the same package that AnyXboxX studies and reach the same conclusion: it cannot be launched, the executable is missing.

- 26,503 shaders, in compressed Xbox shader containers. In a sample of 4,000, all disassemble; about one third of the vertex shaders translate.
- 8,765 mesh programs: they derived the console ABI of mesh shaders from them (`v5` = group vertex id, `s3[27:24]` = wave number, `s_sendmsg 9` to reserve the output, position exports 12, cull exports 13 to 15 and primitive exports 20).
- Blocker: none of these mesh programs has an `RTS0` block (root signature). Without the title's root signature, their resources in user SGPRs (s16 and following) cannot be bound; it is in the missing executable.

## Differences in method

- They work on already decrypted copies of commercial games. AnyXboxX only uses public sources (AMD and Microsoft documentation, LLVM tests) and free test programs.
- Their checks go through title launches. AnyXboxX checks each shader translation against a reference interpreter, byte for byte, and its decoder against LLVM's encoding tests: both tools can validate their translator.

## What AnyXboxX takes

1. This document and [THIRD_PARTY.md](../../THIRD_PARTY.md): credits and licences.
2. Their repository as a Git submodule (`third_party/tombox`), built with `cargo`.
3. The launcher `anyxboxx.exe` loads their Rust DLLs for the console libraries that AnyXboxX does not cover.
4. Their translator `tombox-xshader`, verified by our reference interpreter on the GPU.

## Checking their decoder

Their RDNA 2 disassembler (`tombox-xshader`), called from AnyXboxX's Rust bridge (`rust/anyxboxx-rdna2`), was checked against the 29,522 unique encodings of LLVM's tests by `tools/compare_tombox.py`: no wrong length, 24,831 identical encodings (84%), 4,586 opcodes missing from their table and 105 different names. Details and examples: [tombox-decoder.md](tombox-decoder.md). AnyXboxX's table, generated from LLVM, covers these missing opcodes; it can be offered to them.

## Their XMA2: their verified decoder

Their `xma` crate (LGPL-2.1-or-later) is a Rust port of FFmpeg's WMA Pro / XMA decoder (`libavcodec/wmaprodec.c`). Their test `wmapro_fate_beethoven_matches_reference`, run locally with the public sample `wmapro/Beethovens_9th-1_small.wma` from FFmpeg's FATE suite and a reference PCM produced by `ffmpeg`, gives a maximum difference of 1 at 16 bits and a signal-to-noise ratio of 101.5 dB. AnyXboxX uses it in `rust/anyxboxx-audio` (`anyxboxx_xma <input.xma> <output.wav>`) for single-stream RIFF XMA2 files; no public XMA2 file has yet been used to verify the whole chain.

## Device slot table of d3d12_xs (first boot, W5)

Slots the title calls on the console device before its first `Close` failure, compared with TomBox's device
table (`crates/compat/d3d12_xs/src/console_slots.rs`, 90 slots) and with what AnyXboxX serves. "Assigned" is
what a grep of `[n] = ...` in TomBox's `d3d12_xs/src` finds; the context of each line was read only where stated.
Frozen by `tests/test_d3d12_device_slots.py`.

| Slot | Measured role | TomBox device table | AnyXboxX |
| --- | --- | --- | --- |
| 8 | CreateCommandQueue (PC form) | assigned, `device_wrap.rs` | none |
| 9 | CreateCommandAllocator | assigned, `device_wrap.rs` | none |
| 16 | CreateRootSignature (shape) | assigned at `cmdlist_validate.rs:1218`, context not checked | none |
| 26 | not established | not assigned to the device; 26 is `fake_barrier` in the list table (`cl_ext.rs:771`) | none |
| 36 | CreateFence (PC form) | not assigned | none |
| 41 | CreateCommandSignature | assigned, `device_wrap.rs` | none |
| 48 | CreateCommandQueueX (console) | assigned, `device_wrap.rs` | none |
| 49 | not established | not assigned | none |
| 57 | DeserializeGraphicsPipelineState | assigned, `device_wrap.rs` | none |
| 61 | CreateSampler | assigned, `device_wrap.rs` | none |
| 73 | CreateCommandListX (console) | assigned, `device_wrap.rs` | none |
| 74 | RegisterPagePool | assigned, `device_wrap.rs` | none |
| 79 | ScheduleFrameEvent | assigned, `device_wrap.rs` | none |
| 86 | not established | not assigned | none |
| 127 | CreateCommittedResource3 (shape) | out of range: TomBox's device table has 90 slots | none |

AnyXboxX serves no device slot: `runtime/d3d12_xs/d3d12_xs.def` exports `D3D12ConfigureVirtualMemoryX` and the
root-signature forwarders, not `D3D12XboxCreateDevice`. The gap is the whole device proxy, not a few slots, so no
simple correction exists. The list slots 9, 131 and 132 belong to the command-list table, which TomBox sizes at
110 (its `vt` array has 128 entries); 131 and 132 are outside it.
