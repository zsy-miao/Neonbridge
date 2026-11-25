"""Opérandes des instructions RDNA 2 : champs, registres et constantes, au format de LLVM.

Couvre les encodages scalaires (SOP1, SOP2, SOPC, SOPK, SOPP), vectoriels (VOP1, VOP2, VOPC,
VOP3), les accès mémoire scalaires (SMEM), la mémoire partagée (DS), les exports (EXP,
rdna2_exp), les tampons et FLAT/GLOBAL/SCRATCH (rdna2_operands_vmem) et les images (MIMG,
rdna2_operands_mimg). La forme de chaque opcode (quels champs, dans quel ordre, quelle
largeur de registre) vient de rdna2_shapes.SHAPES, apprise sur les tests de LLVM.
"""
import re
import struct

from emu import (rdna2, rdna2_exp, rdna2_operands_ext, rdna2_operands_mimg, rdna2_operands_vmem as vmem,
                 rdna2_operands_vop3p, rdna2_swizzle)

LITERAL = 255
INLINE_FLOATS = {240: "0.5", 241: "-0.5", 242: "1.0", 243: "-1.0", 244: "2.0", 245: "-2.0",
                 246: "4.0", 247: "-4.0", 248: "0.15915494"}
NAMED = {124: "m0", 125: "null", 235: "src_shared_base", 236: "src_shared_limit",
         237: "src_private_base", 238: "src_private_limit", 239: "src_pops_exiting_wave_id",
         251: "src_vccz", 252: "src_execz", 253: "src_scc", 254: "src_lds_direct"}
PAIRS = {106: ("vcc_lo", "vcc"), 126: ("exec_lo", "exec")}
HIGH_HALVES = {107: "vcc_hi", 127: "exec_hi"}
SGPR_LAST, TTMP_FIRST, TTMP_LAST, VGPR_FIRST = 105, 108, 123, 256

# Champs de chaque famille : nom → (décalage, masque, genre).
FIELDS = {
    "SOP2": {"sdst": (16, 0x7F, "src"), "ssrc0": (0, 0xFF, "src"), "ssrc1": (8, 0xFF, "src")},
    "SOP1": {"sdst": (16, 0x7F, "src"), "ssrc0": (0, 0xFF, "src")},
    "SOPC": {"ssrc0": (0, 0xFF, "src"), "ssrc1": (8, 0xFF, "src")},
    "SOPK": {"sdst": (16, 0x7F, "src"), "simm16": (0, 0xFFFF, "imm")},
    "SOPP": {"simm16": (0, 0xFFFF, "imm")},
    # v_readfirstlane_b32 écrit un registre scalaire dans le champ vdst : « sdst » en est la lecture scalaire.
    "VOP1": {"vdst": (17, 0xFF, "vgpr"), "src0": (0, 0x1FF, "src"), "sdst": (17, 0xFF, "src")},
    "VOP2": {"vdst": (17, 0xFF, "vgpr"), "src0": (0, 0x1FF, "src"), "vsrc1": (9, 0xFF, "vgpr")},
    "VOPC": {"src0": (0, 0x1FF, "src"), "vsrc1": (9, 0xFF, "vgpr")},
    # VOP3 : deux mots lus comme un entier 64 bits. Genre « modN » : source N avec ses bits
    # abs (8 + N) et neg (61 + N). « sdst8 » : destination scalaire des comparaisons en VOP3.
    "VOP3": {"vdst": (0, 0xFF, "vgpr"), "src0": (32, 0x1FF, "mod0"), "src1": (41, 0x1FF, "mod1"),
             "src2": (50, 0x1FF, "mod2"), "sdst8": (0, 0xFF, "src")},
    # VOP3b (retenues, v_div_scale) : le champ abs porte une destination scalaire.
    "VOP3B": {"vdst": (0, 0xFF, "vgpr"), "sdst": (8, 0x7F, "src"), "src0": (32, 0x1FF, "neg0"),
              "src1": (41, 0x1FF, "neg1"), "src2": (50, 0x1FF, "neg2")},
    # SMEM (deux mots) : sbase désigne une paire de registres (numéro / 2), « offset » réunit
    # l'adresse immédiate signée sur 21 bits et soffset (bits 63-57). s_atc_probe lit sdata
    # comme un entier (« sdata_imm »).
    "SMEM": {"sdata": (6, 0x7F, "src"), "sbase": (0, 0x3F, "sbase"), "offset": (32, 0x1FFFFF, "smem_offset"),
             "sdata_imm": (6, 0x7F, "imm")},
    # DS (LDS/GDS, deux mots) : adresse, deux données et destination, toutes vectorielles.
    # offset (bits 15-0) ou offset0/offset1 (bits 7-0 et 15-8), puis gds (bit 17) : suffixes.
    "DS": {"addr": (32, 0xFF, "vgpr"), "data0": (40, 0xFF, "vgpr"), "data1": (48, 0xFF, "vgpr"),
           "vdst": (56, 0xFF, "vgpr")},
    **vmem.FIELDS,                       # MUBUF, MTBUF, FLAT, GLOBAL, SCRATCH (deux mots)
}
FAMILIES = frozenset(FIELDS) - {"VOP3B"}
# Familles au texte fixe, sans forme apprise : fonction (data, offset) → texte.
FIXED_FORMAT = {"EXP": rdna2_exp.text, "VINTRP": rdna2_exp.vintrp_text, "MIMG": rdna2_operands_mimg.text,
                "VOP3P": rdna2_operands_vop3p.text}
