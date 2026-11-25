"""Recompilateur RDNA 2 → HLSL pour les programmes wave64, sur des GPU dont les waves ont 32 lignes (#70).

Un groupe de 64 threads est le wave virtuel : la ligne n est le thread `SV_GroupIndex` n. Les masques de
lignes (EXEC = s[126] et s[127], VCC = s[106] et s[107], résultats de comparaison et retenues dans une paire de
SGPR) sont construits en mémoire `groupshared` entre barrières de groupe (`Ballot64`), et les lectures d'une
autre ligne passent par un tableau `groupshared` de 64 entrées (`ReadLane64`). Rien ne suppose quelle ligne
tombe dans quel wave matériel : seules les garanties de D3D12 sur un groupe servent (docs/research/wave64.md).

Les instructions dont le sens dépend de la taille du wave sont traduites ici ; les autres sont déléguées à
emu/rdna2_hlsl.py avec la condition d'activité `LANE_ON`. Une instruction vectorielle déléguée dont le texte
lit encore VCC ou EXEC bit à bit, appelle `Ballot` ou `ReadLane` ou utilise `lane` est refusée en la nommant. La traduction est vérifiée
contre l'interpréteur de référence en wave64 (`rdna2_exec.Wave(lanes=64)`).
"""
import re

from emu import (rdna2, rdna2_hlsl, rdna2_hlsl_div, rdna2_hlsl_dot, rdna2_hlsl_ext, rdna2_hlsl_f16, rdna2_hlsl_format,
                 rdna2_hlsl_image, rdna2_hlsl_sad, rdna2_hlsl_wide)
from emu.rdna2_exec_cmp16 import check_vop3
from emu.rdna2_exec_memory import ADD_TID, LDS_ADD_TID
from emu.rdna2_exec_vop3 import Fields, float_inputs
from emu.rdna2_hlsl_memory import memory_statements
from emu.rdna2_hlsl import CARRY, SCALAR_FAMILIES, _fields, _literal, _words, modified
from emu.rdna2_hlsl_ops import (BRANCHES, HELPERS as ALU_HELPERS, NULL, Untranslatable, compare_condition,
                                is_compare, scalar_target, source, source_high)

ACTIVE = "LANE_ON"
VCC, EXEC = 106, 126
COMPARE_END = 0x100                     # opcodes VOP3 des comparaisons
BRANCHES64 = {
    **BRANCHES,
    "s_cbranch_vccz": "(s[106] | s[107]) == 0u", "s_cbranch_vccnz": "(s[106] | s[107]) != 0u",
    "s_cbranch_execz": "(s[126] | s[127]) == 0u", "s_cbranch_execnz": "(s[126] | s[127]) != 0u",
}
# Texte délégué qui dépendrait encore d'un wave de 32 lignes : VCC ou EXEC lu bit à bit, échange entre
# lignes ou numéro de ligne. Lire VCC ou EXEC comme une valeur (v_mov_b32 v1, vcc_lo) reste permis.
WAVE32_ONLY = re.compile(r"s\[1(?:06|07|26|27)\]\s*>>|\bBallot\(|\bReadLane\(|\blane\b")

PRELUDE = """// Généré par AnyXboxX (emu/rdna2_hlsl_wave64.py) : programme RDNA 2 wave64 recompilé en HLSL.
// Wave virtuel de 64 lignes = un groupe de 64 threads (ligne n = SV_GroupIndex n), quelle que soit la
// taille des waves du GPU : masques et lectures de ligne passent par la mémoire groupshared.
#define END_PC 0xFFFFFFFFu
#define LANE_ON Bit64(s[126], s[127], lane)
RWByteAddressBuffer g_memory : register(u0);
groupshared uint g_lds[8128];      // D3D12 : 32 Kio de mémoire partagée par groupe au plus
// Un seul tableau d'échange pour Ballot64 (mots 0 et 1) et ReadLane64 (64 mots) : 8128 + 64 = 8192 mots.
// Chaque appel commence par une barrière, donc les lectures de l'appel précédent sont finies.
groupshared uint g_exchange[64];

bool Bit64(uint low, uint high, uint lane) {
  return (((lane < 32u ? low : high) >> (lane & 31u)) & 1u) != 0u;
}

uint2 Ballot64(uint lane, bool bit) {
  GroupMemoryBarrierWithGroupSync();
  if (lane < 2u) g_exchange[lane] = 0u;
  GroupMemoryBarrierWithGroupSync();
  if (bit) InterlockedOr(g_exchange[lane >> 5], 1u << (lane & 31u));
  GroupMemoryBarrierWithGroupSync();
  return uint2(g_exchange[0], g_exchange[1]);
}

uint ReadLane64(uint lane, uint value, uint other) {
  GroupMemoryBarrierWithGroupSync();
  g_exchange[lane] = value;
  GroupMemoryBarrierWithGroupSync();
  return g_exchange[other & 63u];
}

uint FirstLane64(uint low, uint high) {     // plus basse ligne active, 0 si EXEC est nul
  return low != 0u ? firstbitlow(low) : (high != 0u ? 32u + firstbitlow(high) : 0u);
}

[numthreads(64, 1, 1)]
void main(uint lane : SV_GroupIndex) {
  uint s[128];
  uint v[256];
  uint s_null = 0u;
  bool scc = false;
  for (uint i = 0; i < 128; ++i) s[i] = 0u;
  for (uint j = 0; j < 256; ++j) v[j] = 0u;
  v[0] = lane;
  s[126] = 0xFFFFFFFFu;
  s[127] = 0xFFFFFFFFu;
  // La mémoire groupshared n'est pas remise à zéro par D3D12 : on la vide, comme l'interpréteur.
  for (uint w = lane; w < 8128u; w += 64u) g_lds[w] = 0u;
  GroupMemoryBarrierWithGroupSync();
"""


