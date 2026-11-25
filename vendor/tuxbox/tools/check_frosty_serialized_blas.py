#!/usr/bin/env python3
"""Compare decoded Xbox BLAS triangles against independently indexed Frosty render meshes."""
import argparse
from dataclasses import dataclass
import hashlib
from pathlib import Path
import struct
import subprocess
from tempfile import TemporaryDirectory
import zlib

ROOT = Path(__file__).resolve().parents[1]


@dataclass(frozen=True)
class Mesh:
    chunk: int
    chunk_id: str
    asset_sha256: str
    asset_size: int
    section_offset: int
    section_count: int
    vertex_offset: int
    vertex_count: int
    index_offset: int
    primitive_count: int
    blas_offset: int
    blas_size: int
    index_width: int = 2


# Frosty 1.20.1831.3754 package members. The render buffers are independent of the
# serialized acceleration structures, including triangle winding and primitive ordering.
MESHES = {
    # Engine/Content/EngineMeshes/Sphere.uasset
    "sphere": Mesh(3, "f421d8f857b9ed0300000001", "612c245f01199b775c095c578216a10aecd58b60feb93bfa4b70cfcbed3b7c10",
                   87910, 0x89fc, 1, 0x8a3e, 323, 0xa8be, 528, 0xda92, 31296),
    # Prop/Kit_StreetLamp_A/Mesh/SM_StreetLamp_A_StopLight_D.uasset, including empty section 1
    "stoplight": Mesh(2521, "67706e209687432300000001", "b0474245979d65556ab66835e30135e386acadd9242e5be426cd39ac5e223e73",
                      229217, 0x12fd, 13, 0x14ef, 572, 0x53bf, 329, 0x91c3, 26560, 4),
    # Vehicle/vehVan_vehicle01/Mesh/SM_vehVan_vehicle01.uasset, two shared terminal leaves
    "van": Mesh(5759, "209286e45f042b1d00000001", "fa4c278b910cd0a7a13e6e5bae3462f2341e59835cdfe48328e76eda7779410f",
                728215, 0x1135, 15, 0x136f, 1337, 0xe497, 731, 0x16dfb, 63488, 4),
    # Vehicle/Helicopter_Huey/Mesh/SM_Fuselage_Helicopter_Huey.uasset, one duplicated primitive
    "helicopter": Mesh(17546, "8459ad823917595200000001", "0694823addea06c72741e701cc7b4b811b258db8de2ac689423bdb173dfb3403",
                       154288, 0xce8, 4, 0xd96, 583, 0x5fd2, 347, 0xa136, 26048, 4),
    # City/P_Construct_Freeway/SM_FREEWAY_VIS.uasset, one duplicated primitive
    "freeway": Mesh(22046, "e2d9d395f2b0fde100000001", "a48abe16504350da656738de7bfbd849ad730827d18a276aa80e07f95d9edb72",
                    113099, 0xad3, 4, 0xb81, 118, 0x1a81, 64, 0x20d5, 4928),
    # Environment/Bullet_Time/Meshes/BG_Buildings/SM_BG_Building_F_1.uasset
    "building": Mesh(20107, "91ab278fd7eaf1c600000001", "c5e3165260f51eb59a63dbf98102d2911e41fe89841aabde18e09ae9f8eb2122",
                     114592, 0xb32, 1, 0xb74, 119, 0x1a94, 63, 0x20d0, 4160),
    # Effect/Vehicle/BrokenGlass/vehVan_vehicle01/taillight_r_trans/taillight_r_trans_hitbox.uasset
    "taillight": Mesh(14091, "06162da21df71cca00000001", "e081b6a4e8bbed7325b6557fc758c488c99e42a2dda883dbeef6609682ced4c1",
                      211030, 0x1ec00, 4, 0x1ecae, 417, 0x213fe, 624, 0x24ed2, 49472),
}