# Encodages vectoriels 32 bits : LLVM écrit « _e32 », sauf les formes à littéral obligatoire, v_pk_fmac_f16
# et les VOP1 sans forme VOP3 (tests gfx10_vop1.txt et gfx10_vop2.txt du désassembleur de LLVM).
E32_FAMILIES = frozenset({"VOP1", "VOP2", "VOPC"})
WITHOUT_E32 = frozenset({"v_madmk_f32", "v_madak_f32", "v_fmamk_f32", "v_fmaak_f32", "v_madmk_f16",
                         "v_madak_f16", "v_fmamk_f16", "v_fmaak_f16", "v_pk_fmac_f16",
                         "v_readfirstlane_b32", "v_swap_b32", "v_swaprel_b32", "v_clrexcp", "v_nop",
                         "v_pipeflush"})
SRC0_MASK = 0x1FF
TWO_WORD_FAMILIES = frozenset({"VOP3", "SMEM", "DS"}) | vmem.FAMILIES
DS_TWO_OFFSETS = re.compile(r"^ds_(?:read|write|wrxchg)2(?:st64)?_")
DS_SWIZZLE = "ds_swizzle_b32"
DS_GDS_BIT, BYTE, U16 = 17, 0xFF, 0xFFFF
SMEM_SOFFSET_SHIFT, SMEM_OFFSET_BITS = 57, 21
SMEM_FLAGS = ((16, "glc"), (14, "dlc"))                  # ordre d'écriture de LLVM
NULL_REGISTER = 125
VOP3_LIKE = frozenset({"VOP3", "VOP3B"})
VOP3B_MARK = ("@", "VOP3B")
OUTPUT_MODIFIERS = {1: "mul:2", 2: "mul:4", 3: "div:2"}
# Opcodes VOP3 qui encodent une instruction VOPC, VOP2, VOP1 ou VINTRP : LLVM ajoute « _e64 ».
E64_RANGES = ((0x000, 0x100), (0x100, 0x140), (0x180, 0x200), (0x200, 0x203))   # + v_interp_*
IMMEDIATE_FORMATS = ("auto", "dec", "sdec", "hex", "hwreg", "waitcnt")
INLINE_INTEGER_MAX = 64                  # LLVM écrit en décimal jusqu'à 64, en hexadécimal au-delà
HWREG_DEFAULT_SIZE = 32
WAITCNT_MAX = {"vmcnt": 0x3F, "expcnt": 0x7, "lgkmcnt": 0x3F}


def _range(prefix, first, width):
    return f"{prefix}{first}" if width == 1 else f"{prefix}[{first}:{first + width - 1}]"


def source(code, width=1, literal=None):
    """Texte d'un opérande source (ou destination scalaire) encodé sur 9 bits."""
    if code >= VGPR_FIRST:
        return _range("v", code - VGPR_FIRST, width)
    if code <= SGPR_LAST:
        return _range("s", code, width)
    if TTMP_FIRST <= code <= TTMP_LAST:
        return _range("ttmp", code - TTMP_FIRST, width)
    if code in PAIRS:
        return PAIRS[code][width > 1]
    if code in HIGH_HALVES:
        return HIGH_HALVES[code]
    if code in NAMED:
        return NAMED[code]
    if 128 <= code <= 192:
        return str(code - 128)
    if 193 <= code <= 208:
        return str(192 - code)
    if code in INLINE_FLOATS:
        return INLINE_FLOATS[code]
    if code == LITERAL:
        return "<littéral>" if literal is None else f"0x{literal:x}"
    return f"<réservé {code}>"


