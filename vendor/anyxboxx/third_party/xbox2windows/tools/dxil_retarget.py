"""Retarget Xbox Scarlett DXIL modules (dxil-ms-xb2) to desktop DXIL (dxil-ms-dx)."""

import argparse
from collections import defaultdict
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import NamedTemporaryFile

from resource_inventory import parts

BITCODE_MAGIC = b"BC\xc0\xde"
SOURCE_TRIPLE = "dxil-ms-xb2"
DESKTOP_TRIPLE = "dxil-ms-dx"
DEFAULT_DROP_METADATA = ("xbox.compiler.options",)
XBOX_ROOT_SIGNATURE_FLAGS = 0x03000000

BLOCKINFO_BLOCK_ID = 0
MODULE_BLOCK_ID = 8
METADATA_BLOCK_ID = 15

BLOCKINFO_CODE_SETBID = 1
MODULE_CODE_TRIPLE = 2
METADATA_NAME = 4
METADATA_NAMED_NODE = 10

ABBREV_END_BLOCK = 0
ABBREV_ENTER_SUBBLOCK = 1
ABBREV_DEFINE_ABBREV = 2
ABBREV_UNABBREV_RECORD = 3

ENC_FIXED = 1
ENC_VBR = 2
ENC_ARRAY = 3
ENC_CHAR6 = 4
ENC_BLOB = 5

CHAR6_ALPHABET = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._"
CHAR6_INDEX = {ord(ch): idx for idx, ch in enumerate(CHAR6_ALPHABET)}


class BitReader:
    def __init__(self, data: bytes):
        self.data = data
        self.bitpos = 0
        self.nbits = len(data) * 8

    def read(self, width: int) -> int:
        if width < 0 or width > 64:
            raise ValueError(f"Invalid bitcode operand width {width}")
        if self.bitpos + width > self.nbits:
            raise ValueError("Unexpected end of LLVM bitcode stream")
        value = 0
        shift = 0
        remaining = width
        while remaining > 0:
            byte_idx = self.bitpos >> 3
            bit_off = self.bitpos & 7
            take = min(remaining, 8 - bit_off)
            chunk = (self.data[byte_idx] >> bit_off) & ((1 << take) - 1)
            value |= chunk << shift
            self.bitpos += take
            shift += take
            remaining -= take
        return value

    def read_vbr(self, width: int) -> int:
        if width < 2 or width > 32:
            raise ValueError(f"Invalid VBR width {width}")
        value = 0
        shift = 0
        mask = (1 << (width - 1)) - 1
        high = 1 << (width - 1)
        while True:
            piece = self.read(width)
            payload = piece & mask
            if payload.bit_length() > 64 - shift:
                raise ValueError("VBR integer exceeds 64 bits")
            value |= payload << shift
            if not (piece & high):
                return value
            shift += width - 1
            if shift > 64:
                raise ValueError("VBR integer exceeds 64 bits")

    def align32(self) -> None:
        rem = self.bitpos & 31
        if rem:
            pad = 32 - rem
            if self.bitpos + pad > self.nbits:
                raise ValueError("Unexpected end of LLVM bitcode stream during alignment")
            self.bitpos += pad


class BitWriter:
    def __init__(self):
        self.buf = bytearray()
        self.cur = 0
        self.cur_bits = 0

    @property
    def bitpos(self) -> int:
        return len(self.buf) * 8 + self.cur_bits

    def write(self, value: int, width: int) -> None:
        if width < 0 or value < 0:
            raise ValueError("Invalid bit write")
        remaining = width
        while remaining > 0:
            space = 8 - self.cur_bits
            take = min(remaining, space)
            self.cur |= (value & ((1 << take) - 1)) << self.cur_bits
            self.cur_bits += take
            value >>= take
            remaining -= take
            if self.cur_bits == 8:
                self.buf.append(self.cur)
                self.cur = 0
                self.cur_bits = 0

    def write_vbr(self, value: int, width: int) -> None:
        if width < 2 or value < 0:
            raise ValueError("Invalid VBR write")
        mask = (1 << (width - 1)) - 1
        high = 1 << (width - 1)
        while True:
            chunk = value & mask
            value >>= width - 1
            if value:
                self.write(chunk | high, width)
            else:
                self.write(chunk, width)
                return

    def align32(self) -> None:
        rem = self.bitpos & 31
        if rem:
            self.write(0, 32 - rem)

    def write_aligned_bytes(self, payload: bytes) -> None:
        if self.cur_bits != 0 or (len(self.buf) & 3) != 0:
            raise ValueError("Aligned write called off a 32-bit boundary")
        self.buf.extend(payload)

    def patch_u32(self, byte_offset: int, value: int) -> None:
        struct.pack_into("<I", self.buf, byte_offset, value)

    def finish(self) -> bytes:
        if self.cur_bits != 0:
            raise ValueError("Bitstream did not end on a byte boundary")
        return bytes(self.buf)


