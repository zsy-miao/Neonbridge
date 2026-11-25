"""Opérandes des accès mémoire vectoriels RDNA 2 au format de LLVM : tampons (MUBUF, MTBUF) et
FLAT, GLOBAL, SCRATCH. La forme de chaque opcode (quels champs, quelle largeur) est apprise par
tools/rdna2_shapes.py ; ce module rend les champs dépendants des bits de l'instruction et écrit
les modificateurs.

Encodages : MUBUFe_gfx10, MTBUFe_gfx10 (BUFInstructions.td) et FLAT_Real_gfx10
(FLATInstructions.td) ; syntaxe : getMUBUFAsmOps, getMTBUFAsmOps et AMDGPUInstPrinter
(printOffset, printFlatOffset, printCPol, printFormat), llvm/llvm-project, Apache 2.0 avec
exception LLVM. Noms des formats unifiés gfx10 : data/gfx10_formats.json (identiques à
UfmtSymbolicGFX10 de LLVM pour les identifiants 0 à 77).
"""
from emu import rdna2_image

VGPR_FIRST = 256
# Champs (décalage, masque, genre) des deux mots, lus comme un entier 64 bits.
BUFFER_FIELDS = {"vdata": (40, 0xFF, "buf_vdata"), "vaddr": (32, 0xFF, "buf_vaddr"),
                 "srsrc": (48, 0x1F, "rsrc"), "soffset": (56, 0xFF, "src")}
FLAT_FIELDS = {"vdst": (56, 0xFF, "vgpr"), "vaddr": (32, 0xFF, "flat_vaddr"),
               "vdata": (40, 0xFF, "vgpr"), "saddr": (48, 0x7F, "flat_saddr")}
FIELDS = {"MUBUF": BUFFER_FIELDS, "MTBUF": BUFFER_FIELDS,
          "FLAT": FLAT_FIELDS, "GLOBAL": FLAT_FIELDS, "SCRATCH": FLAT_FIELDS}
FAMILIES = frozenset(FIELDS)
BUFFER_FAMILIES = frozenset({"MUBUF", "MTBUF"})
KINDS = frozenset({"buf_vdata", "buf_vaddr", "rsrc", "flat_vaddr", "flat_saddr"})
RSRC_SCALE = 4                           # srsrc désigne un quadruplet de registres scalaires

# Tampons : offset (11-0), offen (12), idxen (13), glc (14), dlc (15), lds (16, MUBUF),
# format (25-19, MTBUF), slc (54), tfe (55).
BUFFER_OFFSET_MASK = 0xFFF
OFFEN_BIT, IDXEN_BIT, BUFFER_GLC_BIT, BUFFER_DLC_BIT, MUBUF_LDS_BIT = 12, 13, 14, 15, 16
BUFFER_SLC_BIT, TFE_BIT = 54, 55
FORMAT_SHIFT, FORMAT_MASK = 19, 0x7F
FORMAT_DEFAULT, FORMAT_LAST_NAMED = 1, 77          # UFMT_DEFAULT et UFMT_LAST (gfx10) de LLVM
FORMAT_PREFIX = "GFX10_FORMAT_"

# FLAT : offset (11-0, signé sur 12 bits pour GLOBAL et SCRATCH), dlc (12), lds (13), glc (16), slc (17).
FLAT_OFFSET_BITS = 12
FLAT_DLC_BIT, FLAT_LDS_BIT, FLAT_GLC_BIT, FLAT_SLC_BIT = 12, 13, 16, 17
SADDR_SHIFT, SADDR_MASK = 48, 0x7F
SADDR_NULL, SADDR_EXEC_HI = 125, 127               # « off » ; exec_hi : mode ST de SCRATCH
CMPSWAP = "cmpswap"


def _bit(word, index):
    return word >> index & 1


def _saddr_is_off(family, saddr):
    return saddr == SADDR_NULL or (family == "SCRATCH" and saddr == SADDR_EXEC_HI)


def _flat_vaddr(family, word, value, source):
    """FLAT : paire 64 bits ; GLOBAL : 32 bits avec saddr ; SCRATCH : « off » avec saddr ou en mode ST."""
    saddr = (word >> SADDR_SHIFT) & SADDR_MASK
    if family == "SCRATCH":
        return source(VGPR_FIRST + value) if saddr == SADDR_NULL else "off"
    width = 2 if family == "FLAT" or saddr == SADDR_NULL else 1
    return source(VGPR_FIRST + value, width)


