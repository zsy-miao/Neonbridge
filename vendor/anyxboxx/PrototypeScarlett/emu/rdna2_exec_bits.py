"""Instructions scalaires RDNA 2 de manipulation de bits, de champs et de masques d'EXEC.

Complète la table SCALAR d'emu/rdna2_exec_ops.py : s_bfe_*, s_lshl{1-4}_add_u32, s_mul_hi_*,
s_absdiff_i32, s_pack_*_b32_b16, opérations 64 bits, s_wqm/s_quadmask/s_bitreplicate, s_bitset*, s_ff0/s_flbit
signés et les variantes saveexec/wrexec restantes. Sémantique : documentation publique d'AMD
« RDNA 2 Instruction Set Architecture », chapitre 5 (SOP2, SOPC, SOP1).

Les fabriques (_sop1, _sop2…) sont reçues en paramètre pour éviter un import circulaire avec
emu/rdna2_exec_ops.py, qui les définit.
"""
MASK32, MASK64 = 0xFFFF_FFFF, (1 << 64) - 1
NO_BIT = MASK32                              # « aucun bit trouvé » : -1 sur 32 bits
WIDTH_SHIFT, WIDTH_MASK = 16, 0x7F           # largeur d'un s_bfe : S1[22:16]
QUAD = 4
HALF = 0xFFFF                                # one 16-bit half of a dword
LITERAL = 255


def _signed(value, bits):
    value &= (1 << bits) - 1
    return value - (1 << bits) if value >> (bits - 1) else value


def bfe(value, control, bits, is_signed):
    """Champ de S0 : décalage S1[4:0] (S1[5:0] en 64 bits), largeur S1[22:16] (jusqu'à 127)."""
    offset = control & (bits - 1)
    width = control >> WIDTH_SHIFT & WIDTH_MASK
    if width == 0:
        return 0
    if not is_signed:
        return (value >> offset) & ((1 << width) - 1)
    shifted = _signed(value, bits) >> offset
    return shifted if width >= bits else _signed(shifted, width)


def wqm(value, bits):
    """Whole quad mode : chaque groupe de 4 bits devient 0xF s'il a un bit posé."""
    return sum(0xF << q for q in range(0, bits, QUAD) if value >> q & 0xF)


