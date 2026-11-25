"""Traduction RDNA 2 → HLSL, instruction par instruction (utilisé par emu/rdna2_hlsl.py).

Chaque ligne du wave32 devient un thread d'un groupe de 32. Registres : `s[128]` (SGPR, VCC
en s[106], EXEC en s[126]), `v[n]` (VGPR du thread), `scc`. Les instructions vectorielles ne
s'appliquent que si le bit de la ligne est posé dans EXEC (`active`). Les masques (v_cmp)
sont construits pour tout le groupe par Ballot() ; la sémantique suit l'interpréteur de
référence emu/rdna2_exec*, contre lequel la traduction est vérifiée.
"""
import re

from emu.rdna2_exec_ops import SOPP_NO_EFFECT, float_bits, signed
from emu.rdna2_hlsl_helpers import HELPERS  # noqa: F401  (réexporté : rdna2_hlsl, graphics, wave64)

NULL, VCC, EXEC, OFF = 125, 106, 126, 0x7D
INLINE_FLOATS = {240: 0.5, 241: -0.5, 242: 1.0, 243: -1.0, 244: 2.0, 245: -2.0, 246: 4.0, 247: -4.0,
                 248: 0.15915494309189535}


class Untranslatable(NotImplementedError):
    """Instruction que le recompilateur ne sait pas encore traduire en HLSL."""



def hexa(value):
    return f"0x{value & 0xFFFF_FFFF:X}u"


def source(code, literal=None):
    """Expression HLSL (uint) d'un opérande source 9 bits."""
    if code >= 256:
        return f"v[{code - 256}]"
    if code == NULL:
        return "0u"
    if code < 128:
        return f"s[{code}]"
    if 128 <= code <= 192:
        return f"{code - 128}u"
    if 193 <= code <= 208:
        return hexa(192 - code)
    if code in INLINE_FLOATS:
        return hexa(float_bits(INLINE_FLOATS[code]))
    if code == 255 and literal is not None:
        return hexa(literal)
    raise Untranslatable(f"opérande {code} non traduit")


def source_high(code):
    """Mot haut (uint) d'une source 64 bits : registre suivant, ou extension d'une constante entière."""
    if code >= 256:
        return f"v[{code - 255}]"
    if code < 128 and code != NULL:
        return f"s[{code + 1}]"
    if code == NULL or 128 <= code <= 192:
        return "0u"
    if 193 <= code <= 208:
        return "0xFFFFFFFFu"
    raise Untranslatable(f"opérande 64 bits {code} non traduit")


def scalar_target(code):
    return "s_null" if code == NULL else f"s[{code}]"


def f(expression):
    return f"asfloat({expression})"