def render_kind(kind, family, word, value, option, source):
    """Texte d'un champ dont la forme dépend d'autres bits de l'instruction."""
    if kind == "buf_vdata":                        # tfe : un registre de plus pour l'état
        return source(VGPR_FIRST + value, option + _bit(word, TFE_BIT))
    if kind == "buf_vaddr":                        # idxen et offen : un registre chacun
        width = _bit(word, IDXEN_BIT) + _bit(word, OFFEN_BIT)
        return source(VGPR_FIRST + value, width) if width else "off"
    if kind == "rsrc":
        return source(value * RSRC_SCALE, option)
    if kind == "flat_vaddr":
        return _flat_vaddr(family, word, value, source)
    if _saddr_is_off(family, value):               # flat_saddr
        return "off"
    return source(value, 2 if family == "GLOBAL" else 1)


def _without(shape, field):
    return tuple(element for element in shape if element[0] != field)


def _returned_value(shape, name):
    """vdst d'une atomique avec retour : la largeur des données, la moitié pour cmpswap."""
    width = next((option for field, option in shape if field == "vdata"), 1)
    return ("vdst", width // 2 if CMPSWAP in name and width > 1 else width)


def adjust_shape(family, name, word, shape):
    """Forme apprise → forme de cette instruction : lds sans données, atomiques avec ou sans retour."""
    if family == "MUBUF" and _bit(word, MUBUF_LDS_BIT):
        return _without(shape, "vdata")
    if family in BUFFER_FAMILIES:
        return shape
    if _bit(word, FLAT_LDS_BIT):
        return _without(shape, "vdst")
    if "_atomic_" in name:
        plain = _without(shape, "vdst")
        return (_returned_value(plain, name), *plain) if _bit(word, FLAT_GLC_BIT) else plain
    return shape


def base_shape(family, name, word, shape):
    """Inverse d'adjust_shape pour l'apprentissage : forme sans lds (un registre de données, comme
    tous les chargements lds) et atomiques sans retour, pour que toutes les lignes votent ensemble."""
    if family == "MUBUF" and _bit(word, MUBUF_LDS_BIT):
        return (("vdata", 1), *shape)
    if family in BUFFER_FAMILIES:
        return shape
    if _bit(word, FLAT_LDS_BIT):
        return (("vdst", 1), *shape)
    return _without(shape, "vdst") if "_atomic_" in name else shape


def segment_shape(segment, shape):
    """Forme d'un opcode FLAT, GLOBAL ou SCRATCH déduite d'un autre segment : seul saddr diffère."""
    plain = _without(shape, "saddr")
    return plain if segment == "FLAT" else (*plain, ("saddr", 1))


def format_text(value):
    """format:[BUF_FMT_...] (identifiant nommé), format:N, ou rien pour le format par défaut."""
    if value == FORMAT_DEFAULT:
        return None
    if value <= FORMAT_LAST_NAMED:
        return f"format:[BUF_FMT_{rdna2_image.FORMAT_NAMES[value].removeprefix(FORMAT_PREFIX)}]"
    return f"format:{value}"


def _cache_policy(word, glc, slc, dlc):
    return [name for bit, name in ((glc, "glc"), (slc, "slc"), (dlc, "dlc")) if _bit(word, bit)]


def _buffer_suffix(family, word):
    parts = []
    if family == "MTBUF":
        parts.append(format_text((word >> FORMAT_SHIFT) & FORMAT_MASK))
    parts += [name for bit, name in ((IDXEN_BIT, "idxen"), (OFFEN_BIT, "offen")) if _bit(word, bit)]
    offset = word & BUFFER_OFFSET_MASK
    parts.append(f"offset:{offset}" if offset else None)
    parts += _cache_policy(word, BUFFER_GLC_BIT, BUFFER_SLC_BIT, BUFFER_DLC_BIT)
    parts.append("lds" if family == "MUBUF" and _bit(word, MUBUF_LDS_BIT) else None)
    parts.append("tfe" if _bit(word, TFE_BIT) else None)
    return [part for part in parts if part]


def _flat_offset(family, word):
    offset = word & ((1 << FLAT_OFFSET_BITS) - 1)
    if family != "FLAT" and offset >> (FLAT_OFFSET_BITS - 1):
        offset -= 1 << FLAT_OFFSET_BITS
    return [f"offset:{offset}"] if offset else []


def suffix(family, word):
    """Modificateurs écrits après les opérandes, dans l'ordre de LLVM."""
    if family in BUFFER_FAMILIES:
        return " ".join(_buffer_suffix(family, word))
    parts = _flat_offset(family, word) + _cache_policy(word, FLAT_GLC_BIT, FLAT_SLC_BIT, FLAT_DLC_BIT)
    if _bit(word, FLAT_LDS_BIT):
        parts.append("lds")
    return " ".join(parts)