def _pair(code):
    """Deux SGPR d'un masque de lignes en destination (NULL : écriture ignorée)."""
    if code == NULL:
        return "s_null", "s_null"
    return scalar_target(code), scalar_target(code + 1)


def _ballot(code, condition):
    low, high = _pair(code)
    return f"{{ bool t = {condition}; uint2 m = Ballot64(lane, t); {low} = m.x; {high} = m.y; }}"


def _bit(code):
    """Bit de la ligne dans un masque de 64 bits lu dans une paire de SGPR."""
    return f"Bit64({source(code)}, {source_high(code)}, lane)"


def _compare(name, a, b, code):
    target = EXEC if name.startswith("v_cmpx") else code
    return [_ballot(target, f"({ACTIVE}) && ({compare_condition(name, a, b)})")]


def _carry(name, a, b, carry_in, vdst, carry_out):
    result, carry = CARRY[name]
    k = f"({_bit(carry_in)} ? 1u : 0u)" if name.endswith("_ci_u32") else "0u"
    low, high = _pair(carry_out)
    return [f"{{ uint x = {a}, y = {b}, k = {k}, r = 0u; bool o = false; "
            f"if ({ACTIVE}) {{ r = {result}; o = {carry}; v[{vdst}] = r; }} "
            f"uint2 m = Ballot64(lane, o); {low} = m.x; {high} = m.y; }}"]


def _mbcnt(name, a, b, vdst):
    """Bits de S0 sous la ligne : mot bas (mbcnt_lo) ou mot haut (mbcnt_hi) de (1 << ligne) - 1."""
    if name == "v_mbcnt_lo_u32_b32":
        below = "(lane < 32u ? (1u << lane) - 1u : 0xFFFFFFFFu)"
    else:
        below = "(lane < 32u ? 0u : (1u << (lane - 32u)) - 1u)"
    return [f"if ({ACTIVE}) v[{vdst}] = countbits({a} & {below}) + {b};"]


def _mad64(name, a, b, fields):
    code = fields.sources[2]
    if code == 255:
        raise Untranslatable(f"non traduite en HLSL : {name} avec littéral 64 bits")
    if name == "v_mad_i64_i32" and fields.sdst != NULL:
        raise Untranslatable("v_mad_i64_i32 : retenue signée dans SDST non vérifiée")
    high_product = "MulHiI32(x, y)" if name == "v_mad_i64_i32" else "MulHiU32(x, y)"
    low, high = _pair(fields.sdst)
    return [f"{{ uint x = {a}, y = {b}; uint pl = x * y, ph = {high_product}; uint rl = pl + {source(code)}; "
            f"uint rh = ph + {source_high(code)}; bool o = rh < ph; uint rh2 = rh + (rl < pl ? 1u : 0u); "
            f"o = o || rh2 < rh; if ({ACTIVE}) {{ v[{fields.vdst}] = rl; v[{fields.vdst + 1}] = rh2; }} "
            f"uint2 m = Ballot64(lane, ({ACTIVE}) && o); {low} = m.x; {high} = m.y; }}"]


def _vector(name, fields, literal):
    """VOP1/VOP2/VOPC sans SDWA ni DPP ; None si l'instruction ne dépend pas de la taille du wave."""
    a = source(fields["src0"], literal)
    b = f"v[{fields['vsrc1']}]" if "vsrc1" in fields else "0u"
    if name == "v_readfirstlane_b32":
        return [f"{scalar_target(fields['vdst'])} = ReadLane64(lane, {a}, FirstLane64(s[126], s[127]));"]
    if is_compare(name):
        return _compare(name, a, b, VCC)
    if name in CARRY:
        return _carry(name, a, b, VCC, fields["vdst"], VCC)
    if name == "v_cndmask_b32":
        return [f"if ({ACTIVE}) v[{fields['vdst']}] = {_bit(VCC)} ? {b} : {a};"]
    return None