# --- Scalaires : modèles avec {d} (destination), {a}, {b} (sources), {k} (immédiat) ----------
SCALAR = {
    "s_mov_b32": "{d} = {a};",
    "s_cmov_b32": "if (scc) {d} = {a};",
    "s_movk_i32": "{d} = {k};",
    "s_cmovk_i32": "if (scc) {d} = {k};",
    "s_addk_i32": "{{ uint x = {d}, y = {k}; uint r = x + y; scc = ((~(x ^ y) & (x ^ r)) >> 31) != 0; {d} = r; }}",
    "s_mulk_i32": "{d} = {d} * {k};",
    "s_cselect_b32": "{d} = scc ? {a} : {b};",
    "s_add_u32": "{{ uint x = {a}, y = {b}; uint r = x + y; scc = r < x; {d} = r; }}",
    "s_sub_u32": "{{ uint x = {a}, y = {b}; scc = x < y; {d} = x - y; }}",
    "s_addc_u32": "{{ uint x = {a}, y = {b}, k = scc ? 1u : 0u; uint r = x + y + k; scc = (r < x) || (k != 0u && r == x); {d} = r; }}",
    "s_subb_u32": "{{ uint x = {a}, y = {b}, k = scc ? 1u : 0u; scc = (x < y) || (k != 0u && x == y); {d} = x - y - k; }}",
    "s_add_i32": "{{ uint x = {a}, y = {b}; uint r = x + y; scc = ((~(x ^ y) & (x ^ r)) >> 31) != 0; {d} = r; }}",
    "s_sub_i32": "{{ uint x = {a}, y = {b}; uint r = x - y; scc = (((x ^ y) & (x ^ r)) >> 31) != 0; {d} = r; }}",
    "s_mul_i32": "{d} = {a} * {b};",
    "s_and_b32": "{{ uint r = {a} & {b}; scc = r != 0; {d} = r; }}",
    "s_or_b32": "{{ uint r = {a} | {b}; scc = r != 0; {d} = r; }}",
    "s_xor_b32": "{{ uint r = {a} ^ {b}; scc = r != 0; {d} = r; }}",
    "s_andn2_b32": "{{ uint r = {a} & ~{b}; scc = r != 0; {d} = r; }}",
    "s_orn2_b32": "{{ uint r = {a} | ~{b}; scc = r != 0; {d} = r; }}",
    "s_nand_b32": "{{ uint r = ~({a} & {b}); scc = r != 0; {d} = r; }}",
    "s_nor_b32": "{{ uint r = ~({a} | {b}); scc = r != 0; {d} = r; }}",
    "s_xnor_b32": "{{ uint r = ~({a} ^ {b}); scc = r != 0; {d} = r; }}",
    "s_lshl_b32": "{{ uint r = {a} << ({b} & 31u); scc = r != 0; {d} = r; }}",
    "s_lshr_b32": "{{ uint r = {a} >> ({b} & 31u); scc = r != 0; {d} = r; }}",
    "s_ashr_i32": "{{ uint r = asuint(asint({a}) >> ({b} & 31u)); scc = r != 0; {d} = r; }}",
    "s_bfm_b32": "{d} = ((1u << ({a} & 31u)) - 1u) << ({b} & 31u);",
    "s_min_u32": "{{ uint x = {a}, y = {b}; scc = x < y; {d} = min(x, y); }}",
    "s_max_u32": "{{ uint x = {a}, y = {b}; scc = x > y; {d} = max(x, y); }}",
    "s_min_i32": "{{ int x = asint({a}), y = asint({b}); scc = x < y; {d} = asuint(min(x, y)); }}",
    "s_max_i32": "{{ int x = asint({a}), y = asint({b}); scc = x > y; {d} = asuint(max(x, y)); }}",
    "s_not_b32": "{{ uint r = ~{a}; scc = r != 0; {d} = r; }}",
    "s_brev_b32": "{d} = reversebits({a});",
    "s_bcnt1_i32_b32": "{{ uint x = {a}; scc = x != 0u; {d} = countbits(x); }}",
    "s_ff1_i32_b32": "{d} = firstbitlow({a});",
    "s_flbit_i32_b32": "{{ uint x = {a}; {d} = x != 0u ? (31u - firstbithigh(x)) : 0xFFFFFFFFu; }}",
    "s_sext_i32_i8": "{d} = asuint((asint({a}) << 24) >> 24);",
    "s_sext_i32_i16": "{d} = asuint((asint({a}) << 16) >> 16);",
    "s_abs_i32": "{{ int x = asint({a}); scc = x != 0; {d} = asuint(x < 0 ? -x : x); }}",
    "s_bitcmp0_b32": "scc = (({a} >> ({b} & 31u)) & 1u) == 0u;",
    "s_bitcmp1_b32": "scc = (({a} >> ({b} & 31u)) & 1u) != 0u;",
    "s_bfe_u32": "{{ uint r = SBfeU32({a}, {b}); scc = r != 0u; {d} = r; }}",
    "s_bfe_i32": "{{ uint r = SBfeI32({a}, {b}); scc = r != 0u; {d} = r; }}",
    "s_absdiff_i32": "{{ uint x = {a} - {b}; uint r = asint(x) < 0 ? 0u - x : x; scc = r != 0u; {d} = r; }}",
    "s_mul_hi_u32": "{d} = MulHiU32({a}, {b});",
    "s_mul_hi_i32": "{d} = MulHiI32({a}, {b});",
    "s_pack_ll_b32_b16": "{d} = ({b} << 16) | ({a} & 0xFFFFu);",
    "s_pack_lh_b32_b16": "{d} = ({b} & 0xFFFF0000u) | ({a} & 0xFFFFu);",
    "s_pack_hh_b32_b16": "{d} = ({b} & 0xFFFF0000u) | ({a} >> 16);",
    "s_bcnt0_i32_b32": "{{ uint r = 32u - countbits({a}); scc = r != 0u; {d} = r; }}",
    "s_ff0_i32_b32": "{d} = firstbitlow(~{a});",
    "s_flbit_i32": "{d} = LeadingSignsI32({a});",
    "s_wqm_b32": "{{ uint r = WqmB32({a}); scc = r != 0u; {d} = r; }}",
    "s_quadmask_b32": "{{ uint r = QuadMaskB32({a}); scc = r != 0u; {d} = r; }}",
    "s_bitset0_b32": "{d} = {d} & ~(1u << ({a} & 31u));",
    "s_bitset1_b32": "{d} = {d} | (1u << ({a} & 31u));",
}
# (S0 << n) + S1 : SCC = retenue au-delà de 32 bits, bits sortis par le décalage compris.
for _n in range(1, 5):
    SCALAR[f"s_lshl{_n}_add_u32"] = (f"{{{{ uint x = {{a}}, t = x << {_n}u; uint r = t + {{b}}; "
                                     f"scc = (x >> {32 - _n}u) != 0u || r < t; {{d}} = r; }}}}")
