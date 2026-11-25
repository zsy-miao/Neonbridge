# Source layout

## Where things live

| Folder | Contents |
| --- | --- |
| `runtime/` | The libraries a game loads, one folder each. `d3d12` is the graphics provider. |
| `runtime/dxbc/` | Reading console shader containers and instructions, shared by the runtime and the translator |
| `tools/shader-compiler/` | The shader translator, `xshader` |
| `examples/lanes/` | Lanes, the test game |
| `project_tuxbox/` | The Python launcher, build and check commands |
| `tests/` | Python tests |
| `evidence/` | Recorded results |

## The graphics provider

`runtime/d3d12/src/commands.rs` and `device.rs` hold the shared types and the tables of Xbox method slots. Their folders hold the methods by subject: for example `commands/barriers.rs`, `commands/submit.rs`, `device/queues.rs` and `device/views/`.

## The shader translator

A shader passes through `tools/shader-compiler/src/hlsl/` in this order:

| Folder | Step |
| --- | --- |
| `driver/` | Entry points. `pipeline.rs` chooses how to translate a program. |
| `rewrite/` | Rewrites of the instruction list, such as recognised wave reductions |
| `emitter/` | Resource resolution, control flow and one file per instruction family |
| `text/` | Operand names and the HLSL helper functions pasted into output |

`hlsl.rs` holds the shared types, including the `Emitter` itself. The other files in `hlsl/` handle one instruction pattern or one kind of program each.

## Rules

`python tuxbox.py check` enforces both.

- **No Rust file is longer than 1,200 lines.** Split it by subject into a folder of the same name.
- **Tests live in each crate's `src/tests/` folder**, mirroring the source. The source file declares them, so they can still reach private items:

  ```rust
  #[cfg(test)]
  #[path = "../tests/commands/compute.rs"]
  mod tests;
  ```

  Cargo's top-level `tests/` folder only sees public items, so it isn't used.
