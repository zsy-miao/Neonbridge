"""Images RDNA 2 (MIMG) : champs de l'instruction, descripteurs T# (image) et S# (échantillonneur).

Positions des champs et constantes :
- Descripteurs T# (`SQ_IMG_RSRC_WORD*`, dont `BASE_LEVEL`/`LAST_LEVEL` bits 12..19 de WORD3) et
  S# (`SQ_IMG_SAMP_WORD*`, constantes `SQ_TEX_CLAMP_*`, `SQ_TEX_XY_FILTER_*`,
  `SQ_TEX_DEPTH_COMPARE_*`, `SQ_TEX_BORDER_COLOR_*`) : décodage de Leonida,
  runtime/src/gfx10_descriptors.h (GPL-3.0, d'après Mesa src/amd/registers/gfx10-rsrc.json, MIT).
- Encodage MIMG gfx10 : LLVM SIInstrFormats.td (MIMGe_gfx10, Apache 2.0).
- Noms des formats : data/gfx10_formats.json (Leonida).
- Ordre des registres d'adresse MIMG (`[offset], [z_ref], s, t, [lod]`) et décalages signés sur
  6 bits (`off_x = bits[5:0]`, `off_y = bits[13:8]`) : AMD RDNA 2 ISA, Table 8.9.
- Sens de comparaison profondeur (`z_ref <op> texel`) : AMD RDNA 2 ISA / D3D12 `SampleCmp`.
- Arrondi UNORM8 en écriture (`floor(v * 255 + 0.5)`) : D3D12 Functional Spec §3.2.3.6.
- Ordre des 4 texels de `image_gather4` (`(x0, y1), (x1, y1), (x1, y0), (x0, y0)`) :
  AMD RDNA 2 ISA / D3D12 `Gather` / Vulkan `OpImageGather`.

Opérations prises en charge (le reste est refusé en le nommant) : image 2D linéaire à un seul
niveau mip (BASE_LEVEL = LAST_LEVEL = 0, lignes de width × taille du texel), formats
8_UNORM, 8_8_UNORM, 8_8_8_8_UNORM, 32_FLOAT, 32_32_FLOAT et 32_32_32_32_FLOAT ; lecture/écriture
directe (image_load[_mip], image_store[_mip], image_get_resinfo) ; échantillonnage
(image_sample[_lz|_l][_o], image_sample_c[_lz|_l][_o], image_gather4[_lz|_l][_o]) au plus proche
ou bilinéaire, adressage wrap, mirror, clamp-to-edge, mirror-once-to-edge, clamp-to-border et
mirror-once-to-border (couleurs de bordure standard). L'interpréteur et le HLSL font le même
calcul, au bit près.
"""
import json
import struct
from pathlib import Path

DWORD = 4
FORMAT_NAMES = {int(code): name for code, name in
                json.loads((Path(__file__).parent / "data" / "gfx10_formats.json").read_text(encoding="utf-8")).items()}
FORMAT_8_UNORM = 1
FORMAT_8_8_UNORM = 14
FORMAT_32_FLOAT = 22
FORMAT_8_8_8_8_UNORM = 56
FORMAT_32_32_FLOAT = 64
FORMAT_32_32_32_32_FLOAT = 77
TEXEL_BYTES = {
    FORMAT_8_UNORM: 1,
    FORMAT_8_8_UNORM: 2,
    FORMAT_32_FLOAT: 4,
    FORMAT_8_8_8_8_UNORM: 4,
    FORMAT_32_32_FLOAT: 8,
    FORMAT_32_32_32_32_FLOAT: 16,
}
TYPE_2D = 9
DIM_2D = 1
CLAMP_WRAP = 0
CLAMP_MIRROR = 1
CLAMP_LAST_TEXEL = 2
CLAMP_MIRROR_ONCE_LAST_TEXEL = 3
CLAMP_BORDER = 6
CLAMP_MIRROR_ONCE_BORDER = 7
SUPPORTED_CLAMPS = frozenset({CLAMP_WRAP, CLAMP_MIRROR, CLAMP_LAST_TEXEL, CLAMP_MIRROR_ONCE_LAST_TEXEL,
                              CLAMP_BORDER, CLAMP_MIRROR_ONCE_BORDER})
