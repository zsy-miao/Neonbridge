"""Instructions mémoire de l'interpréteur RDNA 2 : de vrais petits shaders de calcul sur 32 lignes."""
import pytest

from emu import rdna2_exec, rdna2_hlsl
from emu.rdna2_exec_ops import bits_float, float_bits
from emu.rdna2_ops import MNEMONICS
from test_rdna2_exec import ONE, TWO, V, const, program, vop1, vop2, vop3

OPCODES = {(family, name): opcode for (family, opcode), name in MNEMONICS.items()}
LITERAL, NULL, OFF = 255, 125, 0x7D
INPUT, OUTPUT, TABLE, BUFFER = 0x10000, 0x20000, 0x30000, 0x40000


def smem(name, sdata, sbase, offset=0, soffset=NULL):
    return 0x3D << 26 | OPCODES["SMEM", name] << 18 | sdata << 6 | sbase // 2, soffset << 25 | offset & 0x1FFFFF


def global_(name, addr, data=0, vdst=0, saddr=OFF, offset=0, glc=0):
    low = 0x37 << 26 | OPCODES["GLOBAL", name] << 18 | glc << 16 | 2 << 14 | offset & 0xFFF
    return low, vdst << 24 | saddr << 16 | data << 8 | addr


def ds(name, addr, data0=0, vdst=0, offset0=0, offset1=0, data1=0):
    low = 0x36 << 26 | OPCODES["DS", name] << 18 | offset1 << 8 | offset0
    return low, vdst << 24 | data1 << 16 | data0 << 8 | addr


