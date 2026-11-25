"""Traduction HLSL des registres indexés par M0 et des échanges (emu/rdna2_exec_movrel.py).

Les registres sont les tableaux `v[256]` et `s[128]` de chaque thread : l'indice dynamique
(numéro + M0 = s[124]) les indexe directement. Hors du modèle (au-delà de v255 ou de s105, paire
64 bits impaire), l'interpréteur lève Untranslated ; le HLSL n'y écrit rien et lit 0, sans
garantie de parité.
"""
from emu.rdna2_exec_movrel import LAST_SGPR, LITERAL, SCALAR, VECTOR, VGPR_FIRST, VGPRS
from emu.rdna2_hlsl_ops import NULL, Untranslatable, source, source_high

M0 = "s[124]"
LOW, HIGH = f"({M0} & 0x3FFu)", f"(({M0} >> 16) & 0x3FFu)"
SOURCE_OFFSET = {"v_movrels_b32": M0, "v_movrelsd_b32": M0, "v_movrelsd_2_b32": LOW, "v_swaprel_b32": LOW,
                 "s_movrels_b32": M0, "s_movrels_b64": M0, "s_movrelsd_2_b32": LOW}
DESTINATION_OFFSET = {"v_movreld_b32": M0, "v_movrelsd_b32": M0, "v_movrelsd_2_b32": HIGH, "v_swaprel_b32": HIGH,
                      "s_movreld_b32": M0, "s_movreld_b64": M0, "s_movrelsd_2_b32": HIGH}
TRANSLATED = VECTOR | SCALAR


def _index(number, name, offsets):
    return f"({number}u + {offsets[name]})" if name in offsets else f"{number}u"


def vector(name, vdst, src0, literal, active):
    """Instructions HLSL d'une instruction vectorielle de VECTOR (VOP1 ou VOP3 sans modificateur)."""
    j = _index(vdst, name, DESTINATION_OFFSET)
    if name == "v_movreld_b32":
        return [f"if ({active}) {{ uint x = {source(src0, literal)}, j = {j}; if (j < {VGPRS}u) v[j] = x; }}"]
    if src0 < VGPR_FIRST:
        raise Untranslatable(f"non traduite en HLSL : {name} (la source indexée doit être un VGPR)")
    i = _index(src0 - VGPR_FIRST, name, SOURCE_OFFSET)
    body = f"uint i = {i}, j = {j}; uint x = i < {VGPRS}u ? v[i] : 0u;"
    if name in ("v_swap_b32", "v_swaprel_b32"):
        body += f" if (i < {VGPRS}u && j < {VGPRS}u) {{ v[i] = v[j]; v[j] = x; }}"
    else:
        body += f" if (j < {VGPRS}u) v[j] = x;"
    return [f"if ({active}) {{ {body} }}"]


def _indexed_read(name, ssrc0, wide):
    """Lecture de s[ssrc0 + décalage] (et du registre suivant pour 64 bits) dans x et y."""
    if ssrc0 > LAST_SGPR:
        raise Untranslatable(f"non traduite en HLSL : {name} (la source indexée doit être un SGPR)")
    last, pair = (LAST_SGPR - 1, " && (i & 1u) == 0u") if wide else (LAST_SGPR, "")
    high = "ok ? s[i + 1u] : 0u" if wide else "0u"
    return (f"uint i = {_index(ssrc0, name, SOURCE_OFFSET)}; bool ok = i <= {last}u{pair}; "
            f"uint x = ok ? s[i] : 0u, y = {high};")


def scalar(name, fields, literal):
    """Instructions HLSL d'une instruction scalaire de SCALAR (SOP1)."""
    wide = name.endswith("_b64")
    sdst, ssrc0 = fields["sdst"], fields["ssrc0"]
    if name in ("s_movreld_b32", "s_movreld_b64"):
        if wide and ssrc0 == LITERAL:
            raise Untranslatable(f"non traduite en HLSL : {name} avec littéral 64 bits")
        reads = f"uint x = {source(ssrc0, literal)}, y = {source_high(ssrc0) if wide else '0u'};"
    else:
        reads = _indexed_read(name, ssrc0, wide)
    if name in ("s_movrels_b32", "s_movrels_b64"):
        if sdst == NULL:
            return []
        writes = f"s[{sdst}] = x;" + (f" s[{sdst + 1}] = y;" if wide else "")
        return [f"{{ {reads} {writes} }}"]
    if sdst > LAST_SGPR:
        raise Untranslatable(f"non traduite en HLSL : {name} (la destination indexée doit être un SGPR)")
    last, pair = (LAST_SGPR - 1, " && (j & 1u) == 0u") if wide else (LAST_SGPR, "")
    writes = "s[j] = x;" + (" s[j + 1u] = y;" if wide else "")
    return [f"{{ {reads} uint j = {_index(sdst, name, DESTINATION_OFFSET)}; if (j <= {last}u{pair}) {{ {writes} }} }}"]
