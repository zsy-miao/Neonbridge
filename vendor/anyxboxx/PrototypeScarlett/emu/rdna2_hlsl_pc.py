"""Traduction HLSL du compteur de programme (modèle d'adresses : emu/rdna2_exec_pc.py).

s_getpc_b64 devient une constante : l'adresse de l'instruction suivante, connue à la traduction.
s_setpc_b64, s_swappc_b64 et s_call_b64 restent Untranslatable : leur cible (ou celle du retour, par
s_setpc_b64/s_swappc_b64) est lue dans des SGPR à l'exécution, ce qu'un contrôle de flux structuré
(`if`/`while` sur des blocs connus) ne représente pas.
"""
from emu import rdna2, rdna2_exec_pc
from emu.rdna2_hlsl_ops import NULL, Untranslatable, hexa, scalar_target

TRANSLATED = frozenset({"s_getpc_b64"})
REFUSED = {
    "s_setpc_b64": "saut indirect (cible lue dans une paire de SGPR)",
    "s_swappc_b64": "saut indirect (cible lue dans une paire de SGPR)",
    "s_call_b64": "appel de sous-programme : le retour passe par s_setpc_b64 ou s_swappc_b64, saut indirect",
}
NAMES = TRANSLATED | frozenset(REFUSED)


def statements(name, fields, instruction):
    """Lignes HLSL ; `fields` : champs SOP1/SOPK de l'instruction, `instruction` : son décalage et sa taille."""
    if name in REFUSED:
        raise Untranslatable(f"non traduite en HLSL : {name}, {REFUSED[name]}")
    target = fields["sdst"]
    if target == NULL:
        return []
    if target % 2:
        raise Untranslatable(f"non traduite en HLSL : {name}, destination s{target} impaire (paire non alignée)")
    value = rdna2_exec_pc.address(instruction.offset + instruction.size * rdna2.DWORD)
    return [f"{scalar_target(target)} = {hexa(value)}; {scalar_target(target + 1)} = {hexa(value >> 32)};"]