def _read_abbrev_def(reader: BitReader) -> list[tuple]:
    num_ops = reader.read_vbr(5)
    ops = []
    for _ in range(num_ops):
        if reader.read(1):
            ops.append(("LIT", reader.read_vbr(8)))
        else:
            enc = reader.read(3)
            if enc in (ENC_FIXED, ENC_VBR):
                ops.append((enc, reader.read_vbr(5)))
            elif enc in (ENC_ARRAY, ENC_CHAR6, ENC_BLOB):
                ops.append((enc, None))
            else:
                raise ValueError(f"Unknown LLVM abbreviation encoding {enc}")
    return ops


def _read_abbrev_record(reader: BitReader, ops: list[tuple]) -> tuple[int, list[int], bytes | None]:
    vals = []
    blob = None
    idx = 0
    while idx < len(ops):
        kind, data = ops[idx]
        if kind == "LIT":
            vals.append(data)
        elif kind == ENC_FIXED:
            vals.append(reader.read(data))
        elif kind == ENC_VBR:
            vals.append(reader.read_vbr(data))
        elif kind == ENC_ARRAY:
            if idx + 1 >= len(ops):
                raise ValueError("Array abbreviation missing element encoding")
            count = reader.read_vbr(6)
            idx += 1
            elem_kind, elem_data = ops[idx]
            for _ in range(count):
                if elem_kind == ENC_FIXED:
                    vals.append(reader.read(elem_data))
                elif elem_kind == ENC_VBR:
                    vals.append(reader.read_vbr(elem_data))
                elif elem_kind == ENC_CHAR6:
                    vals.append(ord(CHAR6_ALPHABET[reader.read(6)]))
                else:
                    raise ValueError(f"Unsupported array element encoding {elem_kind}")
        elif kind == ENC_CHAR6:
            vals.append(ord(CHAR6_ALPHABET[reader.read(6)]))
        elif kind == ENC_BLOB:
            nbytes = reader.read_vbr(6)
            reader.align32()
            start = reader.bitpos >> 3
            if start + nbytes > len(reader.data):
                raise ValueError("Truncated bitcode blob operand")
            blob = reader.data[start:start + nbytes]
            reader.bitpos += nbytes * 8
            reader.align32()
        else:
            raise ValueError(f"Unknown abbreviation operand {kind}")
        idx += 1
    if not vals:
        raise ValueError("Abbreviated record missing record code")
    return vals[0], vals[1:], blob


def parse_bitcode_block(
    reader: BitReader,
    block_id: int,
    abbrev_width: int,
    end_bit: int,
    blockinfo_abbrevs: dict[int, list[list[tuple]]],
    recurse_ids: set[int],
) -> list[tuple]:
    abbrevs = list(blockinfo_abbrevs.get(block_id, ()))
    current_bi_block = None
    items = []
    closed = block_id < 0
    while reader.bitpos < end_bit:
        code = reader.read(abbrev_width)
        if code == ABBREV_END_BLOCK:
            reader.align32()
            closed = True
            break
        if code == ABBREV_ENTER_SUBBLOCK:
            sub_id = reader.read_vbr(8)
            sub_aw = reader.read_vbr(4)
            if sub_aw < 2:
                raise ValueError(f"Invalid subblock abbreviation width {sub_aw}")
            reader.align32()
            sub_words = reader.read(32)
            sub_start = reader.bitpos >> 3
            sub_end = sub_start + sub_words * 4
            if sub_end > len(reader.data) or sub_end * 8 > end_bit:
                raise ValueError("Subblock extends past parent bitcode block")
            if sub_id == BLOCKINFO_BLOCK_ID:
                parse_bitcode_block(reader, BLOCKINFO_BLOCK_ID, sub_aw, sub_end * 8, blockinfo_abbrevs, set())
                items.append(("RAW_BLOCK", sub_id, sub_aw, reader.data[sub_start:sub_end]))
            elif sub_id in recurse_ids:
                sub_items = parse_bitcode_block(reader, sub_id, sub_aw, sub_end * 8, blockinfo_abbrevs, recurse_ids)
                items.append(("BLOCK", sub_id, sub_aw, sub_items))
            else:
                items.append(("RAW_BLOCK", sub_id, sub_aw, reader.data[sub_start:sub_end]))
            reader.bitpos = sub_end * 8
        elif code == ABBREV_DEFINE_ABBREV:
            ops = _read_abbrev_def(reader)
            if block_id == BLOCKINFO_BLOCK_ID:
                if current_bi_block is None:
                    raise ValueError("DEFINE_ABBREV in BLOCKINFO before SETBID")
                blockinfo_abbrevs.setdefault(current_bi_block, []).append(ops)
            else:
                abbrevs.append(ops)
            items.append(("DEFINE_ABBREV", ops))
        elif code == ABBREV_UNABBREV_RECORD:
            rec_code = reader.read_vbr(6)
            num_ops = reader.read_vbr(6)
            vals = [reader.read_vbr(6) for _ in range(num_ops)]
            if block_id == BLOCKINFO_BLOCK_ID and rec_code == BLOCKINFO_CODE_SETBID:
                if not vals:
                    raise ValueError("SETBID record missing block ID")
                current_bi_block = vals[0]
            items.append(("UNABBREV", rec_code, vals))
        else:
            abbrev_idx = code - 4
            if abbrev_idx >= len(abbrevs):
                raise ValueError(f"Abbreviation index {abbrev_idx} out of range")
            ops = abbrevs[abbrev_idx]
            rec_code, rec_vals, blob = _read_abbrev_record(reader, ops)
            items.append(("ABBREV", code, ops, rec_code, rec_vals, blob))
    if not closed:
        raise ValueError(f"Unterminated LLVM bitcode block {block_id}")
    return items


