"""Shaders graphiques RDNA 2 (vertex et pixel) : conventions communes à l'interpréteur et au HLSL.

Modèle : chaque invocation (un sommet, un pixel) est un wave d'une seule ligne (EXEC = 1).
- Vertex shader : v0 = numéro du sommet. Ses exports (EXP pos0, param0-7) sont enregistrés
  dans la mémoire du programme (VERTEX_RECORDS) ; pos0 devient SV_Position.
- Pixel shader : v0 = i, v1 = j (barycentriques perspective, centre du pixel), comme le
  « persp center » d'un PS console. v_interp_p1/p2/mov lisent P0, P10 = P1 - P0 et
  P20 = P2 - P0 de chaque attribut ; sur console ces valeurs viennent de la LDS, remplies
  depuis les exports param du VS ; ici elles sont dans PARAMETERS (une entrée par primitive).
  Les exports mrt0-7 deviennent SV_Target0-7 et sont enregistrés dans PIXEL_RECORDS avec
  les entrées (i, j, primitive) pour que l'interpréteur rejoue chaque pixel à l'identique.
Champs : AMD « RDNA 2 Instruction Set Architecture » (VINTRP, EXP) ; cibles : emu/rdna2_exp.py.
"""
import struct

from emu import rdna2_exp

DWORD = 4
CHANNELS = 4
PARAMETER_COUNT = 8                            # param0-param7
EXPORT_SLOTS = 1 + PARAMETER_COUNT             # pos0, puis param0-7
VERTEX_RECORDS = 0x50000
VERTEX_STRIDE = EXPORT_SLOTS * CHANNELS * DWORD
PARAMETERS = 0x54000
PARAMETER_STRIDE = 3 * DWORD                   # P0, P10, P20 d'un canal
PRIMITIVE_STRIDE = PARAMETER_COUNT * CHANNELS * PARAMETER_STRIDE
PIXEL_RECORDS = 0x58000
PIXEL_HEADER = 4                               # couvert, i, j, primitive
RENDER_TARGETS = 8
PIXEL_STRIDE = (PIXEL_HEADER + RENDER_TARGETS * CHANNELS) * DWORD
MAX_VERTICES, MAX_PIXELS = 64, 1024
MEMORY_SIZE = 0x80000
TARGET_MRT0, TARGET_POS0, TARGET_PARAM0 = 0, 12, 32
INTERP_OPS = {0: "v_interp_p1_f32", 1: "v_interp_p2_f32", 2: "v_interp_mov_f32"}
MOV_SOURCES = {0: "P10", 1: "P20", 2: "P0"}


def vintrp_fields(word):
    """vsrc (7-0), chan (9-8), attr (15-10), op (17-16), vdst (25-18)."""
    return {"vsrc": word & 0xFF, "chan": (word >> 8) & 0x3, "attr": (word >> 10) & 0x3F,
            "op": (word >> 16) & 0x3, "vdst": (word >> 18) & 0xFF}


def exp_fields(word):
    """en (3-0), cible (9-4), compr (10), done (11), vm (12), sources (mot 1)."""
    sources = [(word >> (32 + 8 * index)) & 0xFF for index in range(CHANNELS)]
    return {"en": word & 0xF, "target": (word >> 4) & 0x3F, "compr": (word >> 10) & 1,
            "sources": sources, "name": rdna2_exp.target_name((word >> 4) & 0x3F)}


def vertex_slot(target):
    """Emplacement d'enregistrement d'un export de VS, ou None (cible ignorée)."""
    if target == TARGET_POS0:
        return 0
    if TARGET_PARAM0 <= target < TARGET_PARAM0 + PARAMETER_COUNT:
        return 1 + target - TARGET_PARAM0
    return None


def parameter_address(primitive, attribute, channel):
    return PARAMETERS + primitive * PRIMITIVE_STRIDE + (attribute * CHANNELS + channel) * PARAMETER_STRIDE


def _f32(value):
    return struct.unpack("<f", struct.pack("<f", value))[0]


def parameters(vertex_records, primitives):
    """Octets de la zone PARAMETERS : P0, P10, P20 (float32) de chaque param de chaque triangle.

    `vertex_records` : octets de VERTEX_RECORDS (3 sommets par triangle, dans l'ordre)."""
    area = bytearray(primitives * PRIMITIVE_STRIDE)
    for primitive in range(primitives):
        for attribute in range(PARAMETER_COUNT):
            for channel in range(CHANNELS):
                offset = ((1 + attribute) * CHANNELS + channel) * DWORD
                p0, p1, p2 = (struct.unpack_from("<f", vertex_records, (3 * primitive + k) * VERTEX_STRIDE + offset)[0]
                              for k in range(3))
                position = parameter_address(primitive, attribute, channel) - PARAMETERS
                struct.pack_into("<3f", area, position, p0, _f32(p1 - p0), _f32(p2 - p0))
    return bytes(area)
