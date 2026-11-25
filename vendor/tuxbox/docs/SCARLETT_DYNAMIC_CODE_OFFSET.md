# Bounded runtime embedded-table offsets

The compiler previously refused nonconstant SOFFSET on an embedded-code
scalar load. This change permits the compute-only case where a local
GETPC pair reaches the load unchanged, and a bounded ordinary SGPR mask
and shift prove that every dword lies inside the table, with dword
alignment. Unknown clobbers, branch entries, empty tables, overflow,
indirect control flow, packed and mesh paths remain unsupported.
It does not change existing static loads or permit arbitrary root
descriptor offsets.

The unit test uses synthetic gfx1030 instruction encodings and checks
rejection of invalid bounds, high-half clobber, unaligned offset,
multi-dword overrun and a bypass branch. For GPU readback extract the
HLSL between BEGIN_HLSL and END_HLSL from:

```sh
cargo test --target x86_64-unknown-linux-gnu --manifest-path tools/shader-compiler/Cargo.toml --lib bounded_runtime_code_offset -- --nocapture
```

Pass the extracted file to `tools/check_dynamic_code_offset.py --hlsl ...`
with the usual provider, Proton, prefix, Windows compiler-dir, work,
output and adapter arguments. The check adds only a uniform input and
readback stores to the actual emitted HLSL. It compares all 384 words
of each buffer, including input and guards. No title data is required.

The RTX 4080 control passed 64 dispatches, 24,576 words including 4,096
result words, with zero mismatches. This is a bounded synthetic pattern,
not a measured reduction in retail refusal counts. See the receipt.

### Repeat the bounded table-index GPU checks

From the repository root, build the Linux translator:

```sh
cargo build --locked --manifest-path tools/shader-compiler/Cargo.toml \
    --target x86_64-unknown-linux-gnu --target-dir build/table-check
```

Set `DXC` to the Linux compiler (with `dxv` beside it), `PROTON` to a
Proton installation, `PREFIX` to an initialized Wine prefix, `PROVIDER`
to the built `d3d12_xs.dll`, and `COMPILER_DIR` to the directory holding
the Windows DXC DLLs. Set `ADAPTER` to the GPU's vendor and device ID
(e.g. `10de:2702` for the RTX 4080 SUPER). The D3D12 guest uses the
project's Windows cross-build toolchain.

```sh
for pattern in or xor bfe cselect; do
    python3 tools/check_dynamic_code_offset.py \
        --xshader build/table-check/x86_64-unknown-linux-gnu/debug/xshader \
        --pattern "$pattern" --adapter "$ADAPTER" --proton "$PROTON" \
        --prefix "$PREFIX" --provider "$PROVIDER" --compiler-dir "$COMPILER_DIR" \
        --work "build/table-check/$pattern" --output "build/table-check/$pattern.json" || exit 1
done
```

Each synthetic RDNA 2 program loads its input through a UAV, selects a word
from its embedded table and stores one result per lane. The runner translates
each program once and dispatches it with 64 input values. It compares each
result and every surrounding buffer word with the CPU reference.
