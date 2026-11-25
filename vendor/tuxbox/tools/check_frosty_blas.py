#!/usr/bin/env python3
"""Validate Frosty's BLAS prebuild and command-list build against desktop D3D12.

Select a Proton distribution and optional Xbox provider. Both runs build two
triangle geometries on the same device as their TLAS and inline ray query.
--prebuild-only tests just the provider's sizing and uses a desktop build.
"""
import argparse
import json
import math
import os
import re
from pathlib import Path
import shutil
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain, find_dxc
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.runtime_shaders import runtime_shader_compiler
from project_tuxbox.shader_prep import shader_compiler


SPHERE_SHA256 = "5381a79d200954ba9c317f325cdb10fa476e420c6915971dfc11458a1602f9a1"
TRIANGLE = struct.Struct("<9f")
RAY = struct.Struct("<8fI")
CULL_OPAQUE = 0x40
CULL_NON_OPAQUE = 0x80
MISS = (0xffffffff, 0xffffffff, 0xffffffff, 0)
HIT = struct.Struct("<3If")


def sphere_probes(path, work, shadow_tlas=False):
    # The audited stream is the indexed render mesh expanded in primitive-ID order.
    if digest(path) != SPHERE_SHA256:
        raise ValueError("Expected the verified retail sphere triangle stream")
    shutil.copyfile(path, work / "sphere.xyz")
    triangles = (work / "sphere.xyz").read_bytes()
    rays = bytearray()
    expected = []
    for instance in range(2):
        for primitive, v in enumerate(TRIANGLE.iter_unpack(triangles)):
            center = [sum(v[axis::3]) / 3 for axis in range(3)]
            radius = math.hypot(*center)
            normal = [x / radius for x in center]
            distance = 1 + (primitive % 13) * 0.25
            if shadow_tlas and instance == 1:
                center = [512 - 2 * center[1] + 0.5 * center[2],
                          0.5 * center[0] + 0.25 * center[2], 1.5 * center[2]]
                normal = [-0.5 * normal[1], 2 * normal[0],
                          -normal[0] / 3 + normal[1] / 6 + 2 * normal[2] / 3]
                length = math.hypot(*normal)
                normal = [component / length for component in normal]
            origin = [x + n * distance for x, n in zip(center, normal)]
            if not shadow_tlas or instance == 0:
                origin[0] += 512 * instance
            for flags in (0, CULL_OPAQUE, CULL_NON_OPAQUE):
                rays.extend(RAY.pack(*origin, 0, *(-n for n in normal), distance + 1, flags))
                expected.append(MISS if flags == CULL_NON_OPAQUE else (7 + instance, 0, primitive, distance))
    if shadow_tlas:
        for v in TRIANGLE.iter_unpack(triangles):
            center = [sum(v[axis::3]) / 3 for axis in range(3)]
            radius = math.hypot(*center)
            normal = [x / radius for x in center]
            origin = [x + n for x, n in zip(center, normal)]
            origin[0] += 1024
            rays.extend(RAY.pack(*origin, 0, *(-n for n in normal), 2, 0))
            expected.append(MISS)
    for x in (0, 256, 512):
        rays.extend(RAY.pack(x, 200, -180, 0, 0, 0, 1, 360, 0))
        expected.append(MISS)
    (work / "sphere-rays.bin").write_bytes(rays)
    return expected


def sphere_results(path, expected):
    hits = list(HIT.iter_unpack(path.read_bytes()))
    if len(hits) != len(expected):
        raise ValueError(f"Expected {len(expected)} retail sphere ray results, got {len(hits)}")
    worst = 0.0
    for i, (hit, wanted) in enumerate(zip(hits, expected)):
        distance_error = abs(hit[3] - wanted[3])
        if hit[:3] != wanted[:3] or not distance_error <= 0.0002:
            raise ValueError(f"Sphere ray {i} returned {hit}, expected {wanted}")
        worst = max(worst, distance_error)
    misses = expected.count(MISS)
    return dict(hits=len(expected) - misses, misses=misses, max_distance_error=worst, readback_sha256=digest(path))