def hwreg(value):
    """simm16 de s_getreg/s_setreg : identifiant (bits 5-0), décalage (10-6), taille - 1 (15-11)."""
    from emu.rdna2_shapes import HWREG_NAMES

    ident, offset, size = value & 0x3F, (value >> 6) & 0x1F, ((value >> 11) & 0x1F) + 1
    name = HWREG_NAMES.get(ident, str(ident))
    if offset == 0 and size == HWREG_DEFAULT_SIZE:
        return f"hwreg({name})"
    return f"hwreg({name}, {offset}, {size})"


def waitcnt(value):
    """simm16 de s_waitcnt : vmcnt (bits 3-0 et 15-14), expcnt (6-4), lgkmcnt (13-8)."""
    counters = {"vmcnt": (value & 0xF) | ((value >> 14) & 0x3) << 4,
                "expcnt": (value >> 4) & 0x7, "lgkmcnt": (value >> 8) & 0x3F}
    shown = [f"{name}({count})" for name, count in counters.items() if count != WAITCNT_MAX[name]]
    return " ".join(shown) or f"0x{value:x}"


def immediate(value, style):
    if style == "auto":
        return str(value) if value <= INLINE_INTEGER_MAX else f"0x{value:x}"
    if style == "hwreg":
        return hwreg(value)
    if style == "waitcnt":
        return waitcnt(value)
    if style == "hex":
        return f"0x{value:x}"
    if style == "sdec" and value >= 0x8000:
        return str(value - 0x10000)
    return str(value)


def render(family, word, literal, element):
    """Texte d'un élément de forme : ("=", texte fixe) ou (champ, largeur ou style)."""
    name, option = element
    if name == "=":
        return option
    if name == "literal":
        return "<littéral>" if literal is None else f"0x{literal:x}"
    shift, mask, kind = FIELDS[family][name]
    value = (word >> shift) & mask
    if kind in vmem.KINDS:
        return vmem.render_kind(kind, family, word, value, option, source)
    if kind == "imm":
        return immediate(value, option)
    if kind == "vgpr":
        return _range("v", value, option)
    if kind == "sbase":
        return source(value * 2, option)
    if kind == "smem_offset":
        return smem_offset(word)
    operand = source(value, option, literal)
    if kind.startswith(("mod", "neg")):
        index = int(kind[-1])
        if kind.startswith("mod") and word >> (8 + index) & 1:
            operand = f"|{operand}|"
        if word >> (61 + index) & 1:
            operand = f"-{operand}"
    return operand


def vop3_suffix(family, word, shape):
    """Modificateurs écrits après les opérandes : op_sel, clamp, mul/div."""
    parts = []
    sources = sum(1 for name, _ in shape if name in ("src0", "src1", "src2"))
    op_sel = (word >> 11) & 0xF if family == "VOP3" else 0
    if op_sel:
        bits = [op_sel >> i & 1 for i in range(sources)] + [op_sel >> 3 & 1]
        parts.append(f"op_sel:[{','.join(map(str, bits))}]")
    if word >> 15 & 1:
        parts.append("clamp")
    if (word >> 59) & 3:
        parts.append(OUTPUT_MODIFIERS[(word >> 59) & 3])
    return " ".join(parts)


def _hex(value):
    return f"-0x{-value:x}" if value < 0 else f"0x{value:x}"


def smem_offset(word):
    """Décalage SMEM : « 0x64 » (soffset nul), « s7 » ou « s7 offset:0x64 »."""
    offset = (word >> 32) & ((1 << SMEM_OFFSET_BITS) - 1)
    if offset >> (SMEM_OFFSET_BITS - 1):
        offset -= 1 << SMEM_OFFSET_BITS
    soffset = (word >> SMEM_SOFFSET_SHIFT) & 0x7F
    if soffset == NULL_REGISTER:
        return _hex(offset)
    register = source(soffset)
    return f"{register} offset:{_hex(offset)}" if offset else register


def smem_suffix(word):
    return " ".join(name for bit, name in SMEM_FLAGS if word >> bit & 1)


def _ds_offsets(name, word):
    if DS_TWO_OFFSETS.match(name):
        pairs = (("offset0", word & BYTE), ("offset1", (word >> 8) & BYTE))
        return [f"{label}:{value}" for label, value in pairs if value]
    offset = word & U16
    if name == DS_SWIZZLE:
        offset = rdna2_swizzle.text(offset)
    return [f"offset:{offset}"] if offset else []


