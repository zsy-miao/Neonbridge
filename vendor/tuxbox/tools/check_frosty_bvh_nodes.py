#!/usr/bin/env python3
"""Compare standalone BVH4 node math on the CPU, Radeon and GeForce.

The control covers finite inputs and infinite inverse directions for parallel
slabs, zero box growth and
closest-first child sorting. Supplying --serialized adds the root BOX16 or
BOX32 from a user-owned Frosty serialized BLAS to the synthetic inputs.
The output is a mathematical comparison, not a native AMD-instruction oracle.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain, find_dxc
from project_tuxbox.host import Host

INVALID = 0xffffffff


def words(data):
    return list(struct.unpack("<" + "I" * (len(data) // 4), data))


def box_node(bounds, box16):
    data = struct.pack("<4I", 9, 18, 27, INVALID)
    for bound in bounds:
        data += struct.pack("<6e" if box16 else "<6f", *bound)
    return words(data.ljust(128, b"\0"))


def box_reference(node, box16, origin, inverse, extent, sort_children):
    raw = struct.pack("<32I", *node)
    results = []
    for child in range(4):
        bound = struct.unpack_from("<6e" if box16 else "<6f", raw,
                                   16 + child * (12 if box16 else 24))
        ranges = []
        for axis in range(3):
            if math.isinf(inverse[axis]):
                ranges.append((-math.inf, math.inf) if bound[axis] <= origin[axis] <= bound[axis + 3]
                              else (math.inf, -math.inf))
            else:
                a = (bound[axis] - origin[axis]) * inverse[axis]
                b = (bound[axis + 3] - origin[axis]) * inverse[axis]
                ranges.append((min(a, b), max(a, b)))
        near = max(0.0, *(item[0] for item in ranges))
        far = min(extent, *(item[1] for item in ranges))
        results.append((near, node[child] if near <= far else INVALID))
    if sort_children:
        for a, b in ((0, 2), (1, 3), (0, 1), (2, 3), (1, 2)):
            if results[a][1] == INVALID or (results[b][1] != INVALID and results[b][0] < results[a][0]):
                results[a], results[b] = results[b], results[a]
    return [item[1] for item in results]


def subtract(a, b):
    return tuple(x - y for x, y in zip(a, b))


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0])


def dot(a, b):
    return sum(x * y for x, y in zip(a, b))


def triangle_reference(origin, direction, a, b, c):
    e1, e2, v = subtract(b, a), subtract(c, a), subtract(origin, a)
    p, q = cross(direction, e2), cross(v, e1)
    determinant = dot(e1, p)
    t, i, j = dot(e2, q), dot(v, p), dot(direction, q)
    if not determinant:
        return [math.inf, 0.0, i, j]
    u, v = i / determinant, j / determinant
    missed = t / determinant < 0 or u < 0 or v < 0 or u + v > 1
    axis = max(range(3), key=lambda x: abs(direction[x]))
    if abs(direction[axis]) > 0:
        x, y = (axis + 1) % 3, (axis + 2) % 3
        def project(point):
            return (point[x] - direction[x] * point[axis] / direction[axis],
                    point[y] - direction[y] * point[axis] / direction[axis])
        def owns_edge(start, end, inside):
            start, end, inside = map(project, (start, end, inside))
            nx, ny = start[1] - end[1], end[0] - start[0]
            if nx * (inside[0] - start[0]) + ny * (inside[1] - start[1]) < 0:
                nx, ny = -nx, -ny
            return nx > 0 or (nx == 0 and ny * direction[axis] < 0)
        missed |= u == 0 and not owns_edge(a, c, b)
        missed |= v == 0 and not owns_edge(a, b, c)
        missed |= u + v == 1 and not owns_edge(b, c, a)
    if missed:
        return [math.copysign(math.inf, determinant), determinant, i, j]
    return [t, determinant, i, j]


def vector(value):
    def component(v):
        if math.isinf(v):
            return "asfloat(0x7f800000u)" if v > 0 else "asfloat(0xff800000u)"
        return f"{v:.9f}"
    return "float3(" + ", ".join(component(v) for v in value) + ")"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--proton", type=Path, required=True)
    parser.add_argument("--prefix-amd", type=Path, required=True)
    parser.add_argument("--prefix-nvidia", type=Path, required=True)
    parser.add_argument("--amd-vulkan", type=Path, help="optional RADV ICD JSON")
    parser.add_argument("--serialized", type=Path, help="optional user-supplied serialized Xbox BLAS")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    bounds = [
        (-2, -2, 2, 2, 2, 4),
        (-3, -3, 5, 3, 3, 8),
        (10, 10, 10, 12, 12, 12),
        (-1, -1, 0.5, 1, 1, 1),
    ]
    inputs = [(box_node(bounds, kind), kind, "synthetic") for kind in (True, False)]
    source_hash = None
    retail_triangle = None
    retail_pair = None
    if args.serialized:
        data = args.serialized.read_bytes()
        source_hash = hashlib.sha256(data).hexdigest()
        body = data[64:]
        pointer = struct.unpack_from("<I", body, 48)[0]
        if pointer & 7 not in (4, 5):
            raise ValueError("serialized root is not BOX16 or BOX32")
        offset = (pointer >> 3) * 64
        size = 64 if pointer & 7 == 4 else 128
        raw = body[offset:offset + size]
        if len(raw) != size:
            raise ValueError("serialized root extends outside the input")
        inputs.append((words(raw.ljust(128, b"\0")), pointer & 7 == 4, "serialized root"))
        leaf_offset = struct.unpack_from("<I", body, 52)[0]
        if leaf_offset + 64 > len(body):
            raise ValueError("serialized leaf extends outside the input")
        positions = struct.unpack_from("<9f", body, leaf_offset)
        retail_triangle = tuple(tuple(positions[3 * i:3 * i + 3]) for i in range(3))
        leaf_count = struct.unpack_from("<I", body, 56)[0]
        for leaf_index in range(leaf_count):
            raw_leaf = body[leaf_offset + leaf_index * 64:leaf_offset + (leaf_index + 1) * 64]
            if len(raw_leaf) != 64:
                break
            leaf_words = words(raw_leaf)
            selectors = leaf_words[15]
            if not (leaf_words[12] & 0x80 and (selectors & 0xff) in (9, 2, 4)
                    and ((selectors >> 8) & 0xff) in (9, 2, 4)):
                continue
            vertices = struct.unpack_from("<12f", raw_leaf)
            vertices = [tuple(vertices[3*i:3*i+3]) for i in range(4)]
            edge_a, edge_b = subtract(vertices[3], vertices[1]), subtract(vertices[2], vertices[1])
            if abs(cross(edge_a, edge_b)[2]) > 1e-4:
                retail_pair = (leaf_words + [0] * 16, vertices)
                break
    boxes = []
    for node, box16, label in inputs:
        for origin, inverse, extent, sorting in [
            ((0.25, 0.25, -4), (1, 1, 1), 100, True),
            ((0, 0, -4), (0.25, 0.25, 1), 100, True),
            ((100, 100, -4), (1, 1, 1), 100, True),
            ((0, 0, -4), (0.25, 0.25, 1), 5, True),
            ((0, 0, 12), (-0.25, -0.25, -1), 100, True),
            ((0, 0, 3), (0.25, 0.25, 1), 100, True),
            ((0, 0, -4), (0.25, 0.25, 1), 100, False),
        ]:
            boxes.append((node, box16, label, origin, inverse, extent, sorting))
    # The network can reorder equal distances when an earlier lane misses.
    # Keep these distinct from a stable sort even though native tie order is
    # not established without an instruction-level hardware oracle.
    ties = [(10, 10, 2, 12, 12, 4), (-1, -1, 2, 1, 1, 4),
            (-1, -1, 2, 1, 1, 4), (-1, -1, 2, 1, 1, 4)]
    for box16 in (True, False):
        node = box_node(ties, box16)
        node[3] = 36
        origin, inverse = (0, 0, -4), (10, 10, 1)
        assert box_reference(node, box16, origin, inverse, 100, True) == [27, 18, 36, INVALID]
        boxes.append((node, box16, "equal-distance lanes", origin, inverse, 100, True))
    parallel_bounds = [(-1, -1, 2, 1, 1, 4), (-1, -1, 3, 1, 1, 5),
                       (1, -1, 2, 2, 1, 4), (-2, -2, 2, -1, 2, 4)]
    for kind, sign, x in ((True, 1, -1), (True, -1, 1), (False, 1, 2), (False, -1, 3)):
        node = box_node(parallel_bounds, kind)
        node[3] = 36
        boxes.append((node, kind, "parallel slabs", (x, 0, -4),
                      (sign * math.inf, sign * math.inf, 1), 100, True))
    triangles = [
        ((0.25, 0.25, -1), (0, 0, 1), (0, 0, 0), (1, 0, 0), (0, 1, 0)),
        ((0.25, 0.25, 1), (0, 0, -1), (0, 0, 0), (1, 0, 0), (0, 1, 0)),
        ((2, 2, -1), (0, 0, 1), (0, 0, 0), (1, 0, 0), (0, 1, 0)),
        ((0.25, 0.25, 1), (0, 0, 1), (0, 0, 0), (1, 0, 0), (0, 1, 0)),
        ((0.25, 0.25, -1), (0, 0, 1), (0, 0, 0), (1, 0, 0), (2, 0, 0)),
        ((0.5, 0.5, 0), (0, 0, 1), (0, 0, 0), (1, 0, 0), (0, 1, 0)),
        ((0.25, 0.25, -1), (1, 0, 0), (0, 0, 0), (1, 0, 0), (0, 1, 0)),
        ((-0.5, -0.5, -1), (1, 1, 1), (0, 0, 0), (1, 0, 0), (0, 1, 0)),
        ((2, 2, -1), (0, 0, 1), (0, 0, 0), (2, 0, 0), (0, 2, 0)),
        ((2, 2, 1), (0, 0, -1), (0, 0, 0), (2, 0, 0), (0, 2, 0)),
        ((-0.5, 0.5, -1), (0, 0, 1), (0, 0, 0), (1, 1, 0), (-1, 1, 0)),
        ((0, 1, -1), (0, 0, 1), (0, 0, 0), (1, 1, 0), (-1, 1, 0)),
        ((-0.5, -1.5, -1), (1, 2, 1), (0, 0, 0), (1, 0, 0), (0, 1, 0)),
    ]
    if retail_triangle:
        center = tuple(sum(vertex[axis] for vertex in retail_triangle) / 3 for axis in range(3))
        origin = (center[0], center[1], center[2] - 16.0)
        triangles.append((origin, (0, 0, 1), *retail_triangle))
    expected = []
    shader = (ROOT / "tools/reference/bvh-node/node.hlsl").read_text()
    shader += "\nRWBuffer<uint> output : register(u0);\n[numthreads(1,1,1)] void main() {\n"
    for node, box16, label, origin, inverse, extent, sorting in boxes:
        values = ", ".join(f"0x{w:08x}u" for w in node)
        index = len(expected) * 4
        shader += f"{{ uint node[32] = {{ {values} }}; uint4 r = tb_bvh_boxes(node, {'true' if box16 else 'false'}, {vector(origin)}, {vector(inverse)}, {extent}.0, {'true' if sorting else 'false'});"
        shader += "".join(f" output[{index + i}] = r[{i}];" for i in range(4)) + " }\n"
        expected.append(("box", label, box_reference(node, box16, origin, inverse, extent, sorting)))
    for case in triangles:
        index = len(expected) * 4
        leaf = words(struct.pack("<9f", *(v for vertex in case[2:] for v in vertex)).ljust(128, b"\0"))
        values = ", ".join(f"0x{word:08x}u" for word in leaf)
        shader += f"{{ uint node[32] = {{ {values} }}; float4 r = tb_bvh_triangle_leaf(node, {vector(case[0])}, {vector(case[1])});"
        shader += "".join(f" output[{index + i}] = asuint(r[{i}]);" for i in range(4)) + " }\n"
        expected.append(("triangle", "synthetic", triangle_reference(*case)))
    # Validate each documented selector against independently rotated vertices.
    # The two triangles share an edge but use opposite vertex winding.
    paired_cases = []
    corners = [(0., 0., 0.), (1., 0., 0.), (0., 1., 0.), (1., 1., 0.)]
    for side in (0, 1):
        for selector in (9, 2, 4):
            node = words(struct.pack("<12f", *(v for vertex in corners for v in vertex)).ljust(128, b"\0"))
            node[15] = 9 | (9 << 8)
            node[15] = (node[15] & ~(0xff << (8 * side))) | (selector << (8 * side))
            paired_cases.append((node, corners, side, selector, "synthetic pair"))
    if retail_pair:
        node, corners = retail_pair
        paired_cases.append((node, corners, 1, (node[15] >> 8) & 0xff, "serialized pair"))
    for node, corners, side, selector, label in paired_cases:
        raw_corners = ([0, 1, 2], [1, 3, 2])[side]
        rotation = {9: 0, 2: 1, 4: 2}[selector]
        ordered = raw_corners[rotation:] + raw_corners[:rotation]
        vertices = [corners[i] for i in ordered]
        # Distinct weights catch incorrect barycentric selector permutations.
        center = tuple(sum(weight * vertex[axis] for weight, vertex in
                           zip((0.5, 0.375, 0.125), vertices)) for axis in range(3))
        origin = (center[0], center[1], center[2] - 16)
        direction = (0, 0, 1)
        values = ", ".join(f"0x{word:08x}u" for word in node)
        index = len(expected) * 4
        shader += f"{{ uint node[32] = {{ {values} }}; float4 r = tb_bvh_paired_triangle(node, {side}u, {vector(origin)}, {vector(direction)});"
        shader += "".join(f" output[{index + i}] = asuint(r[{i}]);" for i in range(4)) + " }\n"
        expected.append(("triangle", label, triangle_reference(origin, direction, *vertices)))
    shader += "}\n"
    if len(expected) * 4 > 192:
        raise ValueError("control exceeds the guarded UAV")
    report = {"scope": "Finite-input BVH node math compared with CPU calculations; no native instruction oracle",
              "constant_folded_limitation": "HLSL node and ray inputs were compiled as constants and DXC may have folded arithmetic. These results do not establish dynamic GPU arithmetic equivalence; use controls with dynamic inputs.",
              "serialized_sha256": source_hash, "box_cases": len(boxes),
              "triangle_cases": len(triangles) + len(paired_cases), "adapters": {}}
    with TemporaryDirectory(prefix="tuxbox-bvh-node-") as directory:
        work = Path(directory)
        hlsl, cso = work / "node.hlsl", work / "node.cso"
        hlsl.write_text(shader)
        dxc = Path(find_dxc(None))
        subprocess.run([dxc, "-T", "cs_6_0", "-E", "main", hlsl, "-Fo", cso], check=True)
        subprocess.run([dxc.with_name("dxv"), cso], check=True)
        report["hlsl_sha256"] = hashlib.sha256(shader.encode()).hexdigest()
        report["dxil_sha256"] = hashlib.sha256(cso.read_bytes()).hexdigest()
        toolchain = Toolchain()
        obj = toolchain.compile(ROOT / "tools/reference/independent-shaders/compute.cpp",
                                work, f"/I{ROOT / 'runtime/d3d12'}")
        exe = work / "node.exe"
        toolchain.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        (work / "cases.txt").write_text("node 0 uint 1\n")
        for name, prefix in (("AMD", args.prefix_amd), ("NVIDIA", args.prefix_nvidia)):
            os.environ.update(VKD3D_FILTER_DEVICE_NAME=name, DXVK_FILTER_DEVICE_NAME=name)
            if name == "AMD" and args.amd_vulkan:
                os.environ["VK_DRIVER_FILES"] = str(args.amd_vulkan)
            elif name == "NVIDIA":
                os.environ.pop("VK_DRIVER_FILES", None)
            result = Host(None, str(args.proton), prefix).run(exe, [], timeout=45, capture=True)
            if result.returncode:
                raise RuntimeError(f"{name} GPU control failed: {result.returncode}")
            receipt = (work / "result.txt").read_text()
            adapter = next((line.removeprefix("adapter=") for line in receipt.splitlines()
                            if line.startswith("adapter=")), None)
            vendor = "1002:" if name == "AMD" else "10de:"
            if adapter is None or not adapter.lower().startswith(vendor):
                raise ValueError(f"{name} selected unexpected adapter {adapter!r}")
            actual = words((work / "node.bin").read_bytes())
            base = 96
            for case_index, (kind, label, want) in enumerate(expected):
                got = actual[base + case_index * 4:base + case_index * 4 + 4]
                if kind == "triangle":
                    got = [struct.unpack("<f", struct.pack("<I", v))[0] for v in got]
                    if not all(math.isclose(a, b, rel_tol=1e-6, abs_tol=1e-6) for a, b in zip(got, want)):
                        raise ValueError(f"{name} {label} {kind} {case_index}: {got} != {want}")
                elif got != want:
                    raise ValueError(f"{name} {label} {kind} {case_index}: {got} != {want}")
            end = base + len(expected) * 4
            if any(value != 0x76540000 + i for i, value in enumerate(actual) if i < base or i >= end):
                raise ValueError(f"{name} output guard changed")
            report["adapters"][name] = {"adapter": adapter, "exit": result.returncode, "cases": len(expected),
                                         "guard_words": len(actual) - len(expected) * 4}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
