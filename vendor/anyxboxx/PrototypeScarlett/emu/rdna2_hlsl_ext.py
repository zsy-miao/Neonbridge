"""HLSL recompilation of RDNA 2 SDWA (0xF9), DPP16 (0xFA), and DPP8 (0xE9/0xEA)
forms for VOP1, VOP2, and VOPC instructions (#21).
"""
from emu.rdna2_exec_ext import operand_float_flags
from emu.rdna2_exec_ops import SIXTEEN_BIT_RESULTS
from emu.rdna2_hlsl_ops import VECTOR, Untranslatable, compare_condition, is_compare, scalar_target, source
from emu.rdna2_operands_ext import BOUND_CTRL_BIT, CLAMP_BIT, DPP8, DPP8_FI, DPP16, FI_BIT, OMOD_SHIFT, SDWA

EXT_CODES = frozenset({SDWA, DPP16, DPP8, DPP8_FI})
_SEL_INFO = ((0, 8), (8, 8), (16, 8), (24, 8), (0, 16), (16, 16), (0, 32))
_OUTPUT_SCALE = {1: "2.0f", 2: "4.0f", 3: "0.5f"}


def is_ext(family, src0):
    return family in ("VOP1", "VOP2", "VOPC") and src0 in EXT_CODES


def _hlsl_extract(expr, sel, sext):
    if sel >= 6:
        return expr
    shift, width = _SEL_INFO[sel]
    mask = f"0x{(1 << width) - 1:X}u"
    base = f"(({expr} >> {shift}u) & {mask})" if shift else f"({expr} & {mask})"
    if not sext:
        return base
    sign_bit = f"0x{1 << (width - 1):X}u"
    fill = f"0x{0xFFFF_FFFF ^ ((1 << width) - 1):X}u"
    return f"((({base} & {sign_bit}) != 0u) ? ({base} | {fill}) : {base})"


def _hlsl_float_mods(expr, absolute, neg):
    if absolute:
        expr = f"({expr} & 0x7FFFFFFFu)"
    if neg:
        expr = f"({expr} ^ 0x80000000u)"
    return expr


def _hlsl_place_dst(expr, old_expr, dst_sel, dst_unused):
    if dst_sel >= 6:
        return expr
    shift, width = _SEL_INFO[dst_sel]
    mask = (1 << width) - 1
    bits = f"({expr} & 0x{mask:X}u)"
    placed = f"({bits} << {shift}u)" if shift else bits
    if dst_unused == 2:  # UNUSED_PRESERVE
        keep = 0xFFFF_FFFF ^ (mask << shift)
        return f"(({old_expr} & 0x{keep:X}u) | {placed})"
    if dst_unused == 1 and shift + width < 32:  # UNUSED_SEXT
        sign_bit = 1 << (width - 1)
        top_fill = (0xFFFF_FFFF << (shift + width)) & 0xFFFF_FFFF
        return f"((({bits} & 0x{sign_bit:X}u) != 0u) ? ({placed} | 0x{top_fill:X}u) : {placed})"
    return placed


def _hlsl_sdwa(family, name, fields, ext, active):
    if name in SIXTEEN_BIT_RESULTS:
        raise Untranslatable(f"non traduite en HLSL (SDWA) : {name}, résultat 16 bits sous dst_sel non vérifié")
    src_is_float, dst_is_float = operand_float_flags(name)
    s0_code, s0_scalar = ext & 0xFF, (ext >> 23) & 1
    s0_sel, s0_sext, s0_neg, s0_abs = (ext >> 16) & 7, (ext >> 19) & 1, (ext >> 20) & 1, (ext >> 21) & 1
    s1_code, s1_scalar = fields.get("vsrc1", 0), (ext >> 31) & 1
    s1_sel, s1_sext, s1_neg, s1_abs = (ext >> 24) & 7, (ext >> 27) & 1, (ext >> 28) & 1, (ext >> 29) & 1

    a = source(s0_code if s0_scalar else 256 + s0_code)
    a = _hlsl_extract(a, s0_sel, 0 if src_is_float else s0_sext)
    if src_is_float:
        a = _hlsl_float_mods(a, s0_abs, s0_neg)
    if "vsrc1" in fields:
        b = source(s1_code if s1_scalar else 256 + s1_code)
        b = _hlsl_extract(b, s1_sel, 0 if src_is_float else s1_sext)
        if src_is_float:
            b = _hlsl_float_mods(b, s1_abs, s1_neg)
    else:
        b = "0u"

    if family == "VOPC" or is_compare(name):
        if name.startswith("v_cmpx"):
            target = "s[126]"
        elif (ext >> 15) & 1:
            target = scalar_target((ext >> 8) & 0x7F)
        else:
            target = "s[106]"
        cond = compare_condition(name, a, b)
        return [f"{{ bool t = ({active}) && ({cond}); {target} = Ballot(lane, t); }}"]

    if name not in VECTOR:
        raise Untranslatable(f"non traduite en HLSL (SDWA) : {name}")
    vdst = f"v[{fields['vdst']}]"
    expr = VECTOR[name].format(a=a, b=b, c="s[106]", d=vdst, k="0u")
    omod, clamp = (ext >> OMOD_SHIFT) & 3, (ext >> CLAMP_BIT) & 1
    if not dst_is_float and (clamp or omod):
        raise Untranslatable(f"SDWA clamp/omod not supported on an integer result: {name}")
    if dst_is_float and (omod or clamp):
        scaled = f"asfloat({expr}) * {_OUTPUT_SCALE.get(omod, '1.0f')}"
        expr = f"asuint({'saturate(' + scaled + ')' if clamp else scaled})"
    dst_sel, dst_unused = (ext >> 8) & 7, (ext >> 11) & 3
    final_expr = _hlsl_place_dst(expr, vdst, dst_sel, dst_unused)
    return [f"if ({active}) {vdst} = {final_expr};"]


