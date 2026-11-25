"""Instructions scalaires 64 bits RDNA 2 → HLSL (masques de lignes du wave64, paires de SGPR).

Une valeur 64 bits occupe deux SGPR (`s[n]` mot bas, `s[n + 1]` mot haut). Les modèles reçoivent
{a0} {a1} (S0), {b0} {b1} (S1) et {d0} {d1} (destination) ; chaque résultat est calculé dans des
temporaires avant d'être écrit, la destination pouvant être une source. La sémantique (SCC compris) suit
l'interpréteur de référence (emu/rdna2_exec_ops.py et emu/rdna2_exec_bits.py), contre lequel la traduction
est vérifiée sur GPU. Ces instructions servent aux deux tailles de wave.
"""
from emu.rdna2_hlsl_ops import NULL, Untranslatable, scalar_target, source, source_high

LOGIC = {"and": "{x} & {y}", "or": "{x} | {y}", "xor": "{x} ^ {y}", "andn2": "{x} & ~{y}",
         "orn2": "{x} | ~{y}", "nand": "~({x} & {y})", "nor": "~({x} | {y})", "xnor": "~({x} ^ {y})"}
# EXEC = combinaison de S0 (x) et de l'ancien EXEC (y) ; D = ancien EXEC (saveexec) ou nouveau (wrexec).
EXEC_COMBINE = {**LOGIC, "andn1": "~{x} & {y}", "orn1": "~{x} | {y}"}
LITERAL = 255
FIRST_ONE = "({0} != 0u ? firstbitlow({0}) : ({1} != 0u ? 32u + firstbitlow({1}) : 0xFFFFFFFFu))"


def _pair(code, name):
    try:
        return source(code), source_high(code)
    except Untranslatable as error:
        raise Untranslatable(f"non traduite en HLSL : {name} ({error})") from error


def _target(code):
    if code == NULL:
        return "s_null", "s_null"
    return scalar_target(code), scalar_target(code + 1)


def _logic(operation):
    expression = LOGIC[operation]
    low, high = expression.format(x="{a0}", y="{b0}"), expression.format(x="{a1}", y="{b1}")
    return f"{{{{ uint r0 = {low}, r1 = {high}; scc = (r0 | r1) != 0u; {{d0}} = r0; {{d1}} = r1; }}}}"


def _exec(operation, keep_new):
    expression = EXEC_COMBINE[operation]
    low, high = expression.format(x="{a0}", y="e0"), expression.format(x="{a1}", y="e1")
    written = "s[126], s[127]" if keep_new else "e0, e1"
    first, second = written.split(", ")
    return (f"{{{{ uint e0 = s[126], e1 = s[127]; uint r0 = {low}, r1 = {high}; s[126] = r0; s[127] = r1; "
            f"scc = (r0 | r1) != 0u; {{d0}} = {first}; {{d1}} = {second}; }}}}")


SCALAR64 = {
    "s_mov_b64": "{{ uint r0 = {a0}, r1 = {a1}; {d0} = r0; {d1} = r1; }}",
    "s_cmov_b64": "if (scc) {{ uint r0 = {a0}, r1 = {a1}; {d0} = r0; {d1} = r1; }}",
    "s_cselect_b64": "{{ uint r0 = scc ? {a0} : {b0}, r1 = scc ? {a1} : {b1}; {d0} = r0; {d1} = r1; }}",
    "s_not_b64": "{{ uint r0 = ~{a0}, r1 = ~{a1}; scc = (r0 | r1) != 0u; {d0} = r0; {d1} = r1; }}",
    "s_bcnt1_i32_b64": "{{ uint r = countbits({a0}) + countbits({a1}); scc = r != 0u; {d0} = r; }}",
    "s_bcnt0_i32_b64": "{{ uint r = 64u - countbits({a0}) - countbits({a1}); scc = r != 0u; {d0} = r; }}",
    "s_ff1_i32_b64": "{{ uint x0 = {a0}, x1 = {a1}; {d0} = " + FIRST_ONE.format("x0", "x1") + "; }}",
    "s_ff0_i32_b64": "{{ uint x0 = ~{a0}, x1 = ~{a1}; {d0} = " + FIRST_ONE.format("x0", "x1") + "; }}",
    "s_cmp_eq_u64": "scc = {a0} == {b0} && {a1} == {b1};",
    "s_cmp_lg_u64": "scc = {a0} != {b0} || {a1} != {b1};",
}
SCALAR64.update({f"s_{operation}_b64": _logic(operation) for operation in LOGIC})
SCALAR64.update({f"s_{operation}_saveexec_b64": _exec(operation, False) for operation in EXEC_COMBINE})
SCALAR64.update({f"s_{operation}_wrexec_b64": _exec(operation, True) for operation in ("andn1", "andn2")})


def statement(name, fields):
    """Ligne HLSL d'une instruction scalaire 64 bits de SCALAR64 (champs SOP1, SOP2 ou SOPC)."""
    if LITERAL in (fields.get("ssrc0"), fields.get("ssrc1")):
        raise Untranslatable(f"non traduite en HLSL : {name} avec littéral (64 bits)")
    values = {}
    for key, field in (("a", "ssrc0"), ("b", "ssrc1")):
        if field in fields:
            values[f"{key}0"], values[f"{key}1"] = _pair(fields[field], name)
    values["d0"], values["d1"] = _target(fields.get("sdst", NULL))
    return SCALAR64[name].format(**values)
