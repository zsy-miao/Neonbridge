"""Read the deployment metadata in an Xbox microsoft_gameconfig.bin registry hive."""
import struct
from functools import reduce
from operator import xor


def require(condition, detail):
    if not condition:
        raise ValueError(f"Invalid binary game config: {detail}")


class ConfigHive:
    """Read the allocated cells and small, direct subkey lists used by game configs."""

    def __init__(self, data):
        require(len(data) >= 8192 and data[:4] == b"regf", "missing registry header")
        require(data[4:8] == data[8:12], "registry transaction is incomplete")
        major, minor, file_type, file_format, self.root, size = struct.unpack_from("<6I", data, 20)
        require(major == 1 and 2 <= minor <= 6 and file_type == 0 and file_format == 1,
                "unsupported registry format")
        checksum = reduce(xor, struct.unpack_from("<127I", data))
        if checksum in (0, 0xffffffff):
            checksum ^= 1
        require(checksum == struct.unpack_from("<I", data, 508)[0], "header checksum")
        require(0 < size <= len(data) - 4096, "hive size")
        self.data = memoryview(data)[4096:4096 + size]
        self.budget = size * 8  # Bound repeated references to the same cells.
        self.allocated = bytearray((size + 7) // 8)
        block = 0
        while block < size:
            require(block + 32 <= size and self.data[block:block+4] == b"hbin", "hive bin header")
            offset, length = struct.unpack_from("<II", self.data, block + 4)
            end = block + length
            require(offset == block and length >= 4096 and length % 4096 == 0 and end <= size, "hive bin bounds")
            pos = block + 32
            while pos < end:
                span, = struct.unpack_from("<i", self.data, pos)
                require(span and span % 8 == 0 and pos + abs(span) <= end, "cell bounds")
                self.allocated[pos // 8] = span < 0
                pos += abs(span)
            block = end

    def cell(self, offset, minimum, signature=b""):
        require(offset % 8 == 0 and offset < len(self.data) and self.allocated[offset // 8], "invalid cell reference")
        size = -struct.unpack_from("<i", self.data, offset)[0]
        cell = self.data[offset+4:offset+size]
        self.budget -= len(cell)
        require(self.budget >= 0, "excessive repeated cell references")
        require(len(cell) >= minimum and cell[:len(signature)] == signature, "invalid cell contents")
        return cell

    @staticmethod
    def name(cell, start, length, compressed):
        require(start + length <= len(cell), "truncated name")
        return bytes(cell[start:start+length]).decode("latin-1" if compressed else "utf-16-le")

    def subkeys(self, offset):
        key = self.cell(offset, 76, b"nk")
        count, _, index = struct.unpack_from("<III", key, 20)
        children = {}
        if count:
            items = self.cell(index, 4)
            require(items[:2] in (b"lf", b"lh", b"li"), "unsupported subkey list")
            stride = 4 if items[:2] == b"li" else 8
            require(count == struct.unpack_from("<H", items, 2)[0] and 4 + count * stride <= len(items),
                    "subkey list bounds")
            for i in range(count):
                child, = struct.unpack_from("<I", items, 4 + i * stride)
                node = self.cell(child, 76, b"nk")
                require(struct.unpack_from("<I", node, 16)[0] == offset, "wrong subkey parent")
                length, = struct.unpack_from("<H", node, 72)
                name = self.name(node, 76, length, node[2] & 0x20).casefold()
                require(name not in children, "duplicate subkey")
                children[name] = child
        return children

    def fields(self, offset, schema):
        key = self.cell(offset, 76, b"nk")
        count, index = struct.unpack_from("<II", key, 36)
        result = {}
        if count:
            items = self.cell(index, count * 4)
            wanted = {name.casefold(): name for name in schema}
            for (pointer,) in struct.iter_unpack("<I", items[:count*4]):
                val = self.cell(pointer, 20, b"vk")
                length, size, address, kind, flags = struct.unpack_from("<HIIIH", val, 2)
                name = wanted.get(self.name(val, 20, length, flags & 1).casefold())
                if name is None:
                    continue
                require(name not in result and kind == schema[name], f"duplicate or mistyped {name}")
                if size & 0x80000000:
                    size &= 0x7fffffff
                    require(size <= 4, "inline value size")
                    data = val[8:8+size]
                else:
                    data = self.cell(address, size)[:size] if size else b""
                if kind == 4:  # REG_DWORD
                    require(size == 4, name)
                    result[name], = struct.unpack("<I", data)
                else:  # REG_SZ
                    value = bytes(data).decode("utf-16-le").removesuffix("\0")
                    require("\0" not in value, f"embedded NUL in {name}")
                    result[name] = value
        return result


def binary_game_config(stream):
    # Packages seen so far use two 4 KiB blocks. Cap input before allocating,
    # independently of the hive's own size and cell counts.
    limit = 1 << 20
    data = stream.read(limit + 1)
    require(len(data) <= limit, "game config exceeds 1 MiB")
    hive = ConfigHive(data)
    root = hive.fields(hive.root, {"configVersion": 4, "Name": 1, "Version": 1, "Publisher": 1,
                                   "TitleId": 4, "StoreId": 1})
    require(root.get("configVersion") in (0, 1), "unsupported configVersion")
    keys = hive.subkeys(hive.root)
    require("executable" in keys, "missing Executable key")
    entries = []
    for key in hive.subkeys(keys["executable"]).values():
        entry = hive.fields(key, {"Name": 1, "Id": 1, "TargetDeviceFamily": 1})
        require(entry.get("Name"), "missing executable Name")
        entries.append(entry)
    require(entries, "empty Executable key")
    vm = (hive.fields(keys["virtualmachine"], {"SMTEnabled": 4, "XboxAnacondaTitleMemory": 1,
                                               "XboxLockhartTitleMemory": 1})
          if "virtualmachine" in keys else {})
    identity = {k: root[k] for k in ("Name", "Publisher", "Version") if k in root}
    result = {"config_version": root["configVersion"], "identity": identity,
              "executables": entries, "virtual_machine": vm}
    if "TitleId" in root:
        result["title_id"] = f"{root['TitleId']:08X}"
    if "StoreId" in root:
        result["store_id"] = root["StoreId"]
    return result