def write_bitcode_block(writer: BitWriter, abbrev_width: int, items: list[tuple]) -> None:
    for item in items:
        kind = item[0]
        if kind == "RAW_BLOCK":
            _, sub_id, sub_aw, raw = item
            if len(raw) % 4:
                raise ValueError("Raw subblock length must be a multiple of 4 bytes")
            writer.write(ABBREV_ENTER_SUBBLOCK, abbrev_width)
            writer.write_vbr(sub_id, 8)
            writer.write_vbr(sub_aw, 4)
            writer.align32()
            writer.write(len(raw) // 4, 32)
            writer.write_aligned_bytes(raw)
        elif kind == "BLOCK":
            _, sub_id, sub_aw, sub_items = item
            writer.write(ABBREV_ENTER_SUBBLOCK, abbrev_width)
            writer.write_vbr(sub_id, 8)
            writer.write_vbr(sub_aw, 4)
            writer.align32()
            len_offset = len(writer.buf)
            writer.write(0, 32)
            body_start = len(writer.buf)
            write_bitcode_block(writer, sub_aw, sub_items)
            writer.write(ABBREV_END_BLOCK, sub_aw)
            writer.align32()
            writer.patch_u32(len_offset, (len(writer.buf) - body_start) // 4)
        elif kind == "DEFINE_ABBREV":
            _, ops = item
            writer.write(ABBREV_DEFINE_ABBREV, abbrev_width)
            writer.write_vbr(len(ops), 5)
            for op_kind, op_data in ops:
                if op_kind == "LIT":
                    writer.write(1, 1)
                    writer.write_vbr(op_data, 8)
                else:
                    writer.write(0, 1)
                    writer.write(op_kind, 3)
                    if op_kind in (ENC_FIXED, ENC_VBR):
                        writer.write_vbr(op_data, 5)
        elif kind == "UNABBREV":
            _, rec_code, vals = item
            writer.write(ABBREV_UNABBREV_RECORD, abbrev_width)
            writer.write_vbr(rec_code, 6)
            writer.write_vbr(len(vals), 6)
            for value in vals:
                writer.write_vbr(value, 6)
        elif kind == "ABBREV":
            _, code, ops, rec_code, rec_vals, blob = item
            writer.write(code, abbrev_width)
            all_vals = [rec_code, *rec_vals]
            val_idx = 0
            op_idx = 0
            while op_idx < len(ops):
                op_kind, op_data = ops[op_idx]
                if op_kind == "LIT":
                    if all_vals[val_idx] != op_data:
                        raise ValueError("Abbreviated literal operand mismatch")
                    val_idx += 1
                elif op_kind == ENC_FIXED:
                    writer.write(all_vals[val_idx], op_data)
                    val_idx += 1
                elif op_kind == ENC_VBR:
                    writer.write_vbr(all_vals[val_idx], op_data)
                    val_idx += 1
                elif op_kind == ENC_ARRAY:
                    tail = all_vals[val_idx:]
                    writer.write_vbr(len(tail), 6)
                    op_idx += 1
                    elem_kind, elem_data = ops[op_idx]
                    for value in tail:
                        if elem_kind == ENC_FIXED:
                            writer.write(value, elem_data)
                        elif elem_kind == ENC_VBR:
                            writer.write_vbr(value, elem_data)
                        elif elem_kind == ENC_CHAR6:
                            writer.write(CHAR6_INDEX[value], 6)
                        else:
                            raise ValueError(f"Unsupported array element encoding {elem_kind}")
                    val_idx = len(all_vals)
                elif op_kind == ENC_CHAR6:
                    writer.write(CHAR6_INDEX[all_vals[val_idx]], 6)
                    val_idx += 1
                elif op_kind == ENC_BLOB:
                    if blob is None:
                        raise ValueError("Missing blob payload for abbreviated record")
                    writer.write_vbr(len(blob), 6)
                    writer.align32()
                    writer.write_aligned_bytes(blob)
                    writer.align32()
                else:
                    raise ValueError(f"Unknown abbreviation operand {op_kind}")
                op_idx += 1
        else:
            raise ValueError(f"Unknown bitstream item {kind}")


def _record_code(item: tuple) -> int | None:
    if item[0] == "UNABBREV":
        return item[1]
    if item[0] == "ABBREV":
        return item[3]
    return None


def _record_bytes(item: tuple) -> bytes:
    if item[0] == "UNABBREV":
        vals, blob = item[2], None
    elif item[0] == "ABBREV":
        vals, blob = item[4], item[5]
    else:
        raise ValueError("Expected a bitcode record")
    if blob is not None:
        return blob
    if any(not 0 <= value <= 255 for value in vals):
        raise ValueError("Bitcode string operand out of byte range")
    return bytes(vals)


def parse_bitcode(data: bytes, recurse_ids: set[int] = frozenset({MODULE_BLOCK_ID, METADATA_BLOCK_ID})) -> list[tuple]:
    if len(data) < 4 or data[:4] != BITCODE_MAGIC:
        raise ValueError("Expected LLVM bitcode starting with BC\\xc0\\xde")
    reader = BitReader(data)
    reader.bitpos = 32
    return parse_bitcode_block(reader, -1, 2, reader.nbits, {}, set(recurse_ids))


def serialize_bitcode(items: list[tuple]) -> bytes:
    writer = BitWriter()
    writer.write_aligned_bytes(BITCODE_MAGIC)
    write_bitcode_block(writer, 2, items)
    return writer.finish()


def retarget_bitcode(
    data: bytes,
    *,
    source_triple: str = SOURCE_TRIPLE,
    target_triple: str = DESKTOP_TRIPLE,
    drop_named_metadata: tuple[str, ...] = DEFAULT_DROP_METADATA,
) -> tuple[bytes, dict]:
    """Retarget an LLVM 3.7 DXIL bitcode module and report the applied edits."""
    top = parse_bitcode(data)
    drop_set = set(drop_named_metadata)
    original_triple = None
    final_triple = None
    triple_rewritten = False
    removed_metadata = []

    for item in top:
        if item[0] != "BLOCK" or item[1] != MODULE_BLOCK_ID:
            continue
        mod_items = item[3]
        for idx, mod_item in enumerate(mod_items):
            if _record_code(mod_item) == MODULE_CODE_TRIPLE:
                original_triple = _record_bytes(mod_item).decode("ascii")
                if source_triple == "*" or original_triple == source_triple:
                    mod_items[idx] = ("UNABBREV", MODULE_CODE_TRIPLE, list(target_triple.encode("ascii")))
                    final_triple = target_triple
                    triple_rewritten = original_triple != target_triple
                else:
                    final_triple = original_triple
            elif mod_item[0] == "BLOCK" and mod_item[1] == METADATA_BLOCK_ID:
                md_items = mod_item[3]
                rewritten_md = []
                i = 0
                while i < len(md_items):
                    cur = md_items[i]
                    if _record_code(cur) == METADATA_NAME:
                        name = _record_bytes(cur).decode("utf-8", "replace")
                        if name in drop_set:
                            if i + 1 >= len(md_items) or _record_code(md_items[i + 1]) != METADATA_NAMED_NODE:
                                raise ValueError(f"METADATA_NAME '{name}' is not followed by METADATA_NAMED_NODE")
                            removed_metadata.append(name)
                            i += 2
                            continue
                    rewritten_md.append(cur)
                    i += 1
                mod_item[3][:] = rewritten_md

    output = serialize_bitcode(top)
    info = {
        "original_triple": original_triple,
        "target_triple": final_triple,
        "triple_rewritten": triple_rewritten,
        "removed_named_metadata": removed_metadata,
        "input_bytes": len(data),
        "output_bytes": len(output),
    }
    return output, info


def unpack_dxil_program(program: bytes) -> tuple[int, int, int, bytes]:
    """Validate a DXIL/STAT program header and return (version_type, dxil_version, bitcode_offset, bitcode)."""
    if len(program) < 24:
        raise ValueError("Truncated DXIL program header")
    version_type, words, magic, dxil_version, bitcode_offset, bitcode_size = struct.unpack_from("<II4sIII", program)
    start = 8 + bitcode_offset
    if (
        magic != b"DXIL"
        or bitcode_offset < 16
        or bitcode_size < 4
        or not start + bitcode_size <= 4 * words <= len(program)
        or program[start:start + 4] != BITCODE_MAGIC
    ):
        raise ValueError("DXIL program header does not describe a valid bitcode module")
    return version_type, dxil_version, bitcode_offset, program[start:start + bitcode_size]


def pack_dxil_program(version_type: int, dxil_version: int, bitcode: bytes) -> bytes:
    """Wrap bitcode in a canonical desktop DXIL program header (BitcodeOffset=16)."""
    padded = bitcode + bytes((-len(bitcode)) & 3)
    total_words = (24 + len(padded)) // 4
    return struct.pack("<II4sIII", version_type, total_words, b"DXIL", dxil_version, 16, len(bitcode)) + padded


def build_dxbc_container(part_list: list[tuple[bytes, bytes]], digest: bytes = bytes(16)) -> bytes:
    """Assemble a DXBC container from ordered (fourcc, payload) pairs."""
    if len(digest) != 16:
        raise ValueError("DXBC container digest must be 16 bytes")
    count = len(part_list)
    table_end = 32 + 4 * count
    offsets = []
    body = bytearray()
    seen = set()
    for tag, payload in part_list:
        if len(tag) != 4 or tag in seen:
            raise ValueError(f"Invalid or duplicate DXBC part tag {tag!r}")
        seen.add(tag)
        offsets.append(table_end + len(body))
        body.extend(tag)
        body.extend(struct.pack("<I", len(payload)))
        body.extend(payload)
    total = table_end + len(body)
    return (
        b"DXBC"
        + digest
        + struct.pack("<3I", 1, total, count)
        + struct.pack(f"<{count}I", *offsets)
        + bytes(body)
    )


def retarget_container(
    blob: bytes,
    *,
    source_triple: str = SOURCE_TRIPLE,
    target_triple: str = DESKTOP_TRIPLE,
    drop_named_metadata: tuple[str, ...] = DEFAULT_DROP_METADATA,
    clear_root_flags: bool = True,
) -> tuple[bytes, dict]:
    """Retarget a DXBC container's DXIL (and STAT) bitcode and optionally clear Xbox RTS0 flags."""
    sections = parts(blob)
    if b"DXIL" not in sections:
        raise ValueError("DXBC container has no DXIL part")

    ver_type, dxil_ver, orig_offset, orig_bc = unpack_dxil_program(sections[b"DXIL"])
    new_bc, bc_info = retarget_bitcode(
        orig_bc,
        source_triple=source_triple,
        target_triple=target_triple,
        drop_named_metadata=drop_named_metadata,
    )
    new_dxil = pack_dxil_program(ver_type, dxil_ver, new_bc)

    stat_retargeted = False
    original_root_flags = None
    cleared_root_flags = 0
    notes = []
    rebuilt_parts = []
    for tag, payload in sections.items():
        if tag == b"DXIL":
            rebuilt_parts.append((tag, new_dxil))
        elif tag == b"STAT" and len(payload) >= 24 and payload[8:12] == b"DXIL":
            st_ver_type, st_dxil_ver, _, st_bc = unpack_dxil_program(payload)
            new_st_bc, st_info = retarget_bitcode(
                st_bc,
                source_triple=source_triple,
                target_triple=target_triple,
                drop_named_metadata=drop_named_metadata,
            )
            rebuilt_parts.append((tag, pack_dxil_program(st_ver_type, st_dxil_ver, new_st_bc)))
            stat_retargeted = st_info["triple_rewritten"] or bool(st_info["removed_named_metadata"])
        elif tag == b"RTS0" and len(payload) >= 24:
            version, _, _, _, _, flags = struct.unpack_from("<6I", payload)
            original_root_flags = flags
            if flags & XBOX_ROOT_SIGNATURE_FLAGS:
                if clear_root_flags and version in (1, 2):
                    cleared_root_flags = flags & XBOX_ROOT_SIGNATURE_FLAGS
                    payload = payload[:20] + struct.pack("<I", flags & ~XBOX_ROOT_SIGNATURE_FLAGS) + payload[24:]
                elif version not in (1, 2):
                    notes.append(f"RTS0 version {version} keeps unhandled Xbox flags {flags & XBOX_ROOT_SIGNATURE_FLAGS:#x}")
            rebuilt_parts.append((tag, payload))
        else:
            rebuilt_parts.append((tag, payload))

    out_blob = build_dxbc_container(rebuilt_parts)
    info = {
        "kind": "dxbc",
        "dxil": bc_info,
        "original_bitcode_offset": orig_offset,
        "stat_retargeted": stat_retargeted,
        "notes": notes,
        "original_root_flags": hex(original_root_flags) if original_root_flags is not None else None,
        "cleared_root_flags": hex(cleared_root_flags),
        "input_bytes": len(blob),
        "output_bytes": len(out_blob),
    }
    return out_blob, info


def sign_container(probe: Path, blob: bytes) -> bytes:
    """Validate and sign a retargeted DXBC container via root_probe --sign (dxil.dll)."""
    if len(blob) < 32 or blob[:4] != b"DXBC":
        raise ValueError("Cannot sign non-DXBC input")
    run = subprocess.run([str(probe), "--sign"], input=blob, capture_output=True, timeout=10)
    if run.returncode != 0:
        diag = run.stderr.decode("utf-8", "replace").rstrip("\0\r\n")
        raise RuntimeError(f"DXIL signing failed (exit {run.returncode}): {diag or 'no diagnostics'}")
    signed = run.stdout
    if len(signed) < 32 or signed[:4] != b"DXBC":
        raise RuntimeError("DXIL signer returned invalid DXBC output")
    if signed[4:20] == bytes(16):
        raise RuntimeError("DXIL signer left container digest zeroed")
    return signed


def check_compute_pso(device_probe: Path, blob: bytes, *, warp_only: bool = False,
                      intrinsics: dict | None = None, status_of=None) -> dict:
    """Test CreateRootSignature and CreateComputePipelineState via root_device_probe --compute-pso.

    `intrinsics` carries the inventory of the module that would be submitted, and the gate is
    mandatory: a module that still executes an xbox.* call, or one whose inventory is missing,
    is decided without starting the probe process, so it produces no submission at all.
    """
    if len(blob) < 32 or blob[:4] != b"DXBC":
        raise ValueError("Cannot create compute PSO from non-DXBC input")
    from xbox_intrinsic_gate import ALLOW, decide, not_checked

    if intrinsics is None:
        gate = not_checked("module-not-disassembled")
        gate["submitted"] = False
        return {"gate": gate}
    gate = decide(intrinsics, status_of)
    if gate["verdict"] != ALLOW:
        gate["submitted"] = False
        return {"gate": gate}
    command = [str(device_probe), "--compute-pso"]
    if warp_only:
        command.append("--warp-only")
    run = subprocess.run(command, input=blob, capture_output=True, check=True, timeout=30)
    pso = json.loads(run.stdout)
    gate["submitted"] = True
    pso["gate"] = gate
    return pso


def validate_pe_containers(
    pe_path: Path,
    probe: Path,
    *,
    only_without_xbox_calls: bool,
    output_dir: Path | None,
    source_triple: str,
    target_triple: str,
    drop_named_metadata: tuple[str, ...],
    clear_root_flags: bool,
    sign: bool = False,
    pso_probe: Path | None = None,
    warp_only: bool = False,
) -> dict:
    """Extract DXBC containers from a PE image, retarget selected DXIL modules, and validate via root_probe."""
    import pefile
    from call_evidence import shader_containers
    from probe_roots import validate_shader
    from xbox_intrinsic_gate import reaches_xbox

    raw = pe_path.read_bytes()
    with pefile.PE(data=raw, fast_load=True) as pe:
        containers = shader_containers(raw, pe, defaultdict(list))
        results = []
        skipped_non_dxil = 0
        skipped_xbox_intrinsics = 0

        if output_dir is not None:
            output_dir.mkdir(parents=True, exist_ok=True)

        for item in containers:
            rva = item["rva"]
            blob = pe.get_data(int(rva, 16), item["bytes"])
            secs = parts(blob)
            if b"DXIL" not in secs:
                skipped_non_dxil += 1
                continue

            before = validate_shader(probe, blob, secs[b"DXIL"], optional_modes=("module",))
            xbox_calls = before["ir"]["xbox_call_sites"]
            if only_without_xbox_calls and reaches_xbox(before["ir"]["xbox_intrinsics"]):
                skipped_xbox_intrinsics += 1
                continue

            retargeted_blob, info = retarget_container(
                blob,
                source_triple=source_triple,
                target_triple=target_triple,
                drop_named_metadata=drop_named_metadata,
                clear_root_flags=clear_root_flags,
            )
            retargeted_secs = parts(retargeted_blob)
            after = validate_shader(probe, retargeted_blob, retargeted_secs[b"DXIL"], optional_modes=("module",))
            # The gate reads the final module: calls removed or lowered by the adapter are gone here.
            from xbox_intrinsic_gate import decide

            gate_decision = decide(after["ir"]["xbox_intrinsics"])

            signed = False
            digest = None
            pso_result = None
            if sign and after["container"].get("status") == "0x00000000":
                retargeted_blob = sign_container(probe, retargeted_blob)
                signed = True
                digest = retargeted_blob[4:20].hex()
            if pso_probe is not None and after["container"].get("status") == "0x00000000":
                pso_result = check_compute_pso(pso_probe, retargeted_blob, warp_only=warp_only,
                                               intrinsics=after["ir"]["xbox_intrinsics"])

            saved_path = None
            if output_dir is not None:
                out_file = output_dir / f"{pe_path.stem}_{rva}.dxbc"
                out_file.write_bytes(retargeted_blob)
                saved_path = str(out_file)

            entry = {
                "rva": rva,
                "original_bytes": len(blob),
                "retargeted_bytes": len(retargeted_blob),
                "dx_op_count": len(before["ir"]["dx_op_call_sites"]),
                "xbox_call_count": sum(xbox_calls.values()),
                "xbox_intrinsics": before["ir"]["xbox_intrinsics"],
                "gate": gate_decision,
                "retarget": info,
                "signed": signed,
                "digest": digest,
                "before": {
                    "triple": before["ir"]["target_triple"],
                    "container_status": before["container"].get("status"),
                    "module_status": before["module"].get("status"),
                    "container_diagnostics": before["container"]["diagnostics"],
                    "module_diagnostics": before["module"]["diagnostics"],
                },
                "after": {
                    "triple": after["ir"]["target_triple"],
                    "container_status": after["container"].get("status"),
                    "module_status": after["module"].get("status"),
                    "container_diagnostics": after["container"]["diagnostics"],
                    "module_diagnostics": after["module"]["diagnostics"],
                },
                "output_file": saved_path,
            }
            if pso_result is not None:
                entry["pso"] = pso_result
            results.append(entry)

    report = {
        "image": pe_path.name,
        "image_sha256": hashlib.sha256(raw).hexdigest(),
        "total_containers": len(containers),
        "skipped_non_dxil": skipped_non_dxil,
        "skipped_xbox_intrinsics": skipped_xbox_intrinsics,
        "processed": len(results),
        "container_passed": sum(r["after"]["container_status"] == "0x00000000" for r in results),
        "module_passed": sum(r["after"]["module_status"] == "0x00000000" for r in results),
        "signed_count": sum(r["signed"] for r in results),
        "gate_rejected": sum(r["gate"]["verdict"] == "REJECT" for r in results),
        "entries": results,
    }
    if pso_probe is not None:
        report["pso_refused"] = sum(
            r.get("pso", {}).get("gate", {}).get("verdict") == "REJECT" for r in results
        )
        report["warp_pso_passed"] = sum(
            r.get("pso", {}).get("warp_pso") == "0x00000000" for r in results
        )
        if not warp_only:
            report["hardware_pso_passed"] = sum(
                r.get("pso", {}).get("hardware_pso") == "0x00000000" for r in results
            )
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", nargs="?", type=Path, help="Input DXBC container or raw DXIL bitcode file")
    parser.add_argument("output", nargs="?", type=Path, help="Output file path")
    parser.add_argument("--pe", type=Path, help="Extract, retarget, and validate containers embedded in a PE image")
    parser.add_argument("--output-dir", type=Path, help="Optional directory to write retargeted containers in --pe mode")
    parser.add_argument("--report", type=Path, help="Optional JSON report path in --pe mode")
    parser.add_argument("--only-without-xbox-calls", action="store_true",
                        help="In --pe mode, process only DXIL modules that neither call nor otherwise "
                             "reach an xbox.* symbol")
    parser.add_argument("--source-triple", default=SOURCE_TRIPLE,
                        help=f"Source target triple to match (default: {SOURCE_TRIPLE}; '*' matches any)")
    parser.add_argument("--target-triple", default=DESKTOP_TRIPLE,
                        help=f"Replacement target triple (default: {DESKTOP_TRIPLE})")
    parser.add_argument("--drop-metadata", action="append", default=None,
                        help="Named metadata to remove (repeatable; default: xbox.compiler.options)")
    parser.add_argument("--keep-root-flags", action="store_true",
                        help="Leave RTS0 flags unmodified instead of clearing Xbox flags 0x03000000")
    parser.add_argument("--extract-module", action="store_true",
                        help="Write bare retargeted DXIL bitcode instead of a DXBC container")
    parser.add_argument("--probe", type=Path, help="Optional root_probe.exe path to validate or sign the output")
    parser.add_argument("--sign", action="store_true",
                        help="Re-sign retargeted DXBC containers with dxil.dll via --probe")
    parser.add_argument("--pso-probe", type=Path,
                        help="Optional root_device_probe.exe path to test CreateComputePipelineState")
    parser.add_argument("--warp-only", action="store_true",
                        help="Use only the WARP adapter with --pso-probe")
    args = parser.parse_args()

    if not args.pe:
        for dead in ("--output-dir", "--report", "--only-without-xbox-calls"):
            if getattr(args, dead[2:].replace("-", "_")):
                parser.error(f"{dead} requires --pe")
    elif args.extract_module:
        parser.error("--extract-module does not apply to --pe")
    if args.sign and not args.probe:
        parser.error("--sign requires --probe")
    if args.sign and args.extract_module:
        parser.error("--sign cannot be combined with --extract-module")
    if args.pso_probe and args.extract_module:
        parser.error("--pso-probe cannot be combined with --extract-module")
    if args.warp_only and not args.pso_probe:
        parser.error("--warp-only requires --pso-probe")

    drop_md = tuple(args.drop_metadata) if args.drop_metadata is not None else DEFAULT_DROP_METADATA

    if args.pe:
        if args.input or args.output:
            parser.error("Do not pass positional input/output together with --pe")
        if not args.probe:
            parser.error("--pe mode requires --probe")
        report = validate_pe_containers(
            args.pe.resolve(strict=True),
            args.probe.resolve(strict=True),
            only_without_xbox_calls=args.only_without_xbox_calls,
            output_dir=args.output_dir,
            source_triple=args.source_triple,
            target_triple=args.target_triple,
            drop_named_metadata=drop_md,
            clear_root_flags=not args.keep_root_flags,
            sign=args.sign,
            pso_probe=args.pso_probe.resolve(strict=True) if args.pso_probe else None,
            warp_only=args.warp_only,
        )
        if args.report:
            dest = args.report.resolve()
            dest.parent.mkdir(parents=True, exist_ok=True)
            with NamedTemporaryFile(mode="w", dir=dest.parent, delete=False, encoding="utf-8") as handle:
                json.dump(report, handle, indent=2)
                handle.write("\n")
            Path(handle.name).replace(dest)
        print(json.dumps(report, indent=2))
        return 0

    if not args.input or not args.output:
        parser.error("Provide input and output paths, or use --pe")

    raw = args.input.read_bytes()
    if raw[:4] == b"DXBC":
        if args.extract_module:
            secs = parts(raw)
            if b"DXIL" not in secs:
                raise ValueError("DXBC container has no DXIL part")
            _, _, _, orig_bc = unpack_dxil_program(secs[b"DXIL"])
            result_bytes, info = retarget_bitcode(
                orig_bc,
                source_triple=args.source_triple,
                target_triple=args.target_triple,
                drop_named_metadata=drop_md,
            )
            info = {"kind": "bitcode", **info}
        else:
            result_bytes, info = retarget_container(
                raw,
                source_triple=args.source_triple,
                target_triple=args.target_triple,
                drop_named_metadata=drop_md,
                clear_root_flags=not args.keep_root_flags,
            )
            if args.sign:
                result_bytes = sign_container(args.probe.resolve(strict=True), result_bytes)
                info["signed"] = True
                info["digest"] = result_bytes[4:20].hex()
            else:
                info["signed"] = False
    elif raw[:4] == BITCODE_MAGIC:
        if args.sign or args.pso_probe:
            parser.error("--sign and --pso-probe require a DXBC container input")
        result_bytes, info = retarget_bitcode(
            raw,
            source_triple=args.source_triple,
            target_triple=args.target_triple,
            drop_named_metadata=drop_md,
        )
        info = {"kind": "bitcode", **info}
    else:
        raise ValueError("Input must be a DXBC container or raw LLVM bitcode")

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(result_bytes)

    dxil_info = info.get("dxil", info)
    if not dxil_info["triple_rewritten"] and dxil_info["original_triple"] != args.target_triple:
        print(
            f"warning: module triple {dxil_info['original_triple']!r} does not match "
            "--source-triple; output is not retargeted",
            file=sys.stderr,
        )

    if args.probe:
        probe = args.probe.resolve(strict=True)
        if info["kind"] == "dxbc":
            from probe_roots import validate_shader

            secs = parts(result_bytes)
            info["validation"] = validate_shader(probe, result_bytes, secs[b"DXIL"], optional_modes=("module",))
        else:
            run = subprocess.run([str(probe), "--module"], input=result_bytes, capture_output=True, check=True, timeout=10)
            rec = json.loads(run.stdout)
            rec["diagnostics"] = run.stderr.decode("utf-8", "replace").rstrip("\0\r\n").splitlines()
            info["validation"] = {"module": rec}

    if args.pso_probe:
        from xbox_intrinsic_gate import NOT_CHECKED

        # The gate needs the module's disassembly, which only --probe produces. The helper refuses
        # to submit without it; say why instead of leaving a silent NOT_CHECKED in the report.
        final_intrinsics = (info.get("validation") or {}).get("ir", {}).get("xbox_intrinsics")
        # Resolve the probe only when it will actually be used: without an inventory nothing runs.
        probe_path = args.pso_probe.resolve(strict=True) if final_intrinsics is not None else args.pso_probe
        info["pso"] = check_compute_pso(
            probe_path,
            result_bytes,
            warp_only=args.warp_only,
            intrinsics=final_intrinsics,
        )
        if info["pso"]["gate"]["verdict"] == NOT_CHECKED:
            print(
                "warning: --pso-probe needs --probe to gate the final module; "
                "the module was not disassembled, so nothing was submitted",
                file=sys.stderr,
            )

    print(json.dumps(info, indent=2))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError) as exc:
        sys.exit(f"{Path(__file__).name}: {exc}")