def ds_suffix(name, word):
    """offset:N, offset0:N offset1:N ou offset:swizzle(...), puis gds ; rien pour les valeurs nulles."""
    parts = _ds_offsets(name, word)
    if word >> DS_GDS_BIT & 1:
        parts.append("gds")
    return " ".join(parts)


ALIASES = frozenset({("VOP1", "sdst"), ("VOP3", "sdst8"), ("SMEM", "sdata_imm")})


def default_shape(family):
    return tuple((name, "dec" if kind == "imm" else 1) for name, (_, _, kind) in FIELDS[family].items()
                 if (family, name) not in ALIASES)


VCC_DESTINATION = ("=", "vcc_lo")


def fallback_shape(family, name, opcode=None):
    """Forme d'un opcode absent des tests de LLVM : les VOPC hors v_cmpx écrivent vcc (wave32) ;
    la forme VOP3 d'une VOP1 a une seule source (v_sat_pk_u8_i16_e64) ; un opcode _f64 prend la
    forme apprise de son jumeau _f32, registres vectoriels et sources en 64 bits (v_div_scale_f64)."""
    twin = _f32_twin_shape(family, name)
    if twin is not None:
        return tuple((field, 2) if field in WIDE_FIELDS and width == 1 else (field, width) for field, width in twin)
    if family == "VOP3" and opcode is not None and VOP1_IN_VOP3[0] <= opcode < VOP1_IN_VOP3[1]:
        return (("vdst", 1), ("src0", 1))
    shape = default_shape(family)
    if family == "VOPC" and not name.startswith("v_cmpx"):
        return (VCC_DESTINATION, *shape)
    return shape


VOP1_IN_VOP3 = (0x180, 0x200)
WIDE_FIELDS = frozenset({"vdst", "src0", "src1", "src2"})


def _f32_twin_shape(family, name):
    from emu.rdna2_ops import MNEMONICS
    from emu.rdna2_shapes import SHAPES

    if not name.endswith("_f64"):
        return None
    twin = name.removesuffix("_f64") + "_f32"
    return next((SHAPES[key] for key, found in MNEMONICS.items()
                 if key[0] == family and found == twin and key in SHAPES), None)


def vop3_name(name, opcode):
    return f"{name}_e64" if any(low <= opcode < high for low, high in E64_RANGES) else name


def _words(data, offset, family, size):
    """(mot de l'instruction, littéral) ; en VOP3 et SMEM le mot fait 64 bits (littéral après)."""
    count = 2 if family in TWO_WORD_FAMILIES else 1
    if len(data) < offset + count * rdna2.DWORD:
        return None, None
    word = int.from_bytes(data[offset:offset + count * rdna2.DWORD], "little")
    literal = None
    if size > count and len(data) >= offset + (count + 1) * rdna2.DWORD:
        literal = struct.unpack_from("<I", data, offset + count * rdna2.DWORD)[0]
    return word, literal


def text(data, offset=0):
    """Instruction désassemblée au format de LLVM, ou None si la famille n'est pas couverte."""
    from emu.rdna2_shapes import SHAPES

    family, opcode, size = rdna2.classify(data, offset)
    if family in FIXED_FORMAT:
        return FIXED_FORMAT[family](data, offset)
    name = rdna2.mnemonic(family, opcode)
    if family not in FAMILIES or name is None:
        return None
    word, literal = _words(data, offset, family, size)
    if word is None:
        return None
    shape = SHAPES[(family, opcode)] if (family, opcode) in SHAPES else fallback_shape(family, name, opcode)
    if family in E32_FAMILIES:
        if word & SRC0_MASK in rdna2.VOP_EXTENSIONS:       # SDWA, DPP : second mot de contrôle
            return rdna2_operands_ext.text(family, name, data, offset, shape)
        name = name if name in WITHOUT_E32 else f"{name}_e32"
    if shape[:1] == (VOP3B_MARK,):
        family, shape = "VOP3B", shape[1:]
    if family in vmem.FAMILIES:
        shape = vmem.adjust_shape(family, name, word, shape)
    operands = ", ".join(render(family, word, literal, element) for element in shape)
    if family in VOP3_LIKE:
        name = vop3_name(name, opcode)
        operands = f"{operands} {vop3_suffix(family, word, shape)}".strip()
    if family == "SMEM":
        operands = f"{operands} {smem_suffix(word)}".strip()
    if family == "DS":
        operands = f"{operands} {ds_suffix(name, word)}".strip()
    if family in vmem.FAMILIES:
        operands = f"{operands} {vmem.suffix(family, word)}".strip()
    return f"{name} {operands}".rstrip()