def mubuf(name, vaddr, vdata, srsrc, offen=0, offset=0, soffset=128):
    op = OPCODES["MUBUF", name]
    low = 0x38 << 26 | (op >> 7) << 25 | (op & 0x7F) << 18 | offen << 12 | offset
    return low, soffset << 24 | (srsrc // 4) << 16 | vdata << 8 | vaddr


def set_pair(wave, first, address):
    wave.s[first], wave.s[first + 1] = address & 0xFFFF_FFFF, address >> 32


def set_descriptor(wave, first, base, records, stride=0):
    wave.s[first:first + 4] = [base & 0xFFFF_FFFF, base >> 32 | stride << 16, records, 0]


def test_compute_shader_reads_scales_and_writes_an_array():
    wave = rdna2_exec.Wave()
    wave.memory.write_words(INPUT, [float_bits(float(i)) for i in range(32)])
    set_pair(wave, 0, INPUT)
    set_pair(wave, 2, OUTPUT)
    rdna2_exec.run(program(
        vop2("v_lshlrev_b32", 1, const(2), 0),                       # v1 = ligne × 4
        *global_("global_load_dword", addr=1, vdst=2, saddr=0),      # v2 = entrée[ligne]
        *vop3("v_fma_f32", 3, V + 2, TWO, ONE),                      # v3 = v2 × 2 + 1
        *global_("global_store_dword", addr=1, data=3, saddr=2)),    # sortie[ligne] = v3
        wave)
    assert [bits_float(x) for x in wave.memory.read_words(OUTPUT, 32)] == [2.0 * i + 1 for i in range(32)]


def test_scalar_loads_and_buffer_bounds():
    wave = rdna2_exec.Wave()
    wave.memory.write_words(TABLE, [7, 11, 13])
    set_pair(wave, 4, TABLE)
    set_descriptor(wave, 12, TABLE, records=8)                       # buffer de 8 octets
    rdna2_exec.run(program(
        *smem("s_load_dwordx2", 8, 4),
        *smem("s_load_dword", 10, 4, offset=8),
        *smem("s_buffer_load_dword", 16, 12, offset=4),
        *smem("s_buffer_load_dword", 17, 12, offset=8)),             # hors du buffer : 0
        wave)
    assert wave.s[8:11] == [7, 11, 13] and wave.s[16:18] == [11, 0]


def test_lds_rotation_atomic_counter_and_bpermute():
    wave = rdna2_exec.run(program(
        vop2("v_lshlrev_b32", 1, const(2), 0),                       # v1 = ligne × 4
        *ds("ds_write_b32", addr=1, data0=0),                        # lds[ligne] = ligne
        vop2("v_add_nc_u32", 3, const(1), 0),
        vop2("v_and_b32", 3, const(31), 3),
        vop2("v_lshlrev_b32", 3, const(2), 3),                       # v3 = ((ligne + 1) % 32) × 4
        *ds("ds_read_b32", addr=3, vdst=2),                          # v2 = ligne suivante
        vop1("v_mov_b32", 4, LITERAL), 256,
        vop1("v_mov_b32", 6, const(1)),
        *ds("ds_add_rtn_u32", addr=4, data0=6, vdst=5),              # compteur partagé
        *ds("ds_bpermute_b32", addr=3, data0=0, vdst=7)))            # v7 = v0 de la ligne suivante
    assert wave.v[2] == [(lane + 1) % 32 for lane in range(32)]
    assert int.from_bytes(wave.lds[256:260], "little") == 32 and wave.v[5] == list(range(32))
    assert wave.v[7] == wave.v[2]


def test_buffer_load_and_store_respect_the_descriptor():
    wave = rdna2_exec.Wave()
    wave.memory.write_words(BUFFER, [10, 20, 30, 40, 50])
    set_descriptor(wave, 20, BUFFER, records=16)                     # 4 mots accessibles
    rdna2_exec.run(program(
        vop2("v_lshlrev_b32", 1, const(2), 0),
        *mubuf("buffer_load_dword", 1, 8, 20, offen=1),
        *mubuf("buffer_store_dword", 1, 0, 20, offen=1)),            # écrit la ligne, si dans le buffer
        wave)
    assert wave.v[8][:6] == [10, 20, 30, 40, 0, 0]
    assert wave.memory.read_words(BUFFER, 5) == [0, 1, 2, 3, 50]


def test_global_atomic_add_returns_previous_values():
    wave = rdna2_exec.Wave()
    wave.memory.write_words(OUTPUT, [100])
    set_pair(wave, 0, OUTPUT)
    rdna2_exec.run(program(
        vop1("v_mov_b32", 9, const(1)),
        vop1("v_mov_b32", 10, const(0)),
        *global_("global_atomic_add", addr=10, data=9, vdst=11, saddr=0, glc=1)), wave)
    assert wave.memory.read_words(OUTPUT, 1) == [132]
    assert wave.v[11] == [100 + lane for lane in range(32)]


def lds_two_words(first, second, *body):
    """v1 = 0 (adresse), lds[0] = first, lds[4] = second, puis `body`."""
    return program(
        vop1("v_mov_b32", 1, const(0)),
        vop1("v_mov_b32", 2, LITERAL), first,
        vop1("v_mov_b32", 3, LITERAL), second,
        *ds("ds_write_b32", addr=1, data0=2),
        *ds("ds_write_b32", addr=1, data0=3, offset0=4),
        *body)


def test_lds_d16_reads_fill_one_half_and_keep_the_other():
    # Octets de la LDS : 42 7F 81 80. v4 vaut 0xCAFE1357 avant chaque lecture.
    expected = {"ds_read_u8_d16": (1, 0xCAFE007F), "ds_read_u8_d16_hi": (3, 0x00801357),
                "ds_read_i8_d16": (3, 0xCAFEFF80), "ds_read_i8_d16_hi": (3, 0xFF801357),
                "ds_read_u16_d16": (2, 0xCAFE8081), "ds_read_u16_d16_hi": (0, 0x7F421357)}
    for name, (offset, value) in expected.items():
        wave = rdna2_exec.run(lds_two_words(
            0x80817F42, 0,
            vop1("v_mov_b32", 4, LITERAL), 0xCAFE1357,
            *ds(name, addr=1, vdst=4, offset0=offset)))
        assert wave.v[4] == [value] * 32, name
    # Demi-mot à cheval sur deux mots (octet 3) : valeur mesurée sur GPU, publiée dans la
    # pull request 571 de VIctory (mots 0xAB123456 et 0xDEADCDBE, destination 0xCAFE1357).
    wave = rdna2_exec.run(lds_two_words(
        0xAB123456, 0xDEADCDBE,
        vop1("v_mov_b32", 4, LITERAL), 0xCAFE1357,
        *ds("ds_read_u16_d16_hi", addr=1, vdst=4, offset0=3)))
    assert wave.v[4] == [0xBEAB1357] * 32


def test_lds_d16_hi_writes_take_the_high_half_of_the_data():
    wave = rdna2_exec.run(lds_two_words(
        0xFFFFFFFF, 0xFFFFFFFF,
        vop1("v_mov_b32", 5, LITERAL), 0x11AB22CD,
        *ds("ds_write_b8_d16_hi", addr=1, data0=5, offset0=1),       # octet 1 = AB
        *ds("ds_write_b16_d16_hi", addr=1, data0=5, offset0=4)))     # octets 4 et 5 = 11AB
    assert wave.lds[:8] == bytes.fromhex("ffabffff ab11ffff")


def test_memory_instructions_raise_coverage():
    done, total = rdna2_exec.coverage()
    assert 500 <= done < total


def flat(name, addr, vdst, offset):
    """FLAT segment (SEG = 0): no SADDR; `offset` goes in bits 11:0 as given."""
    return 0x37 << 26 | OPCODES["FLAT", name] << 18 | offset & 0xFFF, vdst << 24 | OFF << 16 | addr


def flat_wave(*words):
    wave = rdna2_exec.Wave()
    wave.memory.write_words(INPUT, list(range(0x1000)))
    for lane in range(32):
        wave.v[1][lane], wave.v[2][lane] = INPUT + 0x800, 0
    rdna2_exec.run(program(*words), wave)
    return wave


def test_flat_offset_is_unsigned_and_bit_11_is_refused():
    """ISA section 9.1 (p. 83): "Scratch, Global: 12-bit signed byte offset. Flat: must be positive"."""
    wave = flat_wave(*flat("flat_load_dword", 1, 3, 0x7FC))             # + 2044, not a negative offset
    assert wave.v[3][0] == (0x800 + 0x7FC) // 4
    with pytest.raises(rdna2_exec.Untranslated, match="bit 11"):
        flat_wave(*flat("flat_load_dword", 1, 3, 0x800))
    with pytest.raises(rdna2_hlsl.Untranslatable, match="bit 11"):
        rdna2_hlsl.translate(program(*flat("flat_load_dword", 1, 3, 0x800)))
    wave = flat_wave(*global_("global_load_dword", 1, vdst=3, offset=-4))  # GLOBAL stays signed
    assert wave.v[3][0] == 0x800 // 4 - 1
