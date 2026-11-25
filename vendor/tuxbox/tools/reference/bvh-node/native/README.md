# Native RDNA2 node control

This optional Linux control runs the `image_bvh64_intersect_ray` instruction
on a gfx1036 Radeon using HSA. It checks two equal-distance box tests (BOX32
and BOX16) and uncompressed triangle tests including both windings, misses,
shared edges, zero ray distance, and determinants of magnitude four. The first
box lane misses; the other three hit at the same distance. Expected result
words are checked in the host program. These are synthetic inputs; passing this control does not
establish general opcode equivalence or game compatibility.

On gfx1036 the ordinary misses in this fixture preserve the signed determinant
and return infinity with the same sign. The control checks all six permutations
of three triangles against both Z-axis ray directions at their three edge
midpoints and three vertices (216 cases), 108 oblique-ray edge cases and 108
edge cases with tied dominant ray directions. A seeded set of 48 finite
BOX16/BOX32 inputs, including fractional origins and inverse directions, is
compared with CPU box calculations. The control also
records nine parallel or degenerate triangle cases and five zero, subnormal,
or infinite ray cases. Twenty additional box tests cover both signs of an
infinite inverse direction at slab interiors, boundaries and exterior points.
Another 90 cases exercise descriptor box growth against one- and two-step
floating-point gaps at five positive distances. The scalar helper still only
supports zero growth; other growth behavior needs broader validation.
Earlier scalar shader comparisons used constant inputs, so compiler folding
could bypass GPU arithmetic. General special-value behavior and numerical
equivalence remain unverified. The control checks all three observed barycentric selectors
on each paired triangle type using unequal barycentrics. These cases do not
establish general instruction equivalence.

The same probe checks a TRI0 intersection at distance 1 with four dynamically
loaded maximum distances: 0.5, 1, 2 and 100. All four return the same finite
numerator and denominator on gfx1036. The node instruction has no minimum
distance input; subsequent shader traversal may filter the candidate. This is
one synthetic triangle and does not establish full shader 219 behavior. The
recorded result is `evidence/matrix-awakens/bvh-native-endpoints.json`.
The probe also checks an on-triangle ray origin (distance zero), for both Z
directions at the interior and at three edge points. The interior produces a
zero numerator in both directions; the tested edges have differing ownership.
These node results do not include a traversal's culling or distance filters.

Requirements are an accessible `/dev/kfd`, an HSA runtime and its headers,
a clang with the AMDGPU target, and `ld.lld`. Compile and run from the project
root, using the actual HSA installation prefix in `HSA_ROOT`:

```sh
HSA_ROOT=/opt/rocm
clang -target amdgcn-amd-amdhsa -mcpu=gfx1036 -nogpulib -x cl -cl-std=CL2.0 -O2 -c tools/reference/bvh-node/native/probe.cl -o /tmp/tuxbox-bvh-probe.o
ld.lld -shared /tmp/tuxbox-bvh-probe.o -o /tmp/tuxbox-bvh-probe.hsaco
clang -I"$HSA_ROOT/include" tools/reference/bvh-node/native/probe.c -L"$HSA_ROOT/lib" -lhsa-runtime64 -lrocprofiler-register -o /tmp/tuxbox-bvh-probe
LD_LIBRARY_PATH="$HSA_ROOT/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" /tmp/tuxbox-bvh-probe /tmp/tuxbox-bvh-probe.hsaco
```

To compare those same 48 seeded node inputs and native instruction outputs
directly with the standalone HLSL helper, generate a local synthetic dump and
pass it to the differential tool. The tool requires the project's normal
DXC and Windows guest build dependencies plus test Proton prefixes for each
adapter; no game files are needed.