FILTER_POINT, FILTER_LINEAR = 0, 1
SUPPORTED_FILTERS = frozenset({FILTER_POINT, FILTER_LINEAR})
BORDER_TRANS_BLACK, BORDER_OPAQUE_BLACK, BORDER_OPAQUE_WHITE = 0, 1, 2
SUPPORTED_BORDERS = frozenset({BORDER_TRANS_BLACK, BORDER_OPAQUE_BLACK, BORDER_OPAQUE_WHITE})
COMPARE_NEVER, COMPARE_LESS, COMPARE_EQUAL, COMPARE_LEQUAL = 0, 1, 2, 3
COMPARE_GREATER, COMPARE_NOTEQUAL, COMPARE_GEQUAL, COMPARE_ALWAYS = 4, 5, 6, 7
SELECT_ZERO, SELECT_ONE, SELECT_X = 0, 1, 4
ONE_BITS = 0x3F800000
# float32 exact de c / 255 (conversion UNORM 8 bits), table partagée avec le HLSL.
UNORM8 = tuple(struct.unpack("<I", struct.pack("<f", value / 255.0))[0] for value in range(256))
DIRECT_OPS = frozenset({"image_load", "image_load_mip", "image_store", "image_store_mip", "image_get_resinfo"})
SAMPLE_OPS = frozenset({
    "image_sample", "image_sample_lz", "image_sample_l",
    "image_sample_o", "image_sample_lz_o", "image_sample_l_o",
})
COMPARE_OPS = frozenset({
    "image_sample_c", "image_sample_c_lz", "image_sample_c_l",
    "image_sample_c_o", "image_sample_c_lz_o", "image_sample_c_l_o",
})
GATHER_OPS = frozenset({
    "image_gather4", "image_gather4_lz", "image_gather4_l",
    "image_gather4_o", "image_gather4_lz_o", "image_gather4_l_o",
})
SAMPLER_OPS = SAMPLE_OPS | COMPARE_OPS | GATHER_OPS
SUPPORTED_OPS = DIRECT_OPS | SAMPLER_OPS


class ImageError(ValueError):
    """Instruction ou descripteur d'image non pris en charge."""


def instruction_fields(word):
    """Champs MIMG gfx10 ; `word` : l'instruction entière (64 bits + mot NSA éventuel)."""
    nsa = (word >> 1) & 0x3
    addresses = [(word >> 32) & 0xFF] + [(word >> (64 + 8 * k)) & 0xFF for k in range(4 * nsa)]
    return {"nsa": nsa, "dim": (word >> 3) & 0x7, "dmask": (word >> 8) & 0xF, "unorm": (word >> 12) & 1,
            "r128": (word >> 15) & 1, "tfe": (word >> 16) & 1, "lwe": (word >> 17) & 1,
            "vaddr": addresses, "vdata": (word >> 40) & 0xFF, "srsrc": ((word >> 48) & 0x1F) * 4,
            "ssamp": ((word >> 53) & 0x1F) * 4, "a16": (word >> 62) & 1, "d16": (word >> 63) & 1}


def check_instruction(name, fields):
    if name not in SUPPORTED_OPS:
        raise ImageError(f"instruction d'image non prise en charge : {name}")
    if fields["dim"] != DIM_2D:
        raise ImageError(f"{name} : dimension {fields['dim']} non prise en charge (2D seulement)")
    for flag in ("r128", "tfe", "lwe", "a16", "d16"):
        if fields[flag]:
            raise ImageError(f"{name} : option {flag} non prise en charge")
    if name in GATHER_OPS and bin(fields["dmask"]).count("1") != 1:
        raise ImageError(f"{name} : dmask 0x{fields['dmask']:x} non pris en charge (un seul canal)")


def has_offset(name):
    return "o" in name.split("_")[2:]


def has_lod(name):
    return "l" in name.split("_")[2:]


def address_count(name):
    """Nombre de VGPR d'adresse attendus en 2D (Table 8.9 RDNA 2)."""
    if name == "image_get_resinfo":
        return 1
    if name in ("image_load_mip", "image_store_mip"):
        return 3
    if name in DIRECT_OPS:
        return 2
    return 2 + int(has_offset(name)) + int(name in COMPARE_OPS) + int(has_lod(name))


def address_registers(name, fields):
    """VGPR d'adresse de l'instruction : consécutifs, ou donnés par le mot NSA."""
    count = address_count(name)
    if fields["nsa"]:
        return tuple(fields["vaddr"][:count])
    base = fields["vaddr"][0]
    return tuple(base + i for i in range(count))