# EXEC = combinaison de S0 et d'EXEC ; D = ancien EXEC (saveexec) ou nouveau (wrexec).
EXEC_COMBINE = {"and": "{a} & e", "or": "{a} | e", "xor": "{a} ^ e", "andn2": "{a} & ~e", "orn2": "{a} | ~e",
                "nand": "~({a} & e)", "nor": "~({a} | e)", "xnor": "~({a} ^ e)", "andn1": "~{a} & e",
                "orn1": "~{a} | e"}
for _op, _combine in EXEC_COMBINE.items():
    SCALAR[f"s_{_op}_saveexec_b32"] = f"{{{{ uint e = s[126]; s[126] = {_combine}; scc = s[126] != 0; {{d}} = e; }}}}"
for _op in ("andn1", "andn2"):
    SCALAR[f"s_{_op}_wrexec_b32"] = (f"{{{{ uint e = s[126]; s[126] = {EXEC_COMBINE[_op]}; scc = s[126] != 0; "
                                     f"{{d}} = s[126]; }}}}")
for _name in ("s_nop", "s_waitcnt", "s_waitcnt_vscnt", "s_waitcnt_vmcnt", "s_waitcnt_expcnt",
              "s_waitcnt_lgkmcnt", "s_barrier", "s_sleep", "s_setprio", "s_ttracedata",
              "s_dcache_inv", "s_gl1_inv", "buffer_gl0_inv", "buffer_gl1_inv", "s_clause",
              "s_inst_prefetch", *SOPP_NO_EFFECT):
    SCALAR[_name] = ""
COMPARE_OPERATORS = {"eq": "==", "lg": "!=", "ne": "!=", "neq": "!=", "gt": ">", "ge": ">=", "lt": "<", "le": "<="}
for _op, _symbol in COMPARE_OPERATORS.items():
    SCALAR[f"s_cmp_{_op}_u32"] = f"scc = {{a}} {_symbol} {{b}};"
    SCALAR[f"s_cmp_{_op}_i32"] = f"scc = asint({{a}}) {_symbol} asint({{b}});"
    SCALAR[f"s_cmpk_{_op}_u32"] = f"scc = {{d}} {_symbol} {{u}};"
    SCALAR[f"s_cmpk_{_op}_i32"] = f"scc = asint({{d}}) {_symbol} asint({{k}});"

BRANCHES = {
    "s_branch": "true", "s_cbranch_scc0": "!scc", "s_cbranch_scc1": "scc",
    "s_cbranch_vccz": "s[106] == 0", "s_cbranch_vccnz": "s[106] != 0",
    "s_cbranch_execz": "s[126] == 0", "s_cbranch_execnz": "s[126] != 0",
}


def scalar_statement(name, fields, literal):
    template = SCALAR[name]
    if not template:
        return ""
    values = {"d": scalar_target(fields.get("sdst", NULL))}
    for key, field in (("a", "ssrc0"), ("b", "ssrc1")):
        if field in fields:
            values[key] = source(fields[field], literal)
    if "simm16" in fields:
        values["k"] = hexa(signed(fields["simm16"], 16))
        values["u"] = hexa(fields["simm16"])
    return template.format(**values)