MASKED_MATERIAL_HITS = (0, 31, 520, 5384, 31834)
MATERIAL_HITS = (1840, 2036, 3559, 15261, 17501, 17901, 18610, 18676, 18964, 19004, 28725, 29878, 29890, 30995, 38210)


def extract_material_states(mount, work, selected=MATERIAL_HITS):
    from check_frosty_shaders import ARCHIVES, ENTRY, decoder, decompress, parse_tables, serialized_state
    import hashlib

    offset, size, expected = ARCHIVES["material"]
    with (mount / "Frosty/Content/Paks/Frosty-XSX.pak").open("rb") as archive:
        archive.seek(offset)
        data = memoryview(archive.read(size))
    if hashlib.sha256(data).hexdigest() != expected:
        raise ValueError("Unexpected material archive fingerprint")
    _, _, table, _, payload = parse_tables(data)
    lz4 = decoder()
    for entry in selected:
        row = ENTRY.unpack_from(table, entry * ENTRY.size)
        _, state = serialized_state(decompress(payload, row, lz4))
        (work / f"material-{entry}.sptr").write_bytes(state)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", type=Path)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--prebuild-only", action="store_true")
    mode.add_argument("--copy-only", action="store_true", help="Verify Xbox clone and compact with ray queries against both copies")
    mode.add_argument("--deserialize", type=Path, metavar="FILE",
                      help="Rebuild the verified serialized retail sphere through Xbox slot 119 and trace it through slot 131")
    parser.add_argument("--sphere-vertices", type=Path, metavar="FILE",
                        help="ray-test retail sphere vertices from check_frosty_serialized_blas.py --vertices FILE")
    parser.add_argument("--shadow-tlas", type=Path, metavar="MOUNT",
                        help="compare transformed and masked Xbox TLAS traversal against desktop ray queries")
    rays = parser.add_mutually_exclusive_group()
    rays.add_argument("--ray-states", type=Path, metavar="DIR",
                        help="translate and dispatch retail 1280/1282/1284/1286.sptr against two test instances")
    rays.add_argument("--ray-archive", type=Path, metavar="MOUNT",
                      help="read and dispatch the same four states directly from the retail Global shader archive")
    parser.add_argument("--xshader", type=Path, help="select the translator for retail ray libraries")
    parser.add_argument("--screen-occlusion", action="store_true",
                        help="dispatch the packaged 3801 and 3824 shadow raygens with controlled texture inputs")
    parser.add_argument("--voxel-occlusion", action="store_true", help="also dispatch the virtual voxel shadow variant")
    parser.add_argument("--screen-gather", action="store_true", help="dispatch Frosty’s two screen-probe gather ray generators against a compacted ray queue")
    parser.add_argument("--card-tiles", action="store_true", help="dispatch the direct-light shadow masks and radiosity atlas tile programs")
    parser.add_argument("--radiance-cache", action="store_true", help="dispatch a radiance-cache tile and inspect its packed colors and distances")
    parser.add_argument("--lumen-volume", action="store_true", help="dispatch the two packaged Lumen translucency ray generators")
    parser.add_argument("--lumen-reflections", action="store_true", help="dispatch the material and compact reflection raygens used by the opening scene")
    parser.add_argument("--lumen-material", action="store_true", help="dispatch the packaged five-word Lumen geometry closest-hit and miss pair")
    parser.add_argument("--material-hits", action="store_true", help="check material closest-hit shaders observed in retail ray pipelines")
    parser.add_argument("--masked-materials", action="store_true", help="check packaged material any-hit filtering and closest-hit linkage")
    parser.add_argument("--lighting-hits", action="store_true", help="dispatch the retail lighting miss and hidden material shaders")
    parser.add_argument("--single-build", action="store_true", help="use Xbox List4 slot 117 and compare inline BLAS size outputs with explicit size queries")
    parser.add_argument("--gpu-instances", action="store_true", help="populate the TLAS instance array on the GPU before the scene build and update its transforms after Close")
    parser.add_argument("--reuse-blas", action="store_true", help="reuse a cooked BLAS address across sizes, then check its native pointer and trace the rebuilt scene")
    parser.add_argument("--promote-blas", action="store_true", help="trace direct and relocated versions of one guest BLAS in queue execution order")
    parser.add_argument("--ambiguous-as", action="store_true", help="reject ambiguous overlapping placed acceleration-structure buffers")
    args = parser.parse_args()
    if args.xshader:
        # The Xbox runtime compiler must use the worker that built the desktop control.
        os.environ["TUXBOX_XSHADER"] = str(args.xshader.resolve())
    material_entries = (MATERIAL_HITS if args.material_hits else ()) + (MASKED_MATERIAL_HITS if args.masked_materials else ())
    if args.shadow_tlas and not args.sphere_vertices:
        parser.error("--shadow-tlas needs --sphere-vertices")
    if args.ambiguous_as and (not args.provider or args.copy_only or args.prebuild_only):
        parser.error("--ambiguous-as requires a provider and a build run")
    if args.reuse_blas and args.promote_blas:
        parser.error("choose one BLAS reuse control per run")
    if args.reuse_blas or args.promote_blas:
        if args.sphere_vertices:
            parser.error("BLAS reuse controls use generated two- and fourteen-triangle scenes")
        args.gpu_instances = True
    if args.gpu_instances and (not args.provider or args.copy_only or args.prebuild_only):
        parser.error("--gpu-instances requires a provider and a build run")
    if args.single_build and (not args.provider or args.prebuild_only or args.copy_only):
        parser.error("--single-build needs an Xbox provider and a build run")
    if (material_entries or args.lumen_material or args.lighting_hits) and not (args.ray_states or args.ray_archive):
        parser.error("material and lighting probes need --ray-states or --ray-archive")
    if args.card_tiles and not (args.ray_states or args.ray_archive):
        parser.error("--card-tiles needs --ray-states or --ray-archive")
    if args.voxel_occlusion or args.lumen_volume or args.lumen_reflections or args.radiance_cache or args.screen_gather:
        args.screen_occlusion = True
    if (args.prebuild_only or args.copy_only) and not args.provider:
        parser.error("select --provider for an Xbox test")
    if (args.ray_states or args.ray_archive) and args.sphere_vertices:
        parser.error("the ray library probe needs the two test triangle geometries")
    if args.xshader and not (args.ray_states or args.ray_archive):
        parser.error("--xshader requires --ray-states or --ray-archive")
    if args.screen_occlusion and not (args.ray_states or args.ray_archive):
        parser.error("--screen-occlusion requires --ray-states or --ray-archive")
    if args.xshader:
        # Pin the in-process Proton compiler worker to the desktop control's translator.
        os.environ["TUXBOX_XSHADER"] = str(args.xshader.resolve())
    if args.deserialize and (not args.provider or not args.sphere_vertices or
                             digest(args.deserialize) != "38bd71be9dd00e74952a9612f87f7df37a1617ca9f9847bb531b40bc5a4ad6af"):
        parser.error("--deserialize needs the retail sphere payload, --sphere-vertices and --provider")
    host = Host(None, args.proton, args.prefix)
    src = ROOT / "tools/reference/blas"
    build = ROOT / "build/frosty-blas"
    build.mkdir(exist_ok=True, parents=True)
    with TemporaryDirectory(dir=build) as tmp:
        work = Path(tmp)
        subprocess.run([find_dxc(None), "-T", "cs_6_5", "-E", "main", "-O3", "-Fh", "query_dxil.h",
                        "-Vn", "query_dxil", str(src / "query.hlsl")], cwd=work, check=True)
        probe_flags, ray_inputs, graph_rejections = [], {}, []
        if args.shadow_tlas:
            probe_flags.append("/DTUXBOX_SHADOW_TLAS_PROBE")
        if args.ambiguous_as:
            probe_flags.append("/DTUXBOX_AMBIGUOUS_AS_PROBE")
        if args.reuse_blas:
            probe_flags.append("/DTUXBOX_REUSED_BLAS_PROBE")
        if args.promote_blas:
            probe_flags.append("/DTUXBOX_PROMOTED_BLAS_PROBE")
        if args.gpu_instances:
            probe_flags.append("/DTUXBOX_GPU_INSTANCE_PROBE")
            subprocess.run([find_dxc(None), "-T", "cs_6_0", "-E", "main", "-O3", "-Fh", work / "instance_bindings_dxil.h",
                            "-Vn", "instance_bindings_dxil", src / "instance_bindings.hlsl"], check=True)
        if args.single_build:
            probe_flags.append("/DTUXBOX_SINGLE_BUILD_PROBE")
        ray_entries = (1280, 1282, 1284, 1286, 3801, 3824) if args.screen_occlusion else (1280, 1282, 1284, 1286)
        if args.voxel_occlusion:
            ray_entries += (3810,)
            probe_flags.append("/DTUXBOX_VOXEL_OCCLUSION_PROBE")
        if args.lumen_reflections:
            ray_entries += (467, 425, 456, 421, 432)
            probe_flags.append("/DTUXBOX_REFLECTION_PROBE")
        if args.lumen_reflections or args.radiance_cache:
            subprocess.run([find_dxc(None), "-T", "lib_6_3", "-O3", "-DTRACE_PROBE_INPUTS", "-Fo",
                            work / "ray-input-observer.dxil", src / "occlusion_observer.hlsl"], check=True)
        if args.lumen_material:
            ray_entries += (1237, 1238)
        if args.lighting_hits:
            ray_entries += (2670, 2752)
            probe_flags.append("/DTUXBOX_LIGHTING_PROBE")
            subprocess.run([find_dxc(None), "-T", "lib_6_3", "-O3", "-Fo", work / "lighting-observer.dxil",
                            src / "lighting_observer.hlsl"], check=True)
        if args.card_tiles:
            ray_entries += (218, 1064)
            probe_flags.append("/DTUXBOX_CARD_TILE_PROBE")
            subprocess.run([find_dxc(None), "-T", "cs_6_5", "-E", "main", "-O3", "-Fh", work / "card_candidate_dxil.h",
                            "-Vn", "card_candidate_dxil", src / "card_candidate.hlsl"], check=True)
            subprocess.run([find_dxc(None), "-T", "cs_6_5", "-E", "main", "-O3", "-Fh", work / "zero_aabb_dxil.h",
                            "-Vn", "zero_aabb_dxil", src / "zero_aabb.hlsl"], check=True)
            subprocess.run([find_dxc(None), "-T", "cs_6_0", "-E", "main", "-O3", "-Fh", work / "sidecar_bounds_dxil.h",
                            "-Vn", "sidecar_bounds_dxil", src / "sidecar_bounds.hlsl"], check=True)
            subprocess.run([find_dxc(None), "-T", "cs_6_0", "-E", "main", "-O3", "-Fh", work / "sidecar_map_dxil.h",
                            "-Vn", "sidecar_map_dxil", src / "sidecar_map.hlsl"], check=True)
            candidate_source = work / "inline219_probe.hlsl"
            candidate_source.write_text((ROOT / "tools/reference/bvh-node/rtz_triangle.hlsl").read_text() + "\n" +
                                        (src / "inline219_candidate.hlsl").read_text() + "\n" +
                                        (src / "inline219_probe.hlsl").read_text())
            subprocess.run([find_dxc(None), "-T", "cs_6_5", "-E", "main", "-O3", "-Gis",
                            "-D", "TUXBOX_RTZ_FLOAT32=1", "-D", "TUXBOX_RTZ_WIDE_FALLBACK=1",
                            "-Fh", work / "inline219_dxil.h", "-Vn", "inline219_dxil", candidate_source], check=True)
            generator = work / "sidecar-fixture"
            subprocess.run(["rustc", "--edition=2024", src / "sidecar_fixture.rs", "-o", generator], check=True)
            fixture_args = [generator, work]
            if args.ray_archive:
                from check_frosty_serialized_blas import MESHES, read_asset
                sphere = MESHES["sphere"]
                sphere_data = read_asset(args.ray_archive, sphere)
                serialized = sphere_data[sphere.blas_offset:sphere.blas_offset + sphere.blas_size]
                sphere_serialized = work / "sphere-sidecar.serialized"
                sphere_serialized.write_bytes(serialized)
                sphere_generator = work / "sphere-sidecar-generator"
                subprocess.run(["rustc", "--edition=2024", "-A", "dead_code", "-O",
                                src / "sphere_sidecar.rs", "-o", sphere_generator], check=True)
                subprocess.run([sphere_generator, sphere_serialized, work], check=True)
                invalid_dir = work / "invalid-sidecar"
                invalid_dir.mkdir()
                root = struct.unpack_from("<I", serialized, 64 + 48)[0]
                if root & 7 != 4:
                    raise ValueError("sphere sidecar control expects a BOX16 root")
                root_offset = 64 + (root >> 3) * 64
                children = struct.unpack_from("<4I", serialized, root_offset)
                child = next(i for i, value in enumerate(children) if value != 0xffffffff)
                mutations = {}
                bad = bytearray(serialized)
                struct.pack_into("<I", bad, 64 + 48, (root & ~7) | 2)
                mutations["unsupported_root_type"] = bad
                bad = bytearray(serialized)
                struct.pack_into("<I", bad, root_offset + child * 4, root)
                mutations["root_cycle"] = bad
                bad = bytearray(serialized)
                struct.pack_into("<H", bad, root_offset + 16 + child * 12 + 6, 0x7e00)
                mutations["nan_child_bound"] = bad
                mutations["truncated"] = serialized[:-64]
                bad_path = invalid_dir / "bad.serialized"
                for name, bad in mutations.items():
                    bad_path.write_bytes(bad)
                    rejected = subprocess.run([sphere_generator, bad_path, invalid_dir], capture_output=True)
                    if rejected.returncode == 0 or (invalid_dir / "sphere-sidecar-directory.bin").exists():
                        raise ValueError(f"invalid sphere graph was not rejected: {name}")
                    graph_rejections.append(name)
                leaf_offset = struct.unpack_from("<I", serialized, 64 + 52)[0]
                leaf_count = struct.unpack_from("<I", serialized, 64 + 12)[0]
                for index in range(leaf_count):
                    leaf = serialized[64 + leaf_offset + 64 * index:64 + leaf_offset + 64 * (index + 1)]
                    if len(leaf) == 64 and struct.unpack_from("<I", leaf, 48)[0] & 0xff == 0x80:
                        retail_leaf = work / "sidecar-retail-leaf.bin"
                        retail_leaf.write_bytes(leaf)
                        fixture_args.append(retail_leaf)
                        break
                else:
                    raise ValueError("sphere has no paired TRI0/TRI1 leaf")
            subprocess.run(fixture_args, check=True)
            subprocess.run([find_dxc(None), "-T", "lib_6_3", "-O3", "-Fo", work / "occlusion-observer.dxil",
                            src / "occlusion_observer.hlsl"], check=True)
            subprocess.run([find_dxc(None), "-T", "lib_6_3", "-O3", "-DTRACE_CARD_INPUTS", "-Fo",
                            work / "card-input-observer.dxil", src / "occlusion_observer.hlsl"], check=True)
        if args.screen_gather:
            subprocess.run([find_dxc(None), "-T", "lib_6_3", "-O3", "-Fo", work / "screen-gather-observer.dxil",
                            src / "screen_gather_observer.hlsl"], check=True)
            ray_entries += (64, 71)
            probe_flags.append("/DTUXBOX_SCREEN_GATHER_PROBE")
        if args.radiance_cache:
            ray_entries += (323, 299, 311, 264, 266)
            probe_flags.append("/DTUXBOX_RADIANCE_PROBE")
        if args.lumen_volume:
            ray_entries += (619, 620)
            probe_flags.append("/DTUXBOX_LUMEN_VOLUME_PROBE")
        if args.ray_archive:
            from check_frosty_shaders import decoder, decompress, global_archive, serialized_state
            payload, entries = global_archive(args.ray_archive)
            lz4 = decoder()
            for entry in ray_entries:
                _, state = serialized_state(decompress(payload, entries[entry], lz4))
                (work / f"{entry}.sptr").write_bytes(state)
        material_names = []
        if material_entries:
            if args.ray_archive:
                extract_material_states(args.ray_archive, work, material_entries)
            ray_entries += tuple(f"material-{i}" for i in material_entries)
        if material_entries or args.lumen_material:
            probe_flags.append("/DTUXBOX_MATERIAL_HIT_PROBE")
        if args.ray_states or args.ray_archive:
            translator = args.xshader.resolve() if args.xshader else shader_compiler()
            for entry in ray_entries:
                state = (args.ray_states or work) / f"{entry}.sptr"
                if args.ray_states and args.provider:
                    shutil.copyfile(state, work / state.name)
                hlsl, profile, dxil = (work / f"{entry}.{ext}" for ext in ("hlsl", "profile", "dxil"))
                generation = subprocess.run([translator, state, "--hlsl-out", hlsl, "--profile-out", profile],
                                            check=True, text=True, stdout=subprocess.PIPE)
                if entry == 1238 or isinstance(entry, str) and entry.startswith("material-"):
                    generated = hlsl.read_text()
                    export = re.fullmatch(r"entry=((?:LumenHardwareRayTracing)?MaterialCHS_[a-f0-9]{8})", generation.stdout.strip())
                    if export is None:
                        raise ValueError(f"Unexpected closest-hit export for {entry}")
                    views = ["0"] * 64
                    kinds = {"StructuredBuffer<uint4>": "s", "Buffer<float4>": "b",
                             "Texture2D<uint4>": "i", "Texture2D<float4>": "f"}
                    for kind, slot in re.findall(r"(\w+<\w+>) tbSrv\d+ : register\(t(\d+),space0\)", generated):
                        views[int(slot)] = kinds[kind]
                    if entry != 1238 and "s" not in views:
                        raise ValueError(f"Missing material scene buffers for {entry}")
                    raw = state.read_bytes()
                    pattern = r"HitGroup_[a-f0-9]{8}_[a-f0-9]{8}"
                    groups = {g for k in (0, 1) for g in re.findall(pattern, raw[k:].decode("utf-16-le", "ignore"))}
                    if len(groups) != 1:
                        raise ValueError(f"Unexpected hit group in {entry}")
                    any_hit = re.search(r'void (MaterialAHS_[a-f0-9]{8})\(', generated)
                    if entry in {f"material-{i}" for i in MASKED_MATERIAL_HITS} and not any_hit:
                        raise ValueError(f"Missing any-hit export for {entry}")
                    any_name = f'L"{any_hit.group(1)}"' if any_hit else 'nullptr'
                    material_names.append(f'{{"{entry}", L"{export.group(1)}", L"{groups.pop()}", "{"".join(views)}", {any_name}}},')
                if profile.read_text() != "lib_6_3":
                    raise ValueError(f"Unexpected desktop profile for state {entry}")
                subprocess.run([find_dxc(None), "-T", "lib_6_3", "-O3", "-Fo", dxil, hlsl], check=True)
                ray_inputs[entry] = {"state_sha256": digest(state), "dxil_sha256": digest(dxil)}
            probe_flags.append("/DTUXBOX_RAY_LIBRARY_PROBE")
            if args.screen_occlusion:
                subprocess.run([find_dxc(None), "-T", "lib_6_3", "-O3", "-Fo", work / "occlusion-observer.dxil",
                                src / "occlusion_observer.hlsl"], check=True)
                probe_flags.append("/DTUXBOX_SCREEN_OCCLUSION_PROBE")
        if material_entries or args.lumen_material:
            (work / "material_inputs.h").write_text("\n".join(material_names) + "\n")
            subprocess.run([find_dxc(None), "-T", "lib_6_3", "-O3", "-Fo", work / "material-observer.dxil",
                            src / "material_observer.hlsl"], check=True)
        toolchain = Toolchain()
        obj = toolchain.compile(src / "guest.cpp", work, "/W4", "/WX", *probe_flags,
                                f"/I{work}", f"/I{ROOT / 'runtime/d3d12'}")
        exe = work / "blas.exe"
        toolchain.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        result = {"guest_sha256": digest(exe), "host": host.version, "runs": {}}
        if (work / "sidecar-retail-leaf.bin").exists():
            result["sidecar_retail_leaf_sha256"] = digest(work / "sidecar-retail-leaf.bin")
        if graph_rejections:
            result["sphere_sidecar_graph_rejections"] = graph_rejections
            result["sphere_sidecar_serialized_sha256"] = digest(work / "sphere-sidecar.serialized")
        if args.single_build:
            result["xbox_build_slot"] = 117
        if ray_inputs:
            result["translated_ray_libraries"] = ray_inputs
            result["translator_sha256"] = digest(translator)
        modes = ["desktop"]
        if args.deserialize:
            shutil.copyfile(args.deserialize, work / "sphere.serialized")
            result["serialized_sha256"] = digest(args.deserialize)
        if args.provider:
            shutil.copyfile(args.provider, work / "d3d12_xs.dll")
            result["d3d12_xs_sha256"] = digest(work / "d3d12_xs.dll")
            modes.append("xbox-deserialize" if args.deserialize else "xbox-copy" if args.copy_only else "xbox-prebuild" if args.prebuild_only else "xbox")
        expected_rays = sphere_probes(args.sphere_vertices, work, bool(args.shadow_tlas)) if args.sphere_vertices else None
        shadow_hits = None
        if args.shadow_tlas:
            from check_frosty_serialized_blas import MESHES, read_asset
            sphere = MESHES["sphere"]
            asset = read_asset(args.shadow_tlas, sphere)
            serialized = work / "shadow-sphere.serialized"
            serialized.write_bytes(asset[sphere.blas_offset:sphere.blas_offset + sphere.blas_size])
            control = work / "shadow-tlas-reference"
            subprocess.run(["rustc", "--edition=2024", "-A", "dead_code", "-O",
                            ROOT / "tools/reference/blas/traverse_serialized.rs", "-o", control], check=True)
            shadow_output = work / "shadow-tlas-hits.bin"
            subprocess.run([control, serialized, work / "sphere.xyz", work / "sphere-rays.bin",
                            shadow_output, "--shadow-tlas"], check=True)
            shadow_hits = list(HIT.iter_unpack(shadow_output.read_bytes()))
            if len(shadow_hits) != len(expected_rays):
                raise ValueError("Xbox TLAS reference returned an unexpected ray count")
            result["shadow_tlas_reference"] = {
                "instances": 3,
                "disabled_instances": 1,
                "source_sha256": digest(serialized),
                "readback_sha256": digest(shadow_output),
                "scope": "Synthetic Xbox BOX32 TLAS and unmodified serialized sphere BLAS",
            }
        if expected_rays:
            result["geometry"] = dict(asset="EngineMeshes/Sphere", triangles=528,
                                      instances=3 if args.shadow_tlas else 2,
                                      xyz_sha256=SPHERE_SHA256, rays_sha256=digest(work / "sphere-rays.bin"),
                                      distance_tolerance=0.0002, guest_blas_bytes=31232,
                                      geometry_flags="NONE", query_flags=["NONE", "CULL_OPAQUE", "CULL_NON_OPAQUE"],
                                      build_flags="PREFER_FAST_TRACE | ALLOW_COMPACTION")
        passed = True
        for mode in modes:
            receipt = work / (mode + ".txt")
            output = work / "sphere-hits.bin"
            output.unlink(missing_ok=True)
            command = [mode, receipt.name] + (["sphere"] if expected_rays else [])
            if ray_inputs and mode != "desktop":
                with runtime_shader_compiler(host.backend, host.env, exe, print):
                    run = host.run(exe, command, timeout=90, capture=True)
            else:
                run = host.run(exe, command, timeout=60, capture=True)
            observation = dict(line.split("=", 1) for line in receipt.read_text().splitlines()) if receipt.exists() else {}
            if not observation or run.returncode:
                sys.stderr.buffer.write(run.stdout[-4096:])
            result["runs"][mode] = {"exit": run.returncode, **observation}
            if run.returncode or not observation:
                passed = False
            elif args.reuse_blas and mode == "xbox" and observation.get("blas_reuse_guest_bytes") != "192,704,192,704":
                passed = False
            elif args.promote_blas and mode == "xbox" and (
                    observation.get("promotion_first_primitive") != "1,0,1,0" or
                    observation.get("direct_to_relocated_compact_rays") != "1,0,miss;0,1,miss"):
                passed = False
            elif args.single_build and mode == "xbox" and observation.get("inline_postbuild_sizes_matched") != "4":
                passed = False
            elif args.ambiguous_as and mode.startswith("xbox") and (
                    observation.get("ambiguous_as_equal_va") != "1" or
                    observation.get("ambiguous_as_rejections") != "4" or
                    observation.get("unique_as_after_release") != "1"):
                passed = False
            elif expected_rays:
                result["runs"][mode]["ray_queries"] = sphere_results(output, expected_rays)
                if shadow_hits is not None:
                    desktop_hits = list(HIT.iter_unpack(output.read_bytes()))
                    if len(desktop_hits) != len(shadow_hits):
                        raise ValueError("Xbox TLAS and desktop ray counts differ")
                    errors = [abs(native[3] - guest[3]) for native, guest in zip(desktop_hits, shadow_hits)]
                    mismatches = sum(
                        native[:3] != guest[:3] or error > 0.0002
                        for native, guest, error in zip(desktop_hits, shadow_hits, errors)
                    )
                    result["runs"][mode]["shadow_tlas_comparison"] = {
                        "rays": len(desktop_hits), "mismatches": mismatches,
                        "max_distance_error": max(errors, default=0),
                    }
                    passed = passed and mismatches == 0
            elif observation.get("ray_query_results") != "70010,70120,80010,80120,dead":
                passed = False
        if (material_entries or args.lumen_material) and args.provider and "xbox" in result["runs"]:
            desktop = result["runs"]["desktop"]
            xbox = result["runs"]["xbox"]
            hits = [f"material-{entry}" for entry in material_entries]
            if args.lumen_material:
                hits.append("1238")
            masked = {f"material-{i}" for i in MASKED_MATERIAL_HITS}
            fields = [f"hit_{entry}_{ray}" for entry in hits for ray in range(7 if entry in masked else 5)]
            mismatches = [field for field in fields if field not in desktop or desktop[field] != xbox.get(field)]
            result["material_payload_comparison"] = {"rays": len(fields), "words_per_ray": 16, "mismatches": mismatches}
            passed = passed and not mismatches
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return int(not passed)


if __name__ == "__main__":
    sys.exit(main())
