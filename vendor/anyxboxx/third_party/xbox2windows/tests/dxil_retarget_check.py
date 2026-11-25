"""Synthetic controls and DXC validator checks for tools/dxil_retarget.py."""

import json
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import dxil_retarget as rt
from resource_inventory import dxil_program, parts

SKIP = 77


def _encode_synthetic_bitcode(*, triple: str, abbreviated_triple: bool, abbreviated_md_name: bool,
                              include_xbox_md: bool, include_blob: bool = False) -> bytes:
    """Build a minimal valid LLVM bitstream exercising BLOCKINFO, abbrevs, and metadata."""
    # BLOCKINFO: default abbrev #0 for METADATA_BLOCK (code=4, Char6 array)
    md_name_ops = [("LIT", rt.METADATA_NAME), (rt.ENC_ARRAY, None), (rt.ENC_CHAR6, None)]
    blockinfo_items = [
        ("UNABBREV", rt.BLOCKINFO_CODE_SETBID, [rt.METADATA_BLOCK_ID]),
        ("DEFINE_ABBREV", md_name_ops),
    ]
    bw = rt.BitWriter()
    rt.write_bitcode_block(bw, 2, blockinfo_items)
    bw.write(rt.ABBREV_END_BLOCK, 2)
    bw.align32()
    blockinfo_raw = bw.finish()

    # Dummy opaque subblock (block id 12 = FUNCTION_BLOCK) to verify raw preservation
    opaque_raw = struct.pack("<2I", 0x11223344, 0x55667788)

    mod_items = [
        ("RAW_BLOCK", rt.BLOCKINFO_BLOCK_ID, 2, blockinfo_raw),
        ("UNABBREV", 1, [1]),  # MODULE_CODE_VERSION
    ]
    if abbreviated_triple:
        triple_ops = [("LIT", rt.MODULE_CODE_TRIPLE), (rt.ENC_ARRAY, None), (rt.ENC_FIXED, 8)]
        mod_items.append(("DEFINE_ABBREV", triple_ops))
        mod_items.append(("ABBREV", 4, triple_ops, rt.MODULE_CODE_TRIPLE, list(triple.encode("ascii")), None))
    else:
        mod_items.append(("UNABBREV", rt.MODULE_CODE_TRIPLE, list(triple.encode("ascii"))))

    if include_blob:
        blob_ops = [("LIT", 9), (rt.ENC_VBR, 6), (rt.ENC_BLOB, None)]
        mod_items.append(("DEFINE_ABBREV", blob_ops))
        mod_items.append(("ABBREV", 5 if abbreviated_triple else 4, blob_ops, 9, [42], b"blob-payload"))

    md_items = [
        ("UNABBREV", 1, list(b"__XBOX_ENABLE_WAVE32")),  # METADATA_STRING
        ("UNABBREV", 1, list(b"1")),
        ("UNABBREV", 3, [1, 2]),  # METADATA_NODE
        ("UNABBREV", 1, list(b"synthetic")),
        ("UNABBREV", 3, [4]),
    ]
    if include_xbox_md:
        if abbreviated_md_name:
            md_items.append(("ABBREV", 4, md_name_ops, rt.METADATA_NAME, list(b"xbox.compiler.options"), None))
        else:
            md_items.append(("UNABBREV", rt.METADATA_NAME, list(b"xbox.compiler.options")))
        md_items.append(("UNABBREV", rt.METADATA_NAMED_NODE, [2]))
    if abbreviated_md_name:
        md_items.append(("ABBREV", 4, md_name_ops, rt.METADATA_NAME, list(b"llvm.ident"), None))
    else:
        md_items.append(("UNABBREV", rt.METADATA_NAME, list(b"llvm.ident")))
    md_items.append(("UNABBREV", rt.METADATA_NAMED_NODE, [4]))

    mod_items.append(("BLOCK", rt.METADATA_BLOCK_ID, 3, md_items))
    mod_items.append(("RAW_BLOCK", 12, 4, opaque_raw))
    return rt.serialize_bitcode([("BLOCK", rt.MODULE_BLOCK_ID, 3, mod_items)])


def _inject_xb2_into_bitcode(bc: bytes, *, abbreviate_name: bool = False) -> bytes:
    """Turn a real desktop DXIL bitcode module into a synthetic dxil-ms-xb2 module."""
    top = rt.parse_bitcode(bc)
    injected = False
    for item in top:
        if item[0] != "BLOCK" or item[1] != rt.MODULE_BLOCK_ID:
            continue
        mod_items = item[3]
        for idx, mod_item in enumerate(mod_items):
            if rt._record_code(mod_item) == rt.MODULE_CODE_TRIPLE:
                mod_items[idx] = ("UNABBREV", rt.MODULE_CODE_TRIPLE, list(rt.SOURCE_TRIPLE.encode("ascii")))
            elif not injected and mod_item[0] == "BLOCK" and mod_item[1] == rt.METADATA_BLOCK_ID:
                md_items = mod_item[3]
                # Find the first existing METADATA_NAMED_NODE operand so the injected named node
                # references a real metadata node without disturbing any existing node IDs.
                first_node_id = None
                for rec in md_items:
                    if rt._record_code(rec) == rt.METADATA_NAMED_NODE:
                        vals = rec[2] if rec[0] == "UNABBREV" else rec[4]
                        if vals:
                            first_node_id = vals[0]
                            break
                assert first_node_id is not None, "Expected an existing named metadata node"
                if abbreviate_name:
                    # Insert right before the first METADATA_NAME using the block's own abbrev if any,
                    # or define a local Char6-array abbrev at the end of the metadata block.
                    existing_abbrevs = sum(1 for rec in md_items if rec[0] == "DEFINE_ABBREV")
                    # Count BLOCKINFO abbrevs for METADATA_BLOCK_ID via a quick scan
                    bi_count = 0
                    for m in mod_items:
                        if m[0] == "RAW_BLOCK" and m[1] == rt.BLOCKINFO_BLOCK_ID:
                            bi = {}
                            rt.parse_bitcode_block(
                                rt.BitReader(m[3]), rt.BLOCKINFO_BLOCK_ID, m[2], len(m[3]) * 8, bi, set()
                            )
                            bi_count = len(bi.get(rt.METADATA_BLOCK_ID, ()))
                    # Only emit a new DEFINE_ABBREV if its code fits within the block's abbrev width.
                    next_code = 4 + bi_count + existing_abbrevs
                    if next_code < (1 << mod_item[2]):
                        ops = [("LIT", rt.METADATA_NAME), (rt.ENC_ARRAY, None), (rt.ENC_CHAR6, None)]
                        md_items.append(("DEFINE_ABBREV", ops))
                        md_items.append(("ABBREV", next_code, ops, rt.METADATA_NAME,
                                         list(b"xbox.compiler.options"), None))
                    else:
                        md_items.append(("UNABBREV", rt.METADATA_NAME, list(b"xbox.compiler.options")))
                else:
                    md_items.append(("UNABBREV", rt.METADATA_NAME, list(b"xbox.compiler.options")))
                md_items.append(("UNABBREV", rt.METADATA_NAMED_NODE, [first_node_id]))
                injected = True
    assert injected, "Failed to inject xbox.compiler.options into bitcode"
    return rt.serialize_bitcode(top)