def read_asset(mount, mesh):
    package = mount / "Frosty/Content/Paks/Frosty-XSX"
    with package.with_suffix(".utoc").open("rb") as toc, package.with_suffix(".ucas").open("rb") as data:
        header = toc.read(48)
        if header[:16] != b"-==--==--==--==-" or header[16] != 3:
            raise ValueError("Expected the audited version 3 IoStore container")
        start, entries, blocks, stride, methods, method_length, block_size = struct.unpack_from("<7I", header, 20)
        if start != 144 or stride != 12 or methods != 1 or method_length != 32 or block_size != 65536 or mesh.chunk >= entries:
            raise ValueError("Unexpected IoStore block layout")
        toc.seek(start + 12 * mesh.chunk)
        if toc.read(12).hex() != mesh.chunk_id:
            raise ValueError("The original asset chunk ID differs")
        toc.seek(start + 12 * entries + 10 * mesh.chunk)
        offset = int.from_bytes(toc.read(5), "big")
        length = int.from_bytes(toc.read(5), "big")
        first = offset // block_size
        count = (length + block_size - 1) // block_size
        if offset % block_size or length != mesh.asset_size or first + count > blocks:
            raise ValueError("Unexpected asset chunk extent")
        table = start + 22 * entries
        toc.seek(table + stride * blocks)
        if toc.read(method_length).rstrip(b"\0") != b"Zlib":
            raise ValueError("Expected the original zlib blocks")
        toc.seek(table + stride * first)
        result = bytearray()
        for _ in range(count):
            entry = toc.read(stride)
            packed = int.from_bytes(entry[5:8], "little")
            size = int.from_bytes(entry[8:11], "little")
            if len(entry) != stride or not 0 < packed <= block_size or size != min(block_size, length - len(result)):
                raise ValueError("Invalid IoStore block size")
            data.seek(int.from_bytes(entry[:5], "little"))
            raw = data.read(packed)
            if entry[11] == 1:
                decoder = zlib.decompressobj()
                raw = decoder.decompress(raw, size + 1)
                if not decoder.eof or decoder.unused_data:
                    raise ValueError("Incomplete IoStore compressed block")
            elif entry[11] != 0:
                raise ValueError("Unexpected IoStore block compression")
            if len(raw) != size:
                raise ValueError("IoStore block decompressed to unexpected size")
            result.extend(raw)
    if hashlib.sha256(result).hexdigest() != mesh.asset_sha256:
        raise ValueError("The mesh differs from the audited original")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path, help="original Frosty Mount directory")
    parser.add_argument("--mesh", choices=MESHES, default="sphere")
    parser.add_argument("--vertices", type=Path, help="write verified triangles as unindexed little-endian float3 vertices")
    parser.add_argument("--traversal", action="store_true",
                        help="also check sphere BVH node traversal against the indexed render mesh")
    args = parser.parse_args()
    if args.traversal and args.mesh != "sphere":
        parser.error("--traversal currently supports the sphere mesh")
    try:
        mesh = MESHES[args.mesh]
        asset = read_asset(args.mount, mesh)
        positions = memoryview(asset)[mesh.vertex_offset:mesh.vertex_offset + mesh.vertex_count * 12]
        index_format = {2: "H", 4: "I"}[mesh.index_width]
        indices = struct.unpack_from(f"<{mesh.primitive_count * 3}{index_format}", asset, mesh.index_offset)
        expected = bytearray()
        for geometry in range(mesh.section_count):
            section = struct.unpack_from("<9I", asset, mesh.section_offset + geometry * 36)
            _, first, count, low, high, _, _, force_opaque, _ = section
            if high >= mesh.vertex_count or first + count * 3 > len(indices) or force_opaque:
                raise ValueError("Unexpected original render section extent or geometry flags")
            for primitive in range(count):
                expected.extend(struct.pack("<II", geometry, primitive))
                for i in indices[first + 3 * primitive:first + 3 * primitive + 3]:
                    if not low <= i <= high:
                        raise ValueError("Render index is outside its section's vertex range")
                    expected.extend(positions[i * 12:i * 12 + 12])
        encoded = asset[mesh.blas_offset:mesh.blas_offset + mesh.blas_size]
        with TemporaryDirectory(prefix="tuxbox-blas-control-") as folder:
            control = Path(folder) / "decode"
            subprocess.run(["rustc", "--edition=2024", "-O", ROOT / "tools/reference/blas/decode_serialized.rs",
                            "-o", control], check=True)
            result = subprocess.run([control, "--with-ids"], input=encoded, stdout=subprocess.PIPE, check=True).stdout
        if result != expected or len(result) != mesh.primitive_count * 44:
            raise ValueError("Decoded geometry ID, primitive ID, winding or coordinates differ from the render mesh")
        vertices = b"".join(result[i + 8:i + 44] for i in range(0, len(result), 44))
        if args.traversal:
            with TemporaryDirectory() as directory:
                work = Path(directory)
                control = work / "traverse"
                serialized = work / "sphere.serialized"
                render_mesh = work / "sphere.xyz"
                serialized.write_bytes(encoded)
                render_mesh.write_bytes(vertices)
                subprocess.run(["rustc", "--edition=2024", "-A", "dead_code", "-O",
                                ROOT / "tools/reference/blas/traverse_serialized.rs",
                                "-o", control], check=True)
                subprocess.run([control, serialized, render_mesh], check=True)
        if args.vertices:
            args.vertices.write_bytes(vertices)
        print(f"{args.mesh}: {mesh.primitive_count} Xbox BLAS triangles match the indexed render mesh byte for byte")
        print(f"Triangle SHA-256 {hashlib.sha256(vertices).hexdigest()}")
    except (OSError, ValueError, zlib.error, struct.error, subprocess.SubprocessError) as exc:
        parser.exit(1, f"BLAS verification failed: {exc}\n")


if __name__ == "__main__":
    main()
