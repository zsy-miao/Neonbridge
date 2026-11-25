"""Registres indexés par M0 et échanges de registres de l'interpréteur RDNA 2 : v_movrels_b32,
v_movreld_b32, v_movrelsd_b32, v_movrelsd_2_b32, v_swap_b32, v_swaprel_b32, s_movrels_b32/b64,
s_movreld_b32/b64 et s_movrelsd_2_b32.

D'après le pseudo-code d'AMD (« RDNA 3 Instruction Set Architecture » ; descriptions identiques
dans la spécification XML RDNA 2) : l'indice est le numéro de registre écrit dans l'instruction
(« raw value ») plus M0, ou plus M0[9:0] pour la source et M0[25:16] pour la destination des
formes _2 et de v_swaprel. Les sources indexées doivent être des registres (VGPR pour les
vectorielles, s0-s105 pour les scalaires). Un indice hors des 256 VGPR ou de s0-s105, ou une paire
64 bits d'indice impair, lève Untranslated : le comportement du matériel n'y est pas décrit.
"""
M0, VGPR_FIRST, LITERAL = 124, 256, 255
VGPRS, LAST_SGPR = 256, 105
MASK32 = 0xFFFF_FFFF
FIELD_MASK = 0x3FF
VECTOR = frozenset({"v_movrels_b32", "v_movreld_b32", "v_movrelsd_b32", "v_movrelsd_2_b32", "v_swap_b32",
                    "v_swaprel_b32"})
SCALAR = frozenset({"s_movrels_b32", "s_movrels_b64", "s_movreld_b32", "s_movreld_b64", "s_movrelsd_2_b32"})


def _untranslated(message):
    from emu.rdna2_exec import Untranslated

    return Untranslated(message)


def offsets(name, m0):
    """(décalage de la source, décalage de la destination) ajoutés aux numéros de registres."""
    if name.endswith("_2_b32") or name == "v_swaprel_b32":
        return m0 & FIELD_MASK, m0 >> 16 & FIELD_MASK
    source = m0 if name in ("v_movrels_b32", "v_movrelsd_b32", "s_movrels_b32", "s_movrels_b64") else 0
    destination = m0 if name in ("v_movreld_b32", "v_movrelsd_b32", "s_movreld_b32", "s_movreld_b64") else 0
    return source, destination


def _vgpr(name, code):
    if code < VGPR_FIRST:
        raise _untranslated(f"{name} : la source indexée doit être un VGPR")
    return code - VGPR_FIRST


def _check(name, index, last, width=1):
    if index + width - 1 > last or (width == 2 and index % 2):
        raise _untranslated(f"{name} : registre indexé {index} hors du modèle")
    return index


def vector(wave, name, vdst, src0, literal):
    """Exécute une instruction de VECTOR (VOP1 ou VOP3) sur les lignes actives."""
    source_offset, destination_offset = offsets(name, wave.s[M0])
    destination = _check(name, vdst + destination_offset, VGPRS - 1)
    if name == "v_movreld_b32":
        for lane in wave.active():
            wave.v[destination][lane] = wave.read_lane(src0, lane, literal)
        return
    source = _check(name, _vgpr(name, src0) + source_offset, VGPRS - 1)
    swap = name in ("v_swap_b32", "v_swaprel_b32")
    for lane in wave.active():
        value = wave.v[source][lane]
        if swap:
            wave.v[source][lane] = wave.v[destination][lane]
        wave.v[destination][lane] = value


def scalar(wave, name, fields, literal):
    """Exécute une instruction de SCALAR (SOP1)."""
    width = 2 if name.endswith("_b64") else 1
    source_offset, destination_offset = offsets(name, wave.s[M0])
    sdst, ssrc0 = fields["sdst"], fields["ssrc0"]
    if name in ("s_movreld_b32", "s_movreld_b64"):
        if width == 2 and ssrc0 == LITERAL:
            raise _untranslated(f"{name} : littéral sur une source 64 bits")
        value = wave.read_scalar(ssrc0, literal, width)
    else:
        if ssrc0 > LAST_SGPR:
            raise _untranslated(f"{name} : la source indexée doit être un SGPR")
        index = _check(name, ssrc0 + source_offset, LAST_SGPR, width)
        value = wave.s[index] | (wave.s[index + 1] << 32 if width == 2 else 0)
    if name in ("s_movrels_b32", "s_movrels_b64"):
        wave.write_scalar(sdst, value, width)
        return
    if sdst > LAST_SGPR:
        raise _untranslated(f"{name} : la destination indexée doit être un SGPR")
    target = _check(name, sdst + destination_offset, LAST_SGPR, width)
    wave.s[target] = value & MASK32
    if width == 2:
        wave.s[target + 1] = value >> 32 & MASK32