```sh
LD_LIBRARY_PATH="$HSA_ROOT/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" /tmp/tuxbox-bvh-probe /tmp/tuxbox-bvh-probe.hsaco --dump /tmp/tuxbox-bvh-cases.bin
python tools/check_native_bvh_hlsl.py --dump /tmp/tuxbox-bvh-cases.bin --proton /path/to/proton --prefix-amd /path/to/amd-prefix --prefix-nvidia /path/to/nvidia-prefix --amd-vulkan /path/to/radv.json --output /tmp/tuxbox-bvh-diff.json
```

`--amd-vulkan` is optional when the default Vulkan driver exposes Radeon.
The result checks these cases only and does not establish full traversal or
instruction equivalence. The tool uses runtime-dependent values by default to
prevent constant folding. Older receipts used constant inputs and cannot prove
dynamic GPU arithmetic equivalence.

For six paired-triangle cases with unequal barycentrics, generate the dump
with `--dump-triangles /tmp/tuxbox-bvh-triangles.bin` instead. Pass that file
to the same differential tool with `--dump`.

To test the root node of a user-owned serialized Xbox BLAS, run the probe with
`--retail /path/to/serialized.bin /tmp/tuxbox-bvh-root.bin` and pass the output
to the same differential tool. This checks eight rays against one BOX16 or
BOX32 root; it validates the root's range in the file, not the full graph or
its safety for traversal. The generated dump contains game-owned node bytes;
keep it local.

`--retail-triangles /path/to/serialized.bin /tmp/tuxbox-bvh-leaves.bin`
selects six supported paired leaves with finite vertices and nonzero projected
area; pass the resulting local dump to the differential tool. The tool returns
a nonzero exit status when any raw word differs and writes the mismatches to
the requested JSON report. `--strict` additionally compiles the HLSL with
DXC strict IEEE math. Earlier constant-input comparisons of the original
scalar helper differed from native by up to three ULP for the tested Frosty
sphere leaves, with or without `--strict`; do not treat the helper as bit exact.
For the same six leaves with varied ray directions, use `--retail-oblique`
or `--retail-tied` in place of `--retail-triangles`.

