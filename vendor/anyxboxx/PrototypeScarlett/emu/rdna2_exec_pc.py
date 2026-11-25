"""Compteur de programme : s_getpc_b64, s_setpc_b64, s_swappc_b64 (SOP1) et s_call_b64 (SOPK) dans l'interpréteur.

Adresse d'une instruction dans notre modèle : PROGRAM_BASE + son décalage en octets depuis le début du
programme, sur 64 bits ; PROGRAM_BASE vaut 0. Le programme n'est pas copié dans la mémoire du wave : une
adresse lue par s_getpc_b64 sert à revenir dans le code (s_setpc_b64, s_swappc_b64), pas à lire des
données placées après lui. Le recompilateur HLSL connaît le décalage de chaque instruction et écrit la
même constante (emu/rdna2_hlsl_pc.py).

Sémantique, d'après la spécification XML d'AMD (RDNA 2) et le pseudo-code RDNA 3 d'AMD des mêmes
instructions :
- s_getpc_b64 : D = adresse de l'instruction suivante (PC + 4) ;
- s_setpc_b64 : PC = S0 (paire de SGPR) ;
- s_swappc_b64 : cible = S0, lue d'abord ; D = PC + 4 ; PC = cible ;
- s_call_b64 : D = PC + 4 ; PC = PC + 4 + SIMM16 × 4 (même déplacement que s_branch).
Une cible hors du programme ou non alignée sur 4 octets lève Untranslated en nommant l'instruction.
"""
from emu.rdna2_exec_ops import MASK64, signed

PROGRAM_BASE = 0
DWORD = 4
NULL = 125
SGPR_END = 128                                  # S0 de s_setpc/s_swappc : une paire de SGPR (OPR_SREG)
FAMILIES = {"s_getpc_b64": "SOP1", "s_setpc_b64": "SOP1", "s_swappc_b64": "SOP1", "s_call_b64": "SOPK"}
OPERATIONS = frozenset(FAMILIES)


def _untranslated(message):
    from emu.rdna2_exec import Untranslated
    return Untranslated(message)


def address(offset):
    """Adresse 64 bits de l'octet `offset` du programme."""
    return PROGRAM_BASE + offset


def check_destination(name, code):
    """D occupe une paire de SGPR alignée (numéro pair) ; NULL (125) ignore l'écriture."""
    if code != NULL and code % 2:
        raise _untranslated(f"{name} : destination s{code} impaire, une paire de SGPR doit être alignée")


def _write(wave, name, code, value):
    check_destination(name, code)
    wave.write_scalar(code, value, 2)


def _jump_source(wave, name, code):
    if code >= SGPR_END or (code != NULL and code % 2):
        raise _untranslated(f"{name} : S0 doit être une paire de SGPR alignée (opérande {code})")
    return wave.read_scalar(code, None, 2)


def _offset(name, target, length):
    offset = target - PROGRAM_BASE
    if not 0 <= offset < length or offset % DWORD:
        raise _untranslated(f"{name} : cible 0x{target & MASK64:X} hors du programme ou non alignée")
    return offset


def execute(wave, name, fields, following, length):
    """Exécute l'instruction à wave.pc et renvoie le décalage de la suivante.

    `following` : décalage de l'instruction qui suit ; `length` : taille du programme en octets."""
    if name == "s_getpc_b64":
        _write(wave, name, fields["sdst"], address(following))
        return following
    if name == "s_call_b64":
        target = _offset(name, address(wave.pc + DWORD + signed(fields["simm16"], 16) * DWORD), length)
        _write(wave, name, fields["sdst"], address(following))
        return target
    target = _offset(name, _jump_source(wave, name, fields["ssrc0"]), length)
    if name == "s_swappc_b64":
        _write(wave, name, fields["sdst"], address(following))
    return target
