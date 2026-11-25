"""Independent graph model for Route Game's fixed replay benchmark."""
from itertools import islice, cycle

# Row-major walkable cells on a seven-column board. IDs 8 and 26 are S and E.
CELLS = {8, 9, 10, 11, 12, 15, 17, 19, 22, 23, 24, 25, 26}
DELTAS = (-1, 1, -7, 7)
COINS = {10: 1, 22: 2}
MASK32 = 0xFFFFFFFF
MASK64 = 0xFFFFFFFFFFFFFFFF


def directions():
    word = 0x54555842
    while True:
        word ^= (word << 13) & MASK32
        word ^= word >> 17
        word ^= (word << 5) & MASK32
        yield DELTAS[word >> 30]


def replay(moves):
    cell, coins, blocked = 8, 0, 0
    for delta in moves:
        if cell + delta in CELLS:
            cell += delta
            coins |= COINS.get(cell, 0)
        else:
            blocked += 1
    return (cell % 7 | (cell // 7) << 3 | coins << 6 |
            blocked << 8 | int(cell == 26) << 16)


def reference():
    stream = directions()
    bank = [list(islice(stream, 128)) for _ in range(256)]
    digest = 0xCBF29CE484222325
    for lap in range(256):
        for index, route in enumerate(bank):
            offset = (lap + index) % len(route)
            state = replay(islice(cycle(route), offset, offset + len(route)))
            digest = ((digest ^ state) * 0x100000001B3) & MASK64
    return digest


if __name__ == "__main__":
    print(f"routes=65536 moves=8388608 checksum={reference():016x}")