def _inject_xb2_into_container(blob: bytes, *, abbreviate_name: bool = False) -> bytes:
    """Turn a source-built desktop DXBC container into a synthetic xb2 container."""
    secs = parts(blob)
    ver_type, dxil_ver, _, orig_bc = rt.unpack_dxil_program(secs[b"DXIL"])
    xb2_bc = _inject_xb2_into_bitcode(orig_bc, abbreviate_name=abbreviate_name)
    # Reproduce the observed Xbox DXIL framing: BitcodeOffset=112 (96 extra header bytes) + trailing xbop bytes.
    pre_extra = bytes(96)
    post_extra = b"xbop" + struct.pack("<I", 0)
    padded_bc = xb2_bc + bytes((-len(xb2_bc)) & 3)
    body = pre_extra + padded_bc + post_extra
    xb2_dxil = struct.pack("<II4sIII", ver_type, (24 + len(body)) // 4, b"DXIL", dxil_ver, 112, len(xb2_bc)) + body

    rebuilt = []
    for tag, payload in secs.items():
        if tag == b"DXIL":
            rebuilt.append((tag, xb2_dxil))
        elif tag == b"STAT" and len(payload) >= 24 and payload[8:12] == b"DXIL":
            st_vt, st_dv, _, st_bc = rt.unpack_dxil_program(payload)
            rebuilt.append((tag, rt.pack_dxil_program(st_vt, st_dv, _inject_xb2_into_bitcode(st_bc))))
        elif tag == b"RTS0" and len(payload) >= 24:
            flags, = struct.unpack_from("<I", payload, 20)
            rebuilt.append((tag, payload[:20] + struct.pack("<I", flags | rt.XBOX_ROOT_SIGNATURE_FLAGS) + payload[24:]))
        else:
            rebuilt.append((tag, payload))
    return rt.build_dxbc_container(rebuilt, digest=bytes(range(1, 17)))


def check_pure_python_controls() -> None:
    # 0. VBR operands are limited to the unsigned 64-bit range.
    for value in (0, 1, (1 << 64) - 1):
        writer = rt.BitWriter()
        writer.write_vbr(value, 32)
        encoded = writer.finish()
        reader = rt.BitReader(encoded)
        assert reader.read_vbr(32) == value
        assert reader.bitpos == len(encoded) * 8

    for value in (1 << 64, (1 << 65) - 1):
        writer = rt.BitWriter()
        writer.write_vbr(value, 32)
        try:
            rt.BitReader(writer.finish()).read_vbr(32)
        except ValueError as exc:
            assert str(exc) == "VBR integer exceeds 64 bits"
        else:
            raise AssertionError(f"read_vbr accepted an out-of-range value {value:#x}")

    # 1. Identity round-trip + retarget across unabbreviated/abbreviated/blob combinations
    for abbrev_triple in (False, True):
        for abbrev_md in (False, True):
            bc = _encode_synthetic_bitcode(
                triple="dxil-ms-xb2",
                abbreviated_triple=abbrev_triple,
                abbreviated_md_name=abbrev_md,
                include_xbox_md=True,
                include_blob=True,
            )
            assert rt.serialize_bitcode(rt.parse_bitcode(bc)) == bc
            rewritten, info = rt.retarget_bitcode(bc)
            assert info["original_triple"] == "dxil-ms-xb2"
            assert info["target_triple"] == "dxil-ms-dx"
            assert info["triple_rewritten"] is True
            assert info["removed_named_metadata"] == ["xbox.compiler.options"]

            # Parse rewritten stream and verify triple + remaining named metadata + preserved opaque block
            parsed = rt.parse_bitcode(rewritten)
            mod = [it for it in parsed if it[0] == "BLOCK" and it[1] == rt.MODULE_BLOCK_ID][0][3]
            triples = [rt._record_bytes(it).decode("ascii") for it in mod if rt._record_code(it) == rt.MODULE_CODE_TRIPLE]
            assert triples == ["dxil-ms-dx"]
            md_blocks = [it[3] for it in mod if it[0] == "BLOCK" and it[1] == rt.METADATA_BLOCK_ID]
            md_names = [rt._record_bytes(rec).decode("utf-8") for blk in md_blocks for rec in blk
                        if rt._record_code(rec) == rt.METADATA_NAME]
            assert md_names == ["llvm.ident"], f"Unexpected remaining metadata names: {md_names}"
            opaque = [it[3] for it in mod if it[0] == "RAW_BLOCK" and it[1] == 12]
            assert opaque == [struct.pack("<2I", 0x11223344, 0x55667788)]

            # Idempotency check on already-desktop bitcode
            again, info2 = rt.retarget_bitcode(rewritten)
            assert again == rewritten
            assert info2["triple_rewritten"] is False
            assert info2["removed_named_metadata"] == []

    # 2. Synthetic container with non-canonical offset, STAT, RTS0 flags (including preserved low flag), and stale digest
    bc = _encode_synthetic_bitcode(
        triple="dxil-ms-xb2", abbreviated_triple=False, abbreviated_md_name=True, include_xbox_md=True
    )
    body = bytes(96) + bc + bytes((-len(bc)) & 3) + b"xbop\x00\x00\x00\x00"
    dxil_part = struct.pack("<II4sIII", (5 << 16) | 0x65, (24 + len(body)) // 4, b"DXIL", 0x105, 112, len(bc)) + body
    stat_part = rt.pack_dxil_program((5 << 16) | 0x65, 0x105, bc)
    rts0_part = struct.pack("<6I", 2, 0, 24, 0, 0, 0x03000001)
    synth_dxbc = rt.build_dxbc_container(
        [(b"RTS0", rts0_part), (b"STAT", stat_part), (b"DXIL", dxil_part)],
        digest=b"\xaa" * 16,
    )
    assert dxil_program(synth_dxbc, 0)

    out_dxbc, cinfo = rt.retarget_container(synth_dxbc)
    assert dxil_program(out_dxbc, 0)
    assert out_dxbc[4:20] == bytes(16)
    assert cinfo["original_bitcode_offset"] == 112
    assert cinfo["stat_retargeted"] is True
    assert cinfo["original_root_flags"] == "0x3000001"
    assert cinfo["cleared_root_flags"] == "0x3000000"
    out_secs = parts(out_dxbc)
    assert struct.unpack_from("<I", out_secs[b"RTS0"], 20)[0] == 1
    _, _, new_off, new_bc = rt.unpack_dxil_program(out_secs[b"DXIL"])
    assert new_off == 16
    _, _, _, new_st_bc = rt.unpack_dxil_program(out_secs[b"STAT"])
    assert rt.retarget_bitcode(new_st_bc)[1]["original_triple"] == "dxil-ms-dx"

    kept_dxbc, kept_info = rt.retarget_container(synth_dxbc, clear_root_flags=False)
    assert kept_info["cleared_root_flags"] == "0x0"
    assert struct.unpack_from("<I", parts(kept_dxbc)[b"RTS0"], 20)[0] == 0x03000001

    # 3. Negative controls for malformed inputs
    for bad_bc in (b"NOPE", rt.BITCODE_MAGIC + b"\x01\x00\x00\x00"):
        try:
            rt.retarget_bitcode(bad_bc)
        except ValueError:
            pass
        else:
            raise AssertionError(f"Accepted malformed bitcode {bad_bc!r}")

    # Abbrev-defined operand widths must respect LLVM bitstream bounds (VBR <= 32, fixed <= 64)
    for ops in ([(rt.ENC_VBR, 33)], [(rt.ENC_FIXED, 65)]):
        wide_bc = rt.serialize_bitcode([("BLOCK", rt.MODULE_BLOCK_ID, 3, [
            ("DEFINE_ABBREV", ops),
            ("ABBREV", 4, ops, 9, [42], None),
        ])])
        try:
            rt.parse_bitcode(wide_bc)
        except ValueError as exc:
            assert "width" in str(exc).lower()
        else:
            raise AssertionError(f"Accepted out-of-bounds abbrev operand width in {ops}")

    # Missing METADATA_NAMED_NODE after METADATA_NAME("xbox.compiler.options")
    orphan_md = rt.serialize_bitcode([(
        "BLOCK", rt.MODULE_BLOCK_ID, 3, [(
            "BLOCK", rt.METADATA_BLOCK_ID, 3, [("UNABBREV", rt.METADATA_NAME, list(b"xbox.compiler.options"))]
        )]
    )])
    try:
        rt.retarget_bitcode(orphan_md)
    except ValueError as exc:
        assert "METADATA_NAMED_NODE" in str(exc)
    else:
        raise AssertionError("Accepted METADATA_NAME without METADATA_NAMED_NODE")

    # Container without DXIL part
    no_dxil = rt.build_dxbc_container([(b"RTS0", rts0_part)])
    try:
        rt.retarget_container(no_dxil)
    except ValueError as exc:
        assert "no DXIL part" in str(exc)
    else:
        raise AssertionError("Accepted DXBC container without DXIL part")

    # 4. CLI smoke test (container + --extract-module + flag validation)
    tool = Path(__file__).resolve().parents[1] / "tools" / "dxil_retarget.py"
    with TemporaryDirectory() as tmp:
        tmp_dir = Path(tmp)
        in_file = tmp_dir / "in.dxbc"
        out_file = tmp_dir / "out.dxbc"
        bc_file = tmp_dir / "out.bc"
        fake_probe = tmp_dir / "fake_probe"
        fake_probe.write_bytes(b"")
        in_file.write_bytes(synth_dxbc)
        cli_out = json.loads(subprocess.check_output([sys.executable, "-B", str(tool), str(in_file), str(out_file)]))
        assert cli_out["kind"] == "dxbc" and cli_out["signed"] is False and out_file.read_bytes() == out_dxbc
        cli_bc = json.loads(subprocess.check_output(
            [sys.executable, "-B", str(tool), str(in_file), str(bc_file), "--extract-module"]
        ))
        assert cli_bc["kind"] == "bitcode" and bc_file.read_bytes() == new_bc

        for bad_args in (
            [str(in_file), str(out_file), "--sign"],
            [str(in_file), str(bc_file), "--extract-module", "--sign", "--probe", str(fake_probe)],
            [str(in_file), str(bc_file), "--extract-module", "--pso-probe", str(fake_probe)],
            [str(in_file), str(out_file), "--warp-only"],
            [str(bc_file), str(out_file), "--sign", "--probe", str(fake_probe)],
        ):
            bad_run = subprocess.run([sys.executable, "-B", str(tool), *bad_args], capture_output=True)
            assert bad_run.returncode != 0, f"Accepted invalid CLI arguments {bad_args}"

        # A damaged or missing input is reported as an error message, not a traceback.
        damaged = {"garbage": b"not a shader container", "truncated-dxbc": b"DXBC0123456789abcdef",
                   "truncated-bitcode": b"BC\xc0\xde\x00\x00"}
        for label, payload in damaged.items():
            bad_in = tmp_dir / f"damaged-{label}.bin"
            bad_in.write_bytes(payload)
            bad_out = tmp_dir / f"damaged-{label}.out"
            bad_run = subprocess.run([sys.executable, "-B", str(tool), str(bad_in), str(bad_out)],
                                     capture_output=True, text=True)
            assert bad_run.returncode == 1, (label, bad_run.returncode, bad_run.stderr)
            assert "Traceback" not in bad_run.stderr and not bad_out.exists(), (label, bad_run.stderr)
        missing_run = subprocess.run([sys.executable, "-B", str(tool), str(tmp_dir / "missing.bin"),
                                      str(tmp_dir / "missing.out")], capture_output=True, text=True)
        assert missing_run.returncode == 1 and "Traceback" not in missing_run.stderr, missing_run.stderr

    # 5. Pure-Python controls for sign_container and check_compute_pso
    try:
        rt.sign_container(Path("unused"), b"NOPE")
    except ValueError:
        pass
    else:
        raise AssertionError("sign_container accepted non-DXBC input")

    try:
        rt.check_compute_pso(Path("unused"), b"NOPE")
    except ValueError:
        pass
    else:
        raise AssertionError("check_compute_pso accepted non-DXBC input")

    signed_synth = out_dxbc[:4] + bytes(range(1, 17)) + out_dxbc[20:]
    with patch.object(rt.subprocess, "run") as mock_run:
        mock_run.return_value = subprocess.CompletedProcess(["probe", "--sign"], 0, signed_synth, b"")
        assert rt.sign_container(Path("probe"), out_dxbc) == signed_synth
        assert mock_run.call_args[0][0] == ["probe", "--sign"]

        for bad_proc, expected in (
            (subprocess.CompletedProcess(["probe", "--sign"], 5, b"", b"validation error"), "validation error"),
            (subprocess.CompletedProcess(["probe", "--sign"], 0, b"NOPE", b""), "invalid DXBC"),
            (subprocess.CompletedProcess(["probe", "--sign"], 0, out_dxbc, b""), "digest zeroed"),
        ):
            mock_run.return_value = bad_proc
            try:
                rt.sign_container(Path("probe"), out_dxbc)
            except RuntimeError as exc:
                assert expected in str(exc), str(exc)
            else:
                raise AssertionError(f"sign_container accepted {bad_proc}")

        mock_run.return_value = subprocess.CompletedProcess(
            ["dev", "--compute-pso", "--warp-only"], 0, b'{"warp_pso":"0x00000000"}\n', b""
        )
        cleared_inventory = dict(calls={}, declarations=[], metadata_only=[])
        warp_pso = rt.check_compute_pso(Path("dev"), signed_synth, warp_only=True,
                                        intrinsics=cleared_inventory)
        assert warp_pso["warp_pso"] == "0x00000000", warp_pso
        assert warp_pso["gate"]["verdict"] == "ALLOW", warp_pso
        assert mock_run.call_args[0][0] == ["dev", "--compute-pso", "--warp-only"]
        rt.check_compute_pso(Path("dev"), signed_synth, warp_only=False, intrinsics=cleared_inventory)
        assert mock_run.call_args[0][0] == ["dev", "--compute-pso"]

        # Hardware backend probe helper: exit 5 ("no hardware adapter") maps to None (skip 77),
        # while other exit codes or warp exit 5 still propagate as failures.
        mock_run.return_value = subprocess.CompletedProcess(
            ["dev", "--compute-pso"], 0, b'{"hardware_pso":"0x00000000"}\n', b""
        )
        cleared = dict(calls={}, declarations=[], metadata_only=[])
        hardware_result = _probe_compute_pso(Path("dev"), signed_synth, backend="hardware",
                                             intrinsics=cleared)
        assert hardware_result["hardware_pso"] == "0x00000000", hardware_result
        assert hardware_result["gate"]["verdict"] == "ALLOW", hardware_result
        assert hardware_result["gate"]["submitted"] is True, hardware_result
        assert mock_run.call_args[0][0] == ["dev", "--compute-pso"]

        mock_run.side_effect = subprocess.CalledProcessError(5, ["dev", "--compute-pso"])
        assert _probe_compute_pso(Path("dev"), signed_synth, backend="hardware",
                                  intrinsics=cleared) is None
        try:
            _probe_compute_pso(Path("dev"), signed_synth, backend="warp", intrinsics=cleared)
        except subprocess.CalledProcessError:
            pass
        else:
            raise AssertionError("_probe_compute_pso swallowed exit 5 on warp backend")
        mock_run.side_effect = subprocess.CalledProcessError(6, ["dev", "--compute-pso"])
        try:
            _probe_compute_pso(Path("dev"), signed_synth, backend="hardware", intrinsics=cleared)
        except subprocess.CalledProcessError:
            pass
        else:
            raise AssertionError("_probe_compute_pso swallowed non-adapter error exit 6")
        mock_run.side_effect = None

    # 6. Read-only xbox.* intrinsic inventory: declarations, actual calls and metadata-only names.
    from probe_roots import xbox_intrinsic_inventory

    # (a) Declared and called: the call instruction reports it, and a metadata mention does not
    # turn it into a metadata-only name.
    called = """
declare i32 @xbox.V_ALIGNBYTE_B32(i32, i32, i32, i32)

define void @main() {
  %v = call i32 @xbox.V_ALIGNBYTE_B32(i32 332, i32 %a, i32 %b, i32 %sel)
  ret void
}

!0 = !{!"xbox.V_ALIGNBYTE_B32"}
"""
    inventory = xbox_intrinsic_inventory(called)
    assert inventory["calls"] == {"xbox.V_ALIGNBYTE_B32": 1}, inventory
    assert inventory["declarations"] == ["xbox.V_ALIGNBYTE_B32"], inventory
    assert inventory["metadata_only"] == [], inventory

    # (b) Declared only: an unused declaration is not a call.
    declared = """
declare void @xbox.global_store_dwordx3_with_cache(i32, i64, i32, i32)

define void @main() {
  ret void
}
"""
    inventory = xbox_intrinsic_inventory(declared)
    assert inventory["calls"] == {}, inventory
    assert inventory["declarations"] == ["xbox.global_store_dwordx3_with_cache"], inventory
    assert inventory["metadata_only"] == [], inventory

    # (c) Metadata only: the name lives in a metadata string value, never declared or called.
    # The named-metadata key !xbox.module names the node and is not an intrinsic name.
    metadata = """
define void @main() {
  ret void
}

!xbox.module = !{!0}
!0 = !{!"xbox.V_ALIGNBYTE_B32", i32 332}
"""
    inventory = xbox_intrinsic_inventory(metadata)
    assert inventory["calls"] == {}, inventory
    assert inventory["declarations"] == [], inventory
    assert inventory["metadata_only"] == ["xbox.V_ALIGNBYTE_B32"], inventory

    # A metadata description that looks like a call still contains no instruction.
    inventory = xbox_intrinsic_inventory('!0 = !{!"call void @xbox.inventory_example("}')
    assert inventory == dict(calls={}, declarations=[], metadata_only=["xbox.inventory_example"],
                            references={}), inventory
    inventory = xbox_intrinsic_inventory('  %"call result" = tail call i32 @"xbox.inventory_example"()')
    assert inventory["calls"] == {"xbox.inventory_example": 1}, inventory

    # (e) Named-metadata key alone: the Xbox compiler option node carries no intrinsic name.
    named_metadata = """
define void @main() {
  ret void
}

!xbox.compiler.options = !{!1}
!1 = !{!"__XBOX_ENABLE_WAVE32=1"}
"""
    inventory = xbox_intrinsic_inventory(named_metadata)
    assert inventory == dict(calls={}, declarations=[], metadata_only=[], references={}), inventory

    # (d) Ordinary module: dx.op calls and unrelated metadata report three empty groups.
    ordinary = """
target triple = "dxil-ms-dx"

define void @main() {
  %v = call i32 @dx.op.unary.f32(i32 12, float 0.000000e+00)
  %w = call i32 @"dx.op.threadId.i32"(i32 93, i32 0)
  ret void
}

!llvm.module.flags = !{!0}
!0 = !{i32 2, !"DXC version", !"1.8.2407"}
"""
    inventory = xbox_intrinsic_inventory(ordinary)
    assert inventory == dict(calls={}, declarations=[], metadata_only=[], references={}), inventory

    # 7. Submission gate: the final module is checked and a refused module is never submitted.
    import xbox_intrinsic_gate as gate

    class RecordingSubmit:
        """Submission stub: a refused module must reach it zero times."""

        def __init__(self):
            self.batches = []

        def __call__(self, module_text):
            self.batches.append(module_text)

    stub = RecordingSubmit()
    decision = gate.gate_submission(gate.check_text(called), called, stub)
    assert decision["verdict"] == gate.REJECT, decision
    assert decision["submitted"] is False, decision
    assert stub.batches == [], "a refused module reached the submission callable"
    assert decision["reported_names"] == ["xbox.V_ALIGNBYTE_B32"], decision

    for label, module_text in (("declared only", declared), ("metadata only", metadata),
                               ("ordinary module", ordinary)):
        stub = RecordingSubmit()
        decision = gate.gate_submission(gate.check_text(module_text), module_text, stub)
        assert decision["verdict"] == gate.ALLOW, (label, decision)
        assert len(stub.batches) == 1, (label, stub.batches)

    # An adapter status is advisory: a call the module still executes blocks whatever it claims.
    stub = RecordingSubmit()
    claims_lowered = gate.status_table(["V_ALIGNBYTE_B32"])
    decision = gate.gate_submission(gate.check_text(called, claims_lowered), called, stub)
    assert decision["verdict"] == gate.REJECT, decision
    assert decision["reported_names"] == ["xbox.V_ALIGNBYTE_B32"], decision
    assert decision["blocking"][0]["advisory"] == gate.LOWERED, decision["blocking"][0]
    assert "advisory reports LOWERED" in decision["message"], decision["message"]
    assert stub.batches == [], "an advisory status cleared a call the module still executes"
    # The same module clears once the call is gone from the instructions.
    stub = RecordingSubmit()
    removed = gate.gate_submission(gate.check_text(declared), declared, stub)
    assert removed["verdict"] == gate.ALLOW and len(stub.batches) == 1, removed

    # Every form that keeps an xbox.* symbol reachable blocks, and the refusal names the form.
    alias_module = "@alias_bfi = alias i32 (i32), ptr @xbox.BFI\n"
    store_module = ("define void @main() {\n"
                    "  store ptr @xbox.global_store_dword_with_cache, ptr %slot\n"
                    "  ret void\n}\n")
    indirect_module = ("define void @main() {\n"
                       "  %fp = bitcast ptr @xbox.foo to ptr\n"
                       "  call void %fp(i32 1)\n"
                       "  ret void\n}\n")
    bitcast_call_module = ("define void @main() {\n"
                           "  call void bitcast (void (i32)* @xbox.foo to void (i64)*)(i64 5)\n"
                           "  ret void\n}\n")
    global_initializer_module = "@table = global ptr @xbox.Med3_F32\n"
    # A function pointer: the symbol goes into a global, the call goes through the loaded pointer.
    function_pointer_module = ("@slot = global ptr null\n"
                               "define void @main() {\n"
                               "  store ptr @xbox.foo, ptr @slot\n"
                               "  %fp = load ptr, ptr @slot\n"
                               "  call void %fp(i32 1)\n"
                               "  ret void\n}\n")
    # An ordinary callee that receives the symbol as an argument — not a debug intrinsic.
    call_operand_module = ("declare void @consume(ptr)\n"
                           "define void @main() {\n"
                           "  call void @consume(ptr @xbox.foo)\n"
                           "  ret void\n}\n")
    for label, module_text, form in (
        ("alias", alias_module, "alias"),
        ("store", store_module, "store"),
        ("indirect call through a bitcast", indirect_module, "bitcast"),
        ("function pointer in a global initializer", global_initializer_module, "global-initializer"),
        ("function pointer in a slot", function_pointer_module, "store"),
        ("call operand of an ordinary callee", call_operand_module, "call-operand"),
        ("bitcast-spelled direct call", bitcast_call_module, "call"),
    ):
        stub = RecordingSubmit()
        decision = gate.gate_submission(gate.check_text(module_text), module_text, stub)
        assert decision["verdict"] == gate.REJECT, (label, decision)
        assert stub.batches == [], (label, "a reachable symbol was submitted")
        assert decision["blocking"][0]["form"] == form, (label, decision["blocking"])
        assert form in decision["message"], (label, decision["message"])
    # The bitcast-spelled call is a call site, not merely a reference.
    assert gate.check_text(bitcast_call_module)["calls"] == {"xbox.foo": 1}, gate.check_text(bitcast_call_module)
    # A referenced name is not metadata-only.
    assert gate.check_text(alias_module)["metadata_only"] == [], gate.check_text(alias_module)

    # A debug intrinsic carries the symbol only as metadata the runtime never reads: allowed.
    debug_only_module = ("declare void @llvm.dbg.value(metadata, i64, metadata, metadata)\n"
                         "define void @main() {\n"
                         "  call void @llvm.dbg.value(metadata ptr @xbox.foo, i64 0,"
                         " metadata !0, metadata !1)\n"
                         "  ret void\n}\n"
                         "!0 = !{}\n"
                         "!1 = !{}\n")
    stub = RecordingSubmit()
    decision = gate.gate_submission(gate.check_text(debug_only_module), debug_only_module, stub)
    assert decision["verdict"] == gate.ALLOW, decision
    assert decision["calls"] == {} and decision["blocking"] == [], decision
    assert decision["debug_only"] == ["xbox.foo"], decision
    assert "debug-only" in decision["message"], decision["message"]
    assert len(stub.batches) == 1, decision
    # The same symbol stops being debug-only once an ordinary callee receives it.
    assert gate.check_text(call_operand_module)["verdict"] == gate.REJECT

    # Only the callee decides: an ordinary callee whose argument happens to mention a debug
    # intrinsic is still an executable call, so the symbol must block.
    trap_module = ("declare void @consume(ptr, ptr)\n"
                   "define void @main() {\n"
                   "  call void @consume(ptr @xbox.foo, ptr bitcast (void()* @llvm.dbg.value to i8*))\n"
                   "  ret void\n}\n")
    stub = RecordingSubmit()
    decision = gate.gate_submission(gate.check_text(trap_module), trap_module, stub)
    assert decision["verdict"] == gate.REJECT, decision
    assert stub.batches == [], "a call mentioning a debug intrinsic was submitted"
    assert decision["blocking_references"][0]["form"] == "call-operand", decision
    assert decision["debug_only"] == [], decision

    # The other debug intrinsics are debug-only uses as well.
    for intrinsic, signature in (("llvm.dbg.declare", "metadata, metadata, metadata"),
                                 ("llvm.dbg.assign", "metadata, metadata, metadata, metadata, metadata, metadata")):
        module_text = (f"declare void @{intrinsic}({signature})\n"
                       "define void @main() {\n"
                       f"  call void @{intrinsic}(metadata ptr @xbox.foo, metadata !0, metadata !1"
                       f"{', metadata !0, metadata !1, metadata !0' if 'assign' in intrinsic else ''})\n"
                       "  ret void\n}\n"
                       "!0 = !{}\n"
                       "!1 = !{}\n")
        stub = RecordingSubmit()
        decision = gate.gate_submission(gate.check_text(module_text), module_text, stub)
        assert decision["verdict"] == gate.ALLOW, (intrinsic, decision)
        assert decision["debug_only"] == ["xbox.foo"], (intrinsic, decision)
        assert len(stub.batches) == 1, (intrinsic, decision)

    # sites counts executable uses only; a debug-only use is counted apart.
    debug_inventory = xbox_intrinsic_inventory(debug_only_module)
    assert debug_inventory["references"]["xbox.foo"]["sites"] == 0, debug_inventory
    assert debug_inventory["references"]["xbox.foo"]["debug_sites"] == 1, debug_inventory
    # The archive filter counts anything reachable, debug-only included: skipping is conservative.
    assert gate.reaches_xbox(debug_inventory) is True, debug_inventory

    # An advisory status cannot clear a reference the module still holds.
    stub = RecordingSubmit()
    claims = gate.status_table(["BFI"])
    decision = gate.gate_submission(gate.check_text(alias_module, claims), alias_module, stub)
    assert decision["verdict"] == gate.REJECT, decision
    assert decision["blocking_references"][0]["advisory"] == gate.LOWERED, decision
    assert stub.batches == [], "an advisory status cleared a reference"

    # The same rule at the backend boundary: a reference-only inventory starts no probe process.
    with patch.object(rt.subprocess, "run") as mock_run:
        mock_run.return_value = subprocess.CompletedProcess(
            ["dev", "--compute-pso"], 0, b'{"warp_pso":"0x00000000"}\n', b"")
        reference_only = rt.check_compute_pso(
            Path("dev"), synth_dxbc,
            intrinsics=dict(calls={}, declarations=[], metadata_only=[],
                            references={"xbox.BFI": {"sites": 1, "kinds": ["alias"]}}))
        assert mock_run.call_count == 0, "a reference-only module started a submission"
        assert reference_only["gate"]["verdict"] == gate.REJECT, reference_only
        assert reference_only["gate"]["blocking_references"][0]["form"] == "alias", reference_only

    # --only-without-xbox-calls keeps one meaning: no form reaches an xbox.* symbol.
    reached = xbox_intrinsic_inventory(alias_module)
    assert gate.reaches_xbox(reached) is True, reached
    assert gate.reaches_xbox(xbox_intrinsic_inventory(called)) is True
    assert gate.reaches_xbox(xbox_intrinsic_inventory(declared)) is False
    assert gate.reaches_xbox(xbox_intrinsic_inventory(metadata)) is False
    assert gate.reaches_xbox(xbox_intrinsic_inventory(ordinary)) is False

    # A metadata string that spells a call is not an instruction: it must not refuse the module.
    spelled_call = '!0 = !{!"call void @xbox.text()"}'
    stub = RecordingSubmit()
    decision = gate.gate_submission(gate.check_text(spelled_call), spelled_call, stub)
    assert decision["verdict"] == gate.ALLOW, decision
    assert decision["metadata_only"] == ["xbox.text"], decision
    assert len(stub.batches) == 1, decision

    # A ';' inside a metadata string does not start a comment.
    decision = gate.check_text('!0 = !{!"xbox.a;xbox.b"}')
    assert decision["metadata_only"] == ["xbox.a", "xbox.b"], decision
    assert decision["verdict"] == gate.ALLOW, decision

    # The submission boundary: a refused module never starts the probe process.
    with patch.object(rt.subprocess, "run") as mock_run:
        mock_run.return_value = subprocess.CompletedProcess(
            ["dev", "--compute-pso", "--warp-only"], 0, b'{"warp_pso":"0x00000000"}\n', b"")
        refused = rt.check_compute_pso(
            Path("dev"), synth_dxbc, warp_only=True,
            intrinsics=dict(calls={"xbox.BFI": 1}, declarations=[], metadata_only=[]))
        assert mock_run.call_count == 0, "a refused module started a submission"
        assert refused["gate"]["verdict"] == gate.REJECT, refused
        assert refused["gate"]["submitted"] is False, refused
        assert refused["gate"]["reported_names"] == ["xbox.BFI"], refused
        allowed = rt.check_compute_pso(
            Path("dev"), synth_dxbc, warp_only=True,
            intrinsics=dict(calls={}, declarations=[], metadata_only=[]))
        assert mock_run.call_count == 1, "an allowed module did not submit exactly once"
        assert allowed["warp_pso"] == "0x00000000", allowed
        assert allowed["gate"]["submitted"] is True, allowed

    # The helper itself is fail closed: no inventory means no probe process at all.
    with patch.object(rt.subprocess, "run") as mock_run:
        mock_run.return_value = subprocess.CompletedProcess(
            ["dev", "--compute-pso"], 0, b'{"warp_pso":"0x00000000"}\n', b"")
        no_inventory = rt.check_compute_pso(Path("dev"), synth_dxbc)
        assert mock_run.call_count == 0, "a module with no inventory started a submission"
        assert no_inventory["gate"]["verdict"] == gate.NOT_CHECKED, no_inventory
        assert no_inventory["gate"]["reason"] == "module-not-disassembled", no_inventory
        assert no_inventory["gate"]["submitted"] is False, no_inventory
        advisory_only = rt.check_compute_pso(
            Path("dev"), synth_dxbc,
            intrinsics=dict(calls={"xbox.BFI": 1}, declarations=[], metadata_only=[]),
            status_of=gate.status_table(["BFI"]))
        assert mock_run.call_count == 0, "an advisory status started a submission"
        assert advisory_only["gate"]["verdict"] == gate.REJECT, advisory_only
        assert advisory_only["gate"]["blocking"][0]["advisory"] == gate.LOWERED, advisory_only

    # A module nobody disassembled must not be submitted, and the decision has to stay visible.
    unchecked = gate.not_checked("module-not-disassembled")
    assert unchecked["verdict"] == gate.NOT_CHECKED, unchecked
    assert unchecked["verdict"] != gate.ALLOW and unchecked["checked"] is False, unchecked
    assert unchecked["reported_names"] == [] and unchecked["calls"] == {}, unchecked
    stub = RecordingSubmit()
    gate.gate_submission(unchecked, called, stub)
    assert stub.batches == [], "an unexamined module reached the submission callable"

    # --pso-probe without --probe: no disassembly, so the CLI records NOT_CHECKED and submits nothing.
    tool = Path(__file__).resolve().parents[1] / "tools" / "dxil_retarget.py"
    with TemporaryDirectory() as tmp:
        tmp_dir = Path(tmp)
        in_file = tmp_dir / "in.dxbc"
        out_file = tmp_dir / "out.dxbc"
        absent_probe = tmp_dir / "absent_probe.exe"
        in_file.write_bytes(synth_dxbc)
        ungated = subprocess.run(
            [sys.executable, "-B", str(tool), str(in_file), str(out_file), "--pso-probe", str(absent_probe)],
            capture_output=True, text=True)
        assert ungated.returncode == 0, ungated.stderr
        pso = json.loads(ungated.stdout)["pso"]
        assert pso["gate"]["verdict"] == gate.NOT_CHECKED, pso
        assert pso["gate"]["reason"] == "module-not-disassembled", pso
        assert "warp_pso" not in pso and "hardware_pso" not in pso, pso
        assert "--probe" in ungated.stderr, ungated.stderr

    print("submission gate controls: executable call refused with 0 submissions; declared only, "
          "metadata only and ordinary modules submit once; an advisory LOWERED status still refuses "
          "while the call remains and clears once the call is gone; missing inventory starts 0 probe "
          "processes; an alias, a store, an indirect call through a bitcast, a function pointer in a "
          "global initializer, a function pointer in a slot, a call operand of an ordinary callee and a "
          "bitcast-spelled direct call each refused with their form named in 0 submissions, with calls "
          "and references listed apart; a debug intrinsic that only carries the symbol as metadata is "
          "allowed and reported as debug-only, and so are llvm.dbg.declare and llvm.dbg.assign; an "
          "ordinary call that merely mentions a debug intrinsic in an argument still blocks as "
          "call-operand; an advisory LOWERED status clears neither a call nor a reference; sites counts "
          "executable uses only while the archive filter treats every reachable form as reaching an "
          "intrinsic; a metadata string that spells a call and a ';' inside a metadata string still "
          "pass; without --probe the CLI records NOT_CHECKED and submits nothing")
    print("pure-python dxil_retarget controls: ok")


def _probe_compute_pso(device_probe: Path, blob: bytes, *, backend: str,
                       intrinsics: dict | None = None) -> dict | None:
    """Run root_device_probe --compute-pso; return None on hardware backend exit 5 (no GPU)."""
    try:
        return rt.check_compute_pso(device_probe, blob, warp_only=(backend == "warp"),
                                    intrinsics=intrinsics)
    except subprocess.CalledProcessError as exc:
        if backend == "hardware" and exc.returncode == 5:
            return None
        raise


def check_validator_fixtures(
    probe: Path,
    shaders: list[Path],
    validator_dll: Path,
    device_probe: Path | None = None,
    offscreen: Path | None = None,
    backend: str = "warp",
) -> int:
    from probe_roots import validate_shader
    from shader_probe_check import file_version, module_outcome

    if backend not in ("warp", "hardware"):
        raise ValueError(f"Unsupported backend {backend!r}")
    root_key = f"{backend}_root"
    pso_key = f"{backend}_pso"
    sm_key = f"{backend}_shader_model"

    tool = Path(__file__).resolve().parents[1] / "tools" / "dxil_retarget.py"
    module_records = []
    for idx, shader_path in enumerate(shaders):
        desktop_blob = shader_path.read_bytes()
        xb2_blob = _inject_xb2_into_container(desktop_blob, abbreviate_name=(idx % 2 == 1))
        xb2_secs = parts(xb2_blob)

        # Negative control 1: unmodified synthetic xb2 must be rejected with both xb2 errors.
        before = validate_shader(probe, xb2_blob, xb2_secs[b"DXIL"], optional_modes=("module",))
        assert before["container"]["status"] == "0x80aa0009", before["container"]
        diag_text = "\n".join(before["container"]["diagnostics"])
        assert "Unknown target triple 'dxil-ms-xb2'" in diag_text, diag_text
        assert "Named metadata 'xbox.compiler.options' is unknown" in diag_text, diag_text
        assert before["ir"]["target_triple"] == "dxil-ms-xb2"

        # Negative control 2: wrong triple 'dxil-ms' must still be rejected by DXC.
        bad_triple_blob, _ = rt.retarget_container(xb2_blob, target_triple="dxil-ms")
        bad_triple = validate_shader(probe, bad_triple_blob, parts(bad_triple_blob)[b"DXIL"], optional_modes=("module",))
        assert bad_triple["container"]["status"] == "0x80aa0009", bad_triple["container"]
        assert "Unknown target triple 'dxil-ms'" in "\n".join(bad_triple["container"]["diagnostics"])

        # Negative control 3: keeping RTS0 Xbox flags fails container validation with 0x80aa0013.
        kept_blob, _ = rt.retarget_container(xb2_blob, clear_root_flags=False)
        kept = validate_shader(probe, kept_blob, parts(kept_blob)[b"DXIL"], optional_modes=("module",))
        assert kept["container"]["status"] == "0x80aa0013", kept["container"]
        assert "Unsupported bit-flag set (root signature flags 0x3000000)" in "\n".join(kept["container"]["diagnostics"])

        # Signing must also reject every invalid container without emitting bytes on stdout.
        for label, invalid_blob in (("xb2", xb2_blob), ("dxil-ms", bad_triple_blob), ("xbox-flags", kept_blob)):
            bad_sign = subprocess.run([str(probe), "--sign"], input=invalid_blob, capture_output=True, timeout=10)
            assert bad_sign.returncode != 0 and not bad_sign.stdout and bad_sign.stderr, (label, bad_sign)

        # Positive check: full retarget passes container, module-only, disassembly, STAT, and D3D12 root signature.
        retargeted_blob, info = rt.retarget_container(xb2_blob)
        ret_secs = parts(retargeted_blob)
        assert info["dxil"]["triple_rewritten"] is True
        assert info["dxil"]["removed_named_metadata"] == ["xbox.compiler.options"]
        assert info["cleared_root_flags"] == "0x3000000"
        assert retargeted_blob[4:20] == bytes(16)

        after = validate_shader(probe, retargeted_blob, ret_secs[b"DXIL"], optional_modes=("module",))
        assert after["container"]["status"] == "0x00000000", after["container"]
        assert not after["container"]["diagnostics"], after["container"]["diagnostics"]
        assert after["ir"]["target_triple"] == "dxil-ms-dx"
        module_records.append({k: after["module"][k] for k in ("call_hr", "status") if k in after["module"]})

        # Verify STAT reflection bitcode was also retargeted cleanly
        if b"STAT" in ret_secs:
            _, _, _, st_bc = rt.unpack_dxil_program(ret_secs[b"STAT"])
            st_ir = subprocess.run([str(probe), "--disassemble"], input=st_bc, capture_output=True,
                                   check=True, timeout=10).stdout.decode("utf-8", "replace")
            assert 'target triple = "dxil-ms-dx"' in st_ir
            assert "xbox.compiler.options" not in st_ir

        # Verify D3D12 root signature deserializer + serializer accept the retargeted container
        root_res = json.loads(subprocess.check_output([str(probe)], input=retargeted_blob))
        assert root_res["legacy_hr"] == root_res["versioned_hr"] == root_res["original_serialize_hr"] == "0x00000000"

        # Re-sign the retargeted container via dxil.dll and verify both validation and CLI --sign.
        signed_blob = rt.sign_container(probe, retargeted_blob)
        assert len(signed_blob) == len(retargeted_blob)
        assert signed_blob[4:20] != bytes(16)
        signed_after = validate_shader(probe, signed_blob, parts(signed_blob)[b"DXIL"], optional_modes=("module",))
        assert signed_after["container"]["status"] == "0x00000000", signed_after["container"]
        assert not signed_after["container"]["diagnostics"], signed_after["container"]["diagnostics"]

        with TemporaryDirectory() as tmp:
            tmp_dir = Path(tmp)
            xb2_path = tmp_dir / f"{shader_path.stem}_xb2.dxbc"
            unsigned_path = tmp_dir / f"{shader_path.stem}_unsigned.dxbc"
            signed_path = tmp_dir / f"{shader_path.stem}_signed.dxbc"
            xb2_path.write_bytes(xb2_blob)
            unsigned_path.write_bytes(retargeted_blob)

            unsigned_pso = None
            if device_probe is not None:
                # Negative control 4: unsigned (zero-digest) retargeted DXBC must fail CreateComputePipelineState.
                unsigned_pso = _probe_compute_pso(device_probe, retargeted_blob, backend=backend,
                                                  intrinsics=after["ir"]["xbox_intrinsics"])
                if unsigned_pso is None:
                    print("No compatible D3D12 hardware adapter visible; skipping hardware DXIL retarget check.")
                    return SKIP
                assert unsigned_pso[root_key] == "0x00000000", unsigned_pso
                assert unsigned_pso[pso_key] != "0x00000000", unsigned_pso

            cli_cmd = [sys.executable, "-B", str(tool), str(xb2_path), str(signed_path),
                       "--sign", "--probe", str(probe)]
            if device_probe is not None:
                cli_cmd.extend(["--pso-probe", str(device_probe)])
                if backend == "warp":
                    cli_cmd.append("--warp-only")
            cli_res = json.loads(subprocess.check_output(cli_cmd))
            assert cli_res["signed"] is True
            assert cli_res["digest"] == signed_blob[4:20].hex()
            assert signed_path.read_bytes() == signed_blob

            pso_summary = "skipped"
            if device_probe is not None:
                signed_pso = cli_res["pso"]
                adapter_sm = int(signed_pso[sm_key], 16)
                if shader_path.name == "compute_root.cso" or adapter_sm >= 0x65:
                    assert signed_pso[root_key] == "0x00000000", signed_pso
                    assert signed_pso[pso_key] == "0x00000000", signed_pso

                    # Negative control 5: flipping one bit in the digest must fail CreateComputePipelineState.
                    tampered_blob = signed_blob[:4] + bytes([signed_blob[4] ^ 1]) + signed_blob[5:]
                    tampered_pso = _probe_compute_pso(device_probe, tampered_blob, backend=backend,
                                                      intrinsics=after["ir"]["xbox_intrinsics"])
                    assert tampered_pso is not None and tampered_pso[pso_key] != "0x00000000", tampered_pso
                    pso_summary = (
                        f"{backend}:unsigned={unsigned_pso[pso_key]} "
                        f"signed={signed_pso[pso_key]} tampered={tampered_pso[pso_key]}"
                    )
                else:
                    pso_summary = f"{backend}:skipped_sm=0x{adapter_sm:x}"

            if offscreen is not None and shader_path.name == "compute_root.cso":
                bad_run = subprocess.run([str(offscreen), backend, "--compute-root-file", str(unsigned_path)],
                                         capture_output=True, text=True, timeout=45)
                if backend == "hardware" and bad_run.returncode == SKIP:
                    print("No D3D12-capable hardware adapter visible in offscreen_check; skipping.")
                    return SKIP
                assert bad_run.returncode == 1 and "Create compute pipeline" in bad_run.stderr, (
                    bad_run.returncode, bad_run.stdout, bad_run.stderr
                )
                good_run = subprocess.run([str(offscreen), backend, "--compute-root-file", str(signed_path)],
                                          capture_output=True, text=True, timeout=45)
                if backend == "hardware" and good_run.returncode == SKIP:
                    print("No D3D12-capable hardware adapter visible in offscreen_check; skipping.")
                    return SKIP
                assert good_run.returncode == 0, (good_run.returncode, good_run.stdout, good_run.stderr)
                assert "mismatches=0" in good_run.stdout, good_run.stdout
                pso_summary += " dispatch=ok"

        print(
            f"fixture {shader_path.name}: container=0x00000000 "
            f"module={after['module'].get('status', after['module']['call_hr'])} "
            f"signed={signed_blob[4:20].hex()[:8]}.. pso=({pso_summary})"
        )

    version = file_version(validator_dll)
    outcome = module_outcome(module_records, version)
    print(f"validator={validator_dll.name} file_version={'.'.join(map(str, version))} module_only={outcome}")
    if outcome == "refused":
        print("Module-only validation skipped on 101.x validator; container validation passed.")
        return SKIP
    return 0


def main() -> int:
    check_pure_python_controls()
    if len(sys.argv) == 1:
        return 0
    if len(sys.argv) not in (5, 7, 8):
        raise SystemExit(
            "Usage: dxil_retarget_check.py [root_probe.exe shader1.cso shader2.cso dxil.dll "
            "[root_device_probe.exe offscreen_check.exe [warp|hardware]]]"
        )
    probe, shader1, shader2, validator_dll = map(Path, sys.argv[1:5])
    device_probe, offscreen = (map(Path, sys.argv[5:7]) if len(sys.argv) >= 7 else (None, None))
    backend = sys.argv[7] if len(sys.argv) == 8 else "warp"
    return check_validator_fixtures(probe, [shader1, shader2], validator_dll, device_probe, offscreen, backend=backend)


if __name__ == "__main__":
    sys.exit(main())
