"""Inputs and readback checks for Frosty's FTSRUpdateHistoryCS at global entry 3385."""
import math
import struct
from project_tuxbox.inspect import digest

ENTRY = 3385
SOURCE = "76fb3b539aeeab93ca3bc9a935df2d0497047164c6b7902dc51c309f79e3c086"
CASES = ("black", "current", "transparent", "overlay", "pattern")
SIZE = 32
SENTINEL = (-19.,) * 4


def gain(x, y):
    return 1 + max(0, x) // 4 + 2 * (max(0, y) // 4)


def pattern_data():
    pixels = bytearray()
    for y in range(SIZE):
        for x in range(SIZE):
            value = gain(x, y)
            pixels.extend(struct.pack("<4f", value, 2 * value, 4 * value, 1))
    return pixels


def constants(generate_mip):
    # Word offsets follow the shipping FTSRUpdateHistoryCS parameter metadata.
    data = bytearray(4096)
    def floats(offset, *values):
        struct.pack_into(f"<{len(values)}f", data, 4 * offset, *values)
    def integers(offset, *values):
        struct.pack_into(f"<{len(values)}I", data, 4 * offset, *values)
    # FScreenPassTextureViewportParameters for input, low frequencies,
    # rejection, history and the previous viewport.
    for base in (0, 28, 56, 84, 172):
        floats(base, SIZE, SIZE, 1/SIZE, 1/SIZE, .5, -.5, .5, .5)
        integers(base + 8, 0, 0, SIZE, SIZE)
        floats(base + 12, SIZE, SIZE, 1/SIZE, 1/SIZE, 0, 0, 1, 1, 1, 1, 1, 1,
               .5/SIZE, .5/SIZE, 1-.5/SIZE, 1-.5/SIZE)
    integers(112, 0, 0, SIZE-1, SIZE-1)
    integers(122, 1)  # bCameraCut selects the current frame.
    floats(156, 2/SIZE, -2/SIZE, -1+1/SIZE, 1-1/SIZE, 1, 1, 0, 0)
    floats(169, 1)
    integers(171, int(generate_mip))
    floats(200, .5, -.5, .5, .5, 1)
    return data


def prepare(trial):
    for case in CASES:
        path = trial / case
        path.mkdir()
        (path / "b0").write_bytes(constants(case in ("transparent", "overlay", "pattern")))
        (path / "b1").write_bytes(bytes(4096))
        for slot in range(17):
            color = (0, 0, 0, 0)
            if slot == 0 and case != "black":
                color = (1, 2, 4, 1)
            if slot == 2:
                color = (.125, .5, .25, .5) if case == "overlay" else (0, 0, 0, 1)
            data = struct.pack("<4f", *color) * (SIZE * SIZE)
            if case == "pattern" and slot == 0:
                data = pattern_data()
            (path / f"t{slot}").write_bytes(data)
        for slot in range(8):
            (path / f"u{slot}").write_bytes(struct.pack("<4f", *SENTINEL) * (SIZE * SIZE))
    (trial / "cases.txt").write_text("\n".join(CASES) + "\n")


def verify(trial):
    counts = {}
    for case in CASES:
        path = trial / case
        outputs = []
        images = []
        for slot in range(8):
            file = path / f"out{slot}"
            rows = list(struct.iter_unpack("<4f", file.read_bytes()))
            if len(rows) != SIZE * SIZE:
                raise ValueError(f"Wrong history output size for {case} u{slot}")
            images.append(rows)
            side = 16 if slot != 7 else (8 if case in ("transparent", "overlay", "pattern") else 0)
            for i, row in enumerate(rows):
                x, y = i % SIZE, i // SIZE
                if x >= side or y >= side:
                    ok = row == SENTINEL
                else:
                    ok = row != SENTINEL and all(map(math.isfinite, row))
                    if slot in (0, 6):
                        if case == "pattern":
                            lo, hi = gain(x-3, y-3), gain(x+3, y+3)
                            ok &= all(lo*.98 <= v/channel <= hi*1.02 for v, channel in zip(row, (1, 2, 4, 1)))
                            ok &= abs(row[1] - 2*row[0]) < .025 and abs(row[2] - 4*row[0]) < .05
                        else:
                            expected = (0,)*4 if case == "black" else (1, 2, 4, 1)
                            if case == "overlay" and slot == 6:
                                # Half of the current frame plus the premultiplied overlay.
                                expected = (.625, 1.5, 2.25, .625)
                            ok &= all(abs(v-e) <= .002*max(1,abs(e)) for v,e in zip(row,expected))
                    elif slot == 3:
                        expected = (.125, .5, .25, .125) if case == "overlay" else (0,)*4
                        ok &= all(abs(v-e) < .002 for v,e in zip(row,expected))
                    elif slot == 7:
                        for channel, value in enumerate(row):
                            mean = sum(images[6][SIZE*(2*y+dy) + 2*x+dx][channel]
                                       for dy in range(2) for dx in range(2)) / 4
                            ok &= abs(value-mean) <= .001*max(1, abs(mean))
                if not ok:
                    raise ValueError(f"Unexpected {case} u{slot} output at ({x},{y}): {row}")
            outputs.append(dict(sha256=digest(file), distinct_texels=len(set(rows)),
                                verified_texels=len(rows), guard_texels=len(rows)-side*side))
        counts[case] = dict(outputs=outputs)
    return counts