def _vop3(name, opcode, fields, literal):
    """VOP3 dépendant de la taille du wave ; None pour les autres."""
    a, b, _ = (modified(code, i, fields, literal, float_inputs(name)) for i, code in enumerate(fields.sources))
    if opcode < COMPARE_END and is_compare(name):
        check_vop3(name, fields.op_sel, Untranslatable)
        return _compare(name, a, b, fields.vdst)
    if name in CARRY:
        return _carry(name, a, b, fields.sources[2], fields.vdst, fields.sdst)
    if name in ("v_mad_u64_u32", "v_mad_i64_i32"):
        return _mad64(name, a, b, fields)
    if name == "v_readlane_b32":
        return [f"{scalar_target(fields.vdst)} = ReadLane64(lane, {a}, {b});"]
    if name == "v_readfirstlane_b32":
        return [f"{scalar_target(fields.vdst)} = ReadLane64(lane, {a}, FirstLane64(s[126], s[127]));"]
    if name == "v_writelane_b32":
        return [f"if (lane == ({b} & 63u)) v[{fields.vdst}] = {a};"]
    if name in ("v_mbcnt_lo_u32_b32", "v_mbcnt_hi_u32_b32"):
        return _mbcnt(name, a, b, fields.vdst)
    if name == "v_cndmask_b32":
        if (fields.abs | fields.neg) >> 2 & 1:
            raise Untranslatable("non traduite en HLSL (wave64) : v_cndmask_b32 avec modificateur sur le masque")
        return [f"if ({ACTIVE}) v[{fields.vdst}] = {_bit(fields.sources[2])} ? {b} : {a};"]
    return None


def _bpermute(word):
    """ds_bpermute_b32 : chaque moitié de 32 lignes permute seule (GFX10 wave64, voir rdna2_exec_memory)."""
    offset = (word >> 8 & 0xFF) << 8 | word & 0xFF
    addr, data0, vdst = (word >> 32) & 0xFF, (word >> 40) & 0xFF, (word >> 56) & 0xFF
    other = f"(lane & 32u) | (((v[{addr}] + {offset}u) >> 2) & 31u)"
    return [f"{{ uint value = ReadLane64(lane, v[{data0}], {other}); if ({ACTIVE}) v[{vdst}] = value; }}"]


def _specific(data, instruction, name):
    family, (first, pair) = instruction.family, _words(data, instruction)
    if family in ("VOP1", "VOP2", "VOPC"):
        fields = _fields(family, first)
        if rdna2_hlsl_ext.is_ext(family, fields.get("src0")):
            return None
        return _vector(name, fields, _literal(data, instruction, 1))
    if family == "VOP3":
        return _vop3(name, instruction.opcode, Fields(pair), _literal(data, instruction, 2))
    if family == "DS" and name == "ds_bpermute_b32":
        return _bpermute(pair)
    if family == "GLOBAL" and name in ADD_TID or family == "DS" and name in LDS_ADD_TID:
        return memory_statements(family, name, pair, ACTIVE)    # lane = SV_GroupIndex 0-63: the TID in the wave64
    return None


def statements(data, instruction, finish="return;", special=None):
    """Instructions HLSL d'une instruction RDNA 2 d'un programme wave64 (hors branchements)."""
    name = rdna2.mnemonic(instruction.family, instruction.opcode) or f"{instruction.family}#{instruction.opcode}"
    lines = _specific(data, instruction, name)
    if lines is not None:
        return lines
    lines = rdna2_hlsl.statements(data, instruction, finish, special, active=ACTIVE)
    if instruction.family not in SCALAR_FAMILIES and any(WAVE32_ONLY.search(line) for line in lines):
        raise Untranslatable(f"non traduite en HLSL (wave64) : {name} ({instruction.family})")
    return lines


def translate(data, sgprs=None):
    """Texte HLSL d'un programme RDNA 2 wave64 ; `sgprs` : valeurs initiales {numéro: valeur}."""
    helpers = "\n".join((ALU_HELPERS, rdna2_hlsl_wide.HELPERS, rdna2_hlsl_div.HELPERS, rdna2_hlsl_image.HELPERS,
                         rdna2_hlsl_format.HELPERS, rdna2_hlsl_dot.HELPERS,
                         rdna2_hlsl_sad.HELPERS, rdna2_hlsl_f16.HELPERS))
    prelude = PRELUDE.replace("[numthreads", helpers + "\n[numthreads", 1)
    lines = rdna2_hlsl.body(data, sgprs, branches=BRANCHES64, translate_one=statements)
    return "\n".join([prelude.rstrip("\n"), *lines, "}", ""])


__all__ = ["translate", "statements", "Untranslatable"]