def _dpp16_lane_expr(ctrl):
    """Return (src_lane_hlsl, in_bounds_hlsl) for gfx10 dpp_ctrl."""
    if ctrl <= 0xFF:
        return f"((lane & ~3u) | ((0x{ctrl:X}u >> (2u * (lane & 3u))) & 3u))", "true"
    if 0x101 <= ctrl <= 0x10F:
        n = ctrl & 0xF
        return f"((lane & ~15u) + (lane & 15u) + {n}u)", f"((lane & 15u) + {n}u < 16u)"
    if 0x111 <= ctrl <= 0x11F:
        n = ctrl & 0xF
        return f"((lane & ~15u) + (lane & 15u) - {n}u)", f"((lane & 15u) >= {n}u)"
    if 0x121 <= ctrl <= 0x12F:
        n = ctrl & 0xF
        return f"((lane & ~15u) + (((lane & 15u) + {n}u) & 15u))", "true"
    if ctrl == 0x140:
        return "((lane & ~15u) + (15u - (lane & 15u)))", "true"
    if ctrl == 0x141:
        return "((lane & ~15u) + ((lane & 15u) ^ 7u))", "true"
    if 0x150 <= ctrl <= 0x15F:
        return f"((lane & ~15u) + {ctrl & 0xF}u)", "true"
    if 0x160 <= ctrl <= 0x16F:
        return f"((lane & ~15u) + ((lane & 15u) ^ {ctrl & 0xF}u))", "true"
    raise Untranslatable(f"dpp_ctrl réservé : 0x{ctrl:X}")


def _hlsl_dpp16(name, fields, ext, active):
    if name not in VECTOR:
        raise Untranslatable(f"non traduite en HLSL (DPP16) : {name}")
    ctrl = (ext >> 8) & 0x1FF
    src_lane_expr, in_bounds_expr = _dpp16_lane_expr(ctrl)
    src0_reg = ext & 0xFF
    fi = (ext >> FI_BIT) & 1
    bound_ctrl = (ext >> BOUND_CTRL_BIT) & 1
    s0_neg, s0_abs = (ext >> 20) & 1, (ext >> 21) & 1
    s1_neg, s1_abs = (ext >> 22) & 1, (ext >> 23) & 1
    bank_mask = (ext >> 24) & 0xF
    row_mask = (ext >> 28) & 0xF
    src_is_float, _ = operand_float_flags(name)
    vdst = f"v[{fields['vdst']}]"
    b = f"v[{fields['vsrc1']}]" if "vsrc1" in fields else "0u"
    a_val = "dpp_a"
    if src_is_float:
        a_val = _hlsl_float_mods(a_val, s0_abs, s0_neg)
        b = _hlsl_float_mods(b, s1_abs, s1_neg)
    expr = VECTOR[name].format(a=a_val, b=b, c="s[106]", d=vdst, k="0u")
    valid_expr = f"({in_bounds_expr})" if fi else f"(({in_bounds_expr}) && (((s[126] >> dpp_sl) & 1u) != 0u))"
    mask_ok = f"(((0x{row_mask:X}u >> (lane >> 4u)) & 1u) != 0u && ((0x{bank_mask:X}u >> ((lane & 15u) >> 2u)) & 1u) != 0u)"
    write_cond = f"({active}) && ({mask_ok})" if bound_ctrl else f"({active}) && ({mask_ok}) && dpp_ok"
    return [
        f"{{ uint dpp_sl = {src_lane_expr}; uint dpp_raw = ReadLane(lane, v[{src0_reg}], {in_bounds_expr} ? dpp_sl : lane); "
        f"bool dpp_ok = {valid_expr}; uint dpp_a = dpp_ok ? dpp_raw : 0u; "
        f"if ({write_cond}) {vdst} = {expr}; }}"
    ]


def _hlsl_dpp8(name, fields, ext, fi, active):
    if name not in VECTOR:
        raise Untranslatable(f"non traduite en HLSL (DPP8) : {name}")
    src0_reg = ext & 0xFF
    sel_bits = (ext >> 8) & 0xFF_FFFF
    vdst = f"v[{fields['vdst']}]"
    b = f"v[{fields['vsrc1']}]" if "vsrc1" in fields else "0u"
    expr = VECTOR[name].format(a="dpp_a", b=b, c="s[106]", d=vdst, k="0u")
    src_lane_expr = f"((lane & ~7u) | ((0x{sel_bits:X}u >> (3u * (lane & 7u))) & 7u))"
    write_cond = f"({active})" if fi else f"({active}) && (((s[126] >> dpp_sl) & 1u) != 0u)"
    return [
        f"{{ uint dpp_sl = {src_lane_expr}; uint dpp_a = ReadLane(lane, v[{src0_reg}], dpp_sl); "
        f"if ({write_cond}) {vdst} = {expr}; }}"
    ]


def statements(family, name, fields, ext, active):
    kind = fields["src0"]
    if kind == SDWA:
        return _hlsl_sdwa(family, name, fields, ext, active)
    if kind == DPP16:
        if family == "VOPC":
            raise Untranslatable(f"VOPC non traduite en HLSL (DPP16) : {name}")
        return _hlsl_dpp16(name, fields, ext, active)
    if family == "VOPC":
        raise Untranslatable(f"VOPC non traduite en HLSL (DPP8) : {name}")
    return _hlsl_dpp8(name, fields, ext, fi=(kind == DPP8_FI), active=active)