def parse_sampler_addrs(name, addrs):
    """Décompose les VGPR d'adresse d'un sample/gather 2D : (offset_reg, z_reg, s_reg, t_reg, lod_reg)."""
    idx = 0
    off_reg = addrs[idx] if has_offset(name) else None
    idx += int(off_reg is not None)
    z_reg = addrs[idx] if name in COMPARE_OPS else None
    idx += int(z_reg is not None)
    s_reg, t_reg = addrs[idx], addrs[idx + 1]
    idx += 2
    lod_reg = addrs[idx] if has_lod(name) else None
    return off_reg, z_reg, s_reg, t_reg, lod_reg


def coordinate_registers(fields):
    """VGPR de s et de t : consécutifs, ou donnés par le mot NSA."""
    return address_registers("image_sample", fields)


def gather_channel(dmask):
    """Canal unique (0..3) sélectionné par le dmask d'un image_gather4."""
    for channel in range(4):
        if dmask >> channel & 1:
            return channel
    return 0


def _get(word, low, width):
    return (word >> low) & ((1 << width) - 1)


def image(words):
    """T# (8 mots) → dictionnaire ; ImageError si le descripteur sort du cas pris en charge."""
    kind = _get(words[3], 28, 4)
    fmt = _get(words[1], 20, 9)
    base_level = _get(words[3], 12, 4)
    last_level = _get(words[3], 16, 4)
    found = {"address": ((_get(words[1], 0, 8) << 32 | words[0]) << 8), "format": fmt, "type": kind,
             "width": (_get(words[1], 30, 2) | _get(words[2], 0, 12) << 2) + 1,
             "height": _get(words[2], 14, 14) + 1,
             "swizzle": [_get(words[3], 3 * i, 3) for i in range(4)],
             "base_level": base_level, "last_level": last_level}
    if kind != TYPE_2D:
        raise ImageError(f"T# de type {kind} non pris en charge (2D = {TYPE_2D})")
    if fmt not in TEXEL_BYTES:
        raise ImageError(f"format {FORMAT_NAMES.get(fmt, fmt)} non pris en charge")
    if base_level != 0 or last_level != 0:
        raise ImageError(
            f"T# : niveaux mip non pris en charge (base_level={base_level}, last_level={last_level}, niveau 0 seulement)"
        )
    return found


def sampler(words):
    """S# (4 mots) → dictionnaire ; ImageError hors du cas pris en charge."""
    found = {"clamp_x": _get(words[0], 0, 3), "clamp_y": _get(words[0], 3, 3),
             "depth_compare": _get(words[0], 12, 3), "unnormalized": _get(words[0], 15, 1),
             "mag_filter": _get(words[2], 20, 2), "min_filter": _get(words[2], 22, 2),
             "border_color": _get(words[3], 30, 2)}
    for axis in ("clamp_x", "clamp_y"):
        if found[axis] not in SUPPORTED_CLAMPS:
            raise ImageError(f"S# : adressage {found[axis]} non pris en charge")
    if found["mag_filter"] not in SUPPORTED_FILTERS or found["min_filter"] not in SUPPORTED_FILTERS:
        raise ImageError("S# : filtrage anisotrope non pris en charge (au plus proche ou bilinéaire seulement)")
    if found["border_color"] not in SUPPORTED_BORDERS:
        raise ImageError(f"S# : couleur de bordure {found['border_color']} non prise en charge (standard seulement)")
    return found


def encode_image(address, fmt, width, height, swizzle=(4, 5, 6, 7), base_level=0, last_level=0):
    """T# d'une image 2D (pour les tests et les outils) ; inverse de image()."""
    units = address >> 8
    words = [units & 0xFFFFFFFF, (units >> 32) & 0xFF | fmt << 20 | ((width - 1) & 0x3) << 30,
             (width - 1) >> 2 | (height - 1) << 14,
             sum(sel << (3 * i) for i, sel in enumerate(swizzle))
             | (base_level & 0xF) << 12 | (last_level & 0xF) << 16 | TYPE_2D << 28,
             0, 0, 0, 0]
    return words


def encode_sampler(clamp_x=CLAMP_LAST_TEXEL, clamp_y=CLAMP_LAST_TEXEL, unnormalized=0,
                   mag_filter=FILTER_POINT, min_filter=FILTER_POINT, depth_compare=COMPARE_NEVER,
                   border_color=BORDER_TRANS_BLACK):
    return [clamp_x | clamp_y << 3 | depth_compare << 12 | unnormalized << 15,
            0, mag_filter << 20 | min_filter << 22, border_color << 30]