def quadmask(value, bits):
    """Bit i du résultat : un des bits 4i..4i+3 de S0 est posé."""
    return sum(1 << (q // QUAD) for q in range(0, bits, QUAD) if value >> q & 0xF)


def bitreplicate(value):
    """Chaque bit i de S0 (32 bits) recopié dans les bits 2i et 2i+1 du résultat (64 bits)."""
    return sum(3 << (2 * i) for i in range(32) if value >> i & 1)


def first_one(value):
    """Indice du bit posé le plus bas, -1 s'il n'y en a pas."""
    return (value & -value).bit_length() - 1 if value else NO_BIT


def leading_zeros(value, bits):
    """Nombre de zéros avant le premier bit posé en partant du haut, -1 si S0 = 0."""
    return bits - value.bit_length() if value else NO_BIT


def leading_signs(value, bits):
    """Position (depuis le haut) du premier bit différent du bit de signe, -1 pour 0 et -1."""
    mask = (1 << bits) - 1
    return leading_zeros(~value & mask if value >> (bits - 1) & 1 else value, bits)


def absdiff(a, b):
    """|S0 - S1| calculé sur 32 bits signés, comme le pseudo-code d'AMD."""
    return abs(_signed(a - b, 32)) & MASK32


def lshl_add(shift):
    """(S0 << n) + S1 ; SCC = retenue au-delà de 32 bits (décalage compris)."""
    return lambda a, b, w: ((a << shift) + b, (a << shift) + b >> 32 != 0)


def bitset(bit, width=1):
    """D[S0[4:0]] (D[S0[5:0]] en 64 bits) = bit ; D est lu puis réécrit, SCC inchangé."""
    bits = 32 * width

    def execute(wave, f, literal):
        index = wave.read_scalar(f["ssrc0"], literal) & (bits - 1)
        current = wave.read_scalar(f["sdst"], literal, width)
        wave.write_scalar(f["sdst"], current & ~(1 << index) | bit << index, width)
    return execute


def _nonzero(function, mask=MASK32):
    return lambda a, w: (function(a) & mask, function(a) & mask != 0)


def _value(function):
    return lambda a, w: (function(a), None)


def signed_literal_s0(execute):
    """S0 is a signed 64-bit integer: a 32-bit literal there is sign-extended, not zero-extended.

    AMD "RDNA 2" ISA (November 2020), section 6.2.1, "Literal Expansion to 64 bits" (p. 43): "64-bit
    unsigned integer: zero extended to 64 bits; 64-bit signed integer: sign extended to 64 bits". The
    unsigned and bitwise 64-bit sources (s_bfe_u64, s_*_b64…) keep zero-extending. S1 of these
    instructions is 32 bits and only its low 23 bits are read, so sharing the literal with it is harmless.
    """
    def run(wave, f, literal):
        if f["ssrc0"] == LITERAL and literal is not None:
            literal = _signed(literal, 32) & MASK64
        return execute(wave, f, literal)
    return run


def scalar_operations(sop1, sop2, logic, saveexec, compare):
    """{mnémonique: exécution} des instructions scalaires de ce module."""
    table = {
        "s_bfe_u32": sop2(lambda a, b, w: (r := bfe(a, b, 32, False), r != 0)),
        "s_bfe_i32": sop2(lambda a, b, w: (r := bfe(a, b, 32, True), r != 0)),
        "s_bfe_u64": sop2(lambda a, b, w: (r := bfe(a, b, 64, False), r != 0), 2, 1),
        "s_bfe_i64": signed_literal_s0(sop2(lambda a, b, w: (r := bfe(a, b, 64, True), r != 0), 2, 1)),
        "s_bfm_b64": sop2(lambda a, b, w: (((1 << (a & 63)) - 1) << (b & 63), None), 2, 1, 1),
        "s_ashr_i64": signed_literal_s0(sop2(lambda a, b, w: (r := _signed(a, 64) >> (b & 63), r != 0), 2, 1)),
        "s_absdiff_i32": sop2(lambda a, b, w: (r := absdiff(a, b), r != 0)),
        "s_mul_hi_u32": sop2(lambda a, b, w: (a * b >> 32, None)),
        "s_mul_hi_i32": sop2(lambda a, b, w: (_signed(a, 32) * _signed(b, 32) >> 32, None)),
        # D = {S1 half, S0 half}, SCC unchanged (ISA section 12.1, SOP2 opcodes 50-52, p. 106). An inline
        # float source is its float32 pattern, as for every SOP2 source: LLVM 20.1.8 declares the packs
        # SOP2_32, and on gfx1036 16-bit integer VALU sources read the float32 bits (emu/rdna2_exec_cmp16.py).
        # Assumed for s_pack (not measured directly: PortBridge issue #83).
        "s_pack_ll_b32_b16": sop2(lambda a, b, w: ((b & HALF) << 16 | a & HALF, None)),
        "s_pack_lh_b32_b16": sop2(lambda a, b, w: (b & ~HALF & MASK32 | a & HALF, None)),
        "s_pack_hh_b32_b16": sop2(lambda a, b, w: (b & ~HALF & MASK32 | a >> 16, None)),
        "s_orn2_b64": logic(lambda a, b: a | ~b, 2), "s_nand_b64": logic(lambda a, b: ~(a & b), 2),
        "s_nor_b64": logic(lambda a, b: ~(a | b), 2), "s_xnor_b64": logic(lambda a, b: ~(a ^ b), 2),
        "s_bitcmp0_b64": compare(lambda a, b: not a >> (b & 63) & 1, 2, second_width=1),
        "s_bitcmp1_b64": compare(lambda a, b: a >> (b & 63) & 1, 2, second_width=1),
        "s_cmov_b64": sop1(lambda a, w: (a if w.scc else None, None), 2),
        "s_brev_b64": sop1(_value(lambda a: int(f"{a:064b}"[::-1], 2)), 2),
        "s_bcnt0_i32_b32": sop1(_nonzero(lambda a: 32 - bin(a).count("1"))),
        "s_ff0_i32_b32": sop1(_value(lambda a: first_one(~a & MASK32))),
        "s_flbit_i32": sop1(_value(lambda a: leading_signs(a, 32))),
        "s_wqm_b32": sop1(_nonzero(lambda a: wqm(a, 32))),
        "s_wqm_b64": sop1(_nonzero(lambda a: wqm(a, 64), MASK64), 2),
        "s_quadmask_b32": sop1(_nonzero(lambda a: quadmask(a, 32))),
        "s_quadmask_b64": sop1(_nonzero(lambda a: quadmask(a, 64), MASK64), 2),
        "s_bitset0_b32": bitset(0), "s_bitset1_b32": bitset(1),
        "s_bitset0_b64": bitset(0, 2), "s_bitset1_b64": bitset(1, 2),
    }
    table.update({f"s_lshl{n}_add_u32": sop2(lshl_add(n)) for n in range(1, 5)})
    table.update(_wide_source_operations(sop1))
    table.update(_exec_operations(saveexec))
    return table


def _wide_source_operations(sop1):
    """Source 64 bits, résultat 32 bits : la fabrique lit S0 en 64 bits, on écrit D en 32."""
    def from64(function, scc=False):
        def execute(wave, f, literal):
            result = function(wave.read_scalar(f["ssrc0"], literal, 2)) & MASK32
            wave.write_scalar(f["sdst"], result)
            if scc:
                wave.scc = result != 0
        return execute

    def to64(function):
        def execute(wave, f, literal):
            wave.write_scalar(f["sdst"], function(wave.read_scalar(f["ssrc0"], literal)), 2)
        return execute

    return {
        "s_bcnt0_i32_b64": from64(lambda a: 64 - bin(a).count("1"), scc=True),
        "s_bcnt1_i32_b64": from64(lambda a: bin(a).count("1"), scc=True),
        "s_ff0_i32_b64": from64(lambda a: first_one(~a & MASK64)),
        "s_ff1_i32_b64": from64(first_one),
        "s_flbit_i32_b64": from64(lambda a: leading_zeros(a, 64)),
        "s_flbit_i32_i64": signed_literal_s0(from64(lambda a: leading_signs(a, 64))),
        "s_bitreplicate_b64_b32": to64(bitreplicate),
    }


SAVEEXEC = {
    "and": lambda s, e: s & e, "or": lambda s, e: s | e, "xor": lambda s, e: s ^ e,
    "andn2": lambda s, e: s & ~e, "orn2": lambda s, e: s | ~e, "nand": lambda s, e: ~(s & e),
    "nor": lambda s, e: ~(s | e), "xnor": lambda s, e: ~(s ^ e), "andn1": lambda s, e: ~s & e,
    "orn1": lambda s, e: ~s | e,
}
WREXEC = {"andn1": SAVEEXEC["andn1"], "andn2": SAVEEXEC["andn2"]}


def _exec_operations(saveexec):
    table = {}
    for name, combine in SAVEEXEC.items():
        table[f"s_{name}_saveexec_b32"] = saveexec(combine)
        table[f"s_{name}_saveexec_b64"] = saveexec(combine, 2)
    for name, combine in WREXEC.items():
        table[f"s_{name}_wrexec_b32"] = saveexec(combine, writes_new=True)
        table[f"s_{name}_wrexec_b64"] = saveexec(combine, 2, writes_new=True)
    return table