# --- Vectorielles : expressions uint de {a}, {b}, {c} (sources) et {d} (destination) --------
VECTOR = {
    "v_mov_b32": "{a}",
    "v_add_f32": "F32Add({a}, {b})",
    "v_sub_f32": "F32Sub({a}, {b})",
    "v_subrev_f32": "F32Sub({b}, {a})",
    "v_mul_f32": "F32Mul({a}, {b})",
    "v_mul_legacy_f32": "MulLegacyF32({a}, {b})",
    "v_min_f32": "asuint(min(asfloat({a}), asfloat({b})))",
    "v_max_f32": "asuint(max(asfloat({a}), asfloat({b})))",
    "v_fmac_f32": "FmaF32({a}, {b}, {d})",                 # arrondi unique : emu/rdna2_hlsl_fma.py
    "v_mac_f32": "asuint(mad(asfloat({a}), asfloat({b}), asfloat({d})))",
    "v_fma_f32": "FmaF32({a}, {b}, {c})",
    "v_mad_f32": "asuint(mad(asfloat({a}), asfloat({b}), asfloat({c})))",
    "v_fma_legacy_f32": "FmaLegacyF32({a}, {b}, {c})",
    "v_fmamk_f32": "FmaF32({a}, {k}, {b})",
    "v_fmaak_f32": "FmaF32({a}, {b}, {k})",
    "v_rcp_f32": "asuint(rcp(asfloat({a})))",
    "v_rsq_f32": "asuint(rsqrt(asfloat({a})))",
    "v_sqrt_f32": "asuint(sqrt(asfloat({a})))",
    "v_exp_f32": "asuint(exp2(asfloat({a})))",
    "v_log_f32": "asuint(log2(asfloat({a})))",
    "v_sin_f32": "asuint(sin(asfloat({a}) * 6.283185307179586f))",
    "v_cos_f32": "asuint(cos(asfloat({a}) * 6.283185307179586f))",
    "v_floor_f32": "asuint(floor(asfloat({a})))",
    "v_ceil_f32": "asuint(ceil(asfloat({a})))",
    "v_trunc_f32": "asuint(trunc(asfloat({a})))",
    "v_rndne_f32": "asuint(round(asfloat({a})))",
    # frac(-1e-20) rend 1.0 sur RTX 4060, 0x3F7FFFFF sur WARP et Intel UHD : borné comme rdna2_exec_ops._fract
    "v_fract_f32": "(frac(asfloat({a})) >= 1.0f ? 0x3F7FFFFFu : asuint(frac(asfloat({a}))))",
    "v_ldexp_f32": "asuint(ldexp(asfloat({a}), (float)asint({b})))",
    "v_cvt_f32_u32": "asuint((float){a})",
    "v_cvt_f32_i32": "asuint((float)asint({a}))",
    "v_cvt_u32_f32": "SatF32ToU32({a})",
    "v_cvt_i32_f32": "SatF32ToI32({a})",
    "v_cvt_f32_ubyte0": "asuint((float)({a} & 0xFFu))",
    "v_sat_pk_u8_i16": "((uint)clamp((int)(({a}) << 16u) >> 16, 0, 255) "
                       "| ((uint)clamp((int)({a}) >> 16, 0, 255) << 8u))",
    "v_cvt_f32_ubyte1": "asuint((float)(({a} >> 8) & 0xFFu))",
    "v_cvt_f32_ubyte2": "asuint((float)(({a} >> 16) & 0xFFu))",
    "v_cvt_f32_ubyte3": "asuint((float)(({a} >> 24) & 0xFFu))",
    "v_add_nc_u32": "({a} + {b})",
    "v_sub_nc_u32": "({a} - {b})",
    "v_subrev_nc_u32": "({b} - {a})",
    "v_and_b32": "({a} & {b})",
    "v_or_b32": "({a} | {b})",
    "v_xor_b32": "({a} ^ {b})",
    "v_xnor_b32": "(~({a} ^ {b}))",
    "v_not_b32": "(~{a})",
    "v_bfrev_b32": "reversebits({a})",
    "v_ffbl_b32": "firstbitlow({a})",
    "v_ffbh_u32": "({a} != 0u ? (31u - firstbithigh({a})) : 0xFFFFFFFFu)",
    "v_lshlrev_b32": "({b} << ({a} & 31u))",
    "v_lshrrev_b32": "({b} >> ({a} & 31u))",
    "v_ashrrev_i32": "asuint(asint({b}) >> ({a} & 31u))",
    "v_min_u32": "min({a}, {b})",
    "v_max_u32": "max({a}, {b})",
    "v_min_i32": "asuint(min(asint({a}), asint({b})))",
    "v_max_i32": "asuint(max(asint({a}), asint({b})))",
    "v_mul_u32_u24": "(({a} & 0xFFFFFFu) * ({b} & 0xFFFFFFu))",
    "v_mul_hi_u32_u24": "MulHiU32({a} & 0xFFFFFFu, {b} & 0xFFFFFFu)",
    "v_mul_i32_i24": "asuint(((asint({a}) << 8) >> 8) * ((asint({b}) << 8) >> 8))",
    "v_mul_lo_u32": "({a} * {b})",
    "v_mul_hi_u32": "MulHiU32({a}, {b})",
    "v_mul_hi_i32": "MulHiI32({a}, {b})",
    "v_mad_u32_u24": "(({a} & 0xFFFFFFu) * ({b} & 0xFFFFFFu) + {c})",
    "v_mad_i32_i24": "asuint(((asint({a}) << 8) >> 8) * ((asint({b}) << 8) >> 8) + asint({c}))",
    "v_bfe_u32": "(({a} >> ({b} & 31u)) & ((1u << ({c} & 31u)) - 1u))",
    "v_bfe_i32": "BfeI32({a}, {b}, {c})",
    "v_bfi_b32": "(({a} & {b}) | (~{a} & {c}))",
    "v_bfm_b32": "(((1u << ({a} & 31u)) - 1u) << ({b} & 31u))",
    "v_alignbit_b32": "AlignBit({a}, {b}, {c})",
    "v_alignbyte_b32": "AlignBit({a}, {b}, ({c} & 3u) * 8u)",
    "v_perm_b32": "PermB32({a}, {b}, {c})",
    "v_add3_u32": "({a} + {b} + {c})",
    "v_lshl_add_u32": "(({a} << ({b} & 31u)) + {c})",
    "v_add_lshl_u32": "(({a} + {b}) << ({c} & 31u))",
    "v_lshl_or_b32": "(({a} << ({b} & 31u)) | {c})",
    "v_and_or_b32": "(({a} & {b}) | {c})",
    "v_or3_b32": "({a} | {b} | {c})",
    "v_xor3_b32": "({a} ^ {b} ^ {c})",
    "v_xad_u32": "(({a} ^ {b}) + {c})",
    "v_sad_u32": "SadU32({a}, {b}, {c})",
    "v_min3_f32": "asuint(min(min(asfloat({a}), asfloat({b})), asfloat({c})))",
    "v_max3_f32": "asuint(max(max(asfloat({a}), asfloat({b})), asfloat({c})))",
    "v_med3_f32": "asuint(max(min(asfloat({a}), asfloat({b})), min(max(asfloat({a}), asfloat({b})), asfloat({c}))))",
    "v_min3_u32": "min(min({a}, {b}), {c})",
    "v_max3_u32": "max(max({a}, {b}), {c})",
    "v_med3_u32": "max(min({a}, {b}), min(max({a}, {b}), {c}))",
    "v_min3_i32": "asuint(min(min(asint({a}), asint({b})), asint({c})))",
    "v_max3_i32": "asuint(max(max(asint({a}), asint({b})), asint({c})))",
    "v_med3_i32": "asuint(max(min(asint({a}), asint({b})), min(max(asint({a}), asint({b})), asint({c}))))",
    "v_bcnt_u32_b32": "(countbits({a}) + {b})",
    "v_mbcnt_lo_u32_b32": "(countbits({a} & ((1u << lane) - 1u)) + {b})",
    "v_mbcnt_hi_u32_b32": "{b}",
    "v_cndmask_b32": "((({c} >> lane) & 1u) != 0 ? {b} : {a})",
    "v_mul_hi_i32_i24": "MulHiI32(asuint((asint({a}) << 8) >> 8), asuint((asint({b}) << 8) >> 8))",
    "v_cvt_flr_i32_f32": "FloorToI32({a}, false)",
    "v_cvt_rpi_i32_f32": "FloorToI32({a}, true)",
    "v_cvt_off_f32_i4": "asuint((float)((asint({a}) << 28) >> 28) * 0.0625f)",
    "v_rcp_iflag_f32": "asuint(rcp(asfloat({a})))",
    "v_ffbh_i32": "LeadingSignsI32({a})",
    "v_frexp_exp_i32_f32": "FrexpExpF32({a})",
    "v_frexp_mant_f32": "FrexpMantF32({a})",
    "v_fmac_legacy_f32": "FmaLegacyF32({a}, {b}, {d})",
    "v_madmk_f32": "asuint(mad(asfloat({a}), asfloat({k}), asfloat({b})))",
    "v_madak_f32": "asuint(mad(asfloat({a}), asfloat({b}), asfloat({k})))",
    "v_mul_lo_i32": "({a} * {b})",
    "v_sad_u8": "(SadU8({a}, {b}) + {c})",
    "v_sad_hi_u8": "((SadU8({a}, {b}) << 16) + {c})",
    "v_lerp_u8": "LerpU8({a}, {b}, {c})",
    "v_cubeid_f32": "asuint(Cube({a}, {b}, {c}).x)",
    "v_cubesc_f32": "asuint(Cube({a}, {b}, {c}).y)",
    "v_cubetc_f32": "asuint(Cube({a}, {b}, {c}).z)",
    "v_cubema_f32": "asuint(Cube({a}, {b}, {c}).w)",
}
COMPARE = re.compile(r"^v_cmp(x?)_(f|lt|eq|le|gt|lg|ge|ne|neq|t|tru|o|u|nge|nlg|ngt|nle|nlt|class)"
                     r"_(f32|u32|i32|u16|i16)$")