The separate `rtz_triangle.hlsl` experiment uses a ray-relative projected
edge formulation with round-toward-zero after each operation. The original
binary64 implementation mismatches native on dynamically evaluated AMD inputs
even when constant-input controls match. An experimental float32 split-product
variant is selected with `--strict --projected-rtz --float-rtz`. Its sampled
dynamic-input hits, misses, edges and degenerate cases are in
`evidence/matrix-awakens/bvh-native-rtz-edges.json`. Three of eight extreme
scale cases mismatch native: signed zero at 2^-60, saturation versus infinity
at 2^60, and NaN/saturation differences at 2^100. Its split-product arithmetic
is not validated for overflow or underflow; numerical behavior outside these
inputs and performance remain unverified. Neither variant is connected to the
runtime. This experiment is
informed by [AMD's public ray-triangle description](https://patents.google.com/patent/US20200193684A1/en);
the tested results do not establish equivalence for other rays or inputs.

An optional `--wide-fallback` in the edge and performance controls uses
binary64 only for exceptional-range operations and constructs float32 bits
explicitly. It matches the sampled extreme-scale and underflow controls on
both GPUs, but has not established behavior for every floating-point input or
device capability. Its sample microbenchmark is slower than the float-only
variant; neither variant is enabled for games.

For a repeatable broader check, run the generated synthetic sweep after
building the native probe:

```sh
LD_LIBRARY_PATH="$HSA_ROOT/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" python tools/check_native_rtz_sweep.py --probe /tmp/tuxbox-bvh-probe --code /tmp/tuxbox-bvh-probe.hsaco --proton /path/to/proton --prefix-amd /path/to/amd-prefix --prefix-nvidia /path/to/nvidia-prefix --amd-vulkan /path/to/radv.json --output /tmp/tuxbox-bvh-rtz-sweep.json
```

Add `--oblique` for varied directions and magnitudes, `--tied` for tied
dominant axes, `--miss` for offset rays, `--edge` for triangle-edge rays, or
`--nearedge` for rays offset by one float ULP. Edge modes require
`--float-rtz`; the default uses positive-Z unit rays.

To replay native edge, degenerate, or extreme-scale cases using runtime-fed
GPU data, generate a dump with `--dump-edges`, `--dump-degenerate`,
`--dump-scales`, `--dump-mixed`, or `--dump-underflow`, then pass it to `tools/check_native_bvh_edges.py` with the
same Proton and prefix options as the differential tool above. The scale
comparison currently returns nonzero because of the documented mismatches.
The mixed-exponent comparison also returns nonzero: three of ten sampled
cases differ by 1–2 ULP even though their final values are finite. Saturating
overflow alone does not fix these cases. See
`evidence/matrix-awakens/bvh-native-mixed-scale.json` for the baseline result.
For user-owned leaves, `--retail-edge` and `--retail-nearedge` generate local
six-case dumps accepted by the same tool; keep those dumps local.

`--dump-precision /tmp/tuxbox-bvh-precision.bin` provides a game-independent
reproducer: it varies one triangle coordinate by 0–15 float ULPs and dumps
the resulting native outputs. Eight of these 16 cases differ from the scalar
helper on the tested GPUs. The native control also tests four ray origins for
each triangle and shows that the hardware determinant can change with origin,
so rounding the final determinant alone cannot reproduce its behavior.

`--dump-fractional /tmp/tuxbox-bvh-fractional.bin` emits a single synthetic
fractional-direction native result. With dynamic inputs this reproduces an
AMD-only mismatch in the binary64 reference; the float32 split-product
candidate matches the native words on the tested Radeon and GeForce.

`tools/check_native_bvh_perf.py` measures a synthetic float32 workload after
checking all 65,536 outputs against the native instruction. Its GPU timestamps
include UAV barriers and cover one triangle with 1,024 distinct ray origins;
they are not a game benchmark, a native-instruction speed comparison, or proof
of general correctness. The control builds the required HSA and D3D12 guests
in a temporary directory:

```sh
python tools/check_native_bvh_perf.py --hsa-root "$HSA_ROOT" --proton /path/to/proton --prefix-amd /path/to/amd-prefix --prefix-nvidia /path/to/nvidia-prefix --amd-vulkan /path/to/radv.json --output /tmp/tuxbox-bvh-perf.json
```

The descriptor encodes a zero base, full 42-bit range, closest-first sorting
and barycentric triangle return mode. Except for the dedicated growth cases,
box growth is zero. The HSA host
checks that its GPU is gfx1036 and that the allocation meets the node alignment
before dispatch. The result is specific to the chosen inputs and GPU. Compare
with `tools/check_frosty_bvh_nodes.py`; its earlier constant-input comparisons
do not establish dynamic GPU arithmetic equivalence.

To inspect immediate BVH descriptor setup from a user-supplied cooked shader,
run the standalone Rust inspector from the project root. It recognises the
verified SGPR 68–71 setup and rejects an unsupported write or branch between
setup and use; it does not inspect dynamically generated descriptors:

```sh
cargo build --manifest-path tools/shader-compiler/Cargo.toml --target-dir /tmp/tuxbox-inspect-build --target x86_64-unknown-linux-gnu --lib --no-default-features
rustc --edition=2021 tools/reference/bvh-node/inspect_shader.rs --extern tuxbox_xshader="$(find /tmp/tuxbox-inspect-build/x86_64-unknown-linux-gnu/debug/deps -maxdepth 1 -name 'libtuxbox_xshader-*.rlib' -print -quit)" -L dependency=/tmp/tuxbox-inspect-build/x86_64-unknown-linux-gnu/debug/deps -o /tmp/tuxbox-inspect-shader
/tmp/tuxbox-inspect-shader /path/to/shader.dxbc
```

For the examined Frosty shader 219, both native intersection instructions
construct descriptors with zero box growth and sorting enabled. See
`evidence/matrix-awakens/bvh-descriptor.json` for the input fingerprint.