CONSTANT_TESTS = {"f": "false", "t": "true", "tru": "true"}
# Tests flottants non ordonnés (vrais si une source est NaN) : négation du test ordonné.
# NaN testé à part : sans cela, le pilote AMD RDNA 2 confond comparaisons ordonnées et non ordonnées (#134).
UNORDERED_F32 = {
    "o": "(!IsNanF32({a}) && !IsNanF32({b}))", "u": "(IsNanF32({a}) || IsNanF32({b}))",
    "nge": "(IsNanF32({a}) || IsNanF32({b}) || !(asfloat({a}) >= asfloat({b})))",
    "nlg": "(IsNanF32({a}) || IsNanF32({b}) || !(asfloat({a}) < asfloat({b}) || asfloat({a}) > asfloat({b})))",
    "ngt": "(IsNanF32({a}) || IsNanF32({b}) || !(asfloat({a}) > asfloat({b})))",
    "nle": "(IsNanF32({a}) || IsNanF32({b}) || !(asfloat({a}) <= asfloat({b})))",
    "nlt": "(IsNanF32({a}) || IsNanF32({b}) || !(asfloat({a}) < asfloat({b})))", "class": "((({b}) >> ClassF32({a})) & 1u) != 0u",
}


HALF_CASTS = {"u16": "(({x}) & 0xFFFFu)", "i16": "(asint(({x}) << 16u) >> 16)"}


def compare_condition(name, a, b):
    found = COMPARE.match(name)
    operation, kind = found.group(2), found.group(3)
    if operation in CONSTANT_TESTS:
        return CONSTANT_TESTS[operation]
    if operation in UNORDERED_F32:
        if kind != "f32":
            raise Untranslatable(f"non traduite en HLSL : {name}")
        return UNORDERED_F32[operation].format(a=a, b=b)
    symbol = COMPARE_OPERATORS[operation]
    if kind in HALF_CASTS:                             # S0[15:0] vs S1[15:0] (emu/rdna2_exec_cmp16.py)
        return f"{HALF_CASTS[kind].format(x=a)} {symbol} {HALF_CASTS[kind].format(x=b)}"
    cast = {"f32": "asfloat", "u32": "", "i32": "asint"}[kind]
    if operation == "lg" and kind == "f32":            # ordonné et différent
        return f"(!IsNanF32({a}) && !IsNanF32({b}) && ({cast}({a}) < {cast}({b}) || {cast}({a}) > {cast}({b})))"
    if kind == "f32" and operation in ("lt", "le", "gt", "ge"):
        return f"(!IsNanF32({a}) && !IsNanF32({b}) && {cast}({a}) {symbol} {cast}({b}))"
    return f"{cast}({a}) {symbol} {cast}({b})"


def is_compare(name):
    return bool(COMPARE.match(name))
