"""Échanges entre lignes par l'unité DS, sans accès à la LDS : ds_swizzle_b32, ds_permute_b32,
ds_bpermute_b32 (interpréteur RDNA 2).

D'après le pseudo-code d'AMD (« RDNA 3 Instruction Set Architecture », DS ; descriptions identiques
dans la spécification XML RDNA 2) :
- ds_swizzle_b32 : offset 16 bits = offset1:offset0. Bit 15 à 1 et bits 14:8 à 0 : QUAD_PERM, la
  ligne 4q + k lit la ligne 4q + offset[2k+1:2k]. Bit 15 à 0 : BITMASK_PERM (SWAP, REVERSE et
  BROADCAST en sont des cas), la ligne i lit ((i & and) | or) ^ xor, masques sur 5 bits
  (offset[4:0], [9:5], [14:10]). Une ligne source inactive donne 0 ;
- ds_bpermute_b32 : la ligne i lit la ligne ((ADDR + offset) / 4) % 32, 0 si celle-ci est inactive ;
- ds_permute_b32 : chaque ligne active i écrit sa donnée dans la ligne ((ADDR + offset) / 4) % 32 ;
  à destination commune, la ligne de plus grand numéro l'emporte ; une ligne que personne ne vise
  reçoit 0.
En wave64, chaque moitié de 32 lignes échange seule (GFX10 : Mesa ACO émule une permutation sur
64 lignes, « emulate full-wave bpermute » ; lu statiquement, non mesuré). Seules les lignes actives
écrivent VDST. Les modes ROTATE et FFT (offset ≥ 0xC000) lèvent
Untranslated : leur pseudo-code ne dit ni comment l'indice reste dans le groupe de 32 lignes, ni
sur combien de bits reverse_bits opère. Les autres offsets (0x8100 à 0xBFFF) sont réservés.
"""
from emu import rdna2_swizzle as swz

LANES = 32
DWORD = 4


def _untranslated(message):
    from emu.rdna2_exec import Untranslated

    return Untranslated(message)


def swizzle_source(offset, lane):
    """Ligne lue par `lane` pour l'offset de ds_swizzle_b32."""
    if offset >= swz.ROTATE_MODE_LO:
        raise _untranslated(f"ds_swizzle_b32 : mode ROTATE/FFT (offset 0x{offset:04X}) non vérifié")
    if offset & swz.QUAD_PERM_ENC_MASK == swz.QUAD_PERM_ENC:
        quad = lane % swz.LANE_NUM
        return lane - quad + (offset >> (swz.LANE_SHIFT * quad) & swz.LANE_MASK)
    if offset & swz.BITMASK_PERM_ENC_MASK == swz.BITMASK_PERM_ENC:
        and_mask = offset >> swz.BITMASK_AND_SHIFT & swz.BITMASK_MASK
        or_mask = offset >> swz.BITMASK_OR_SHIFT & swz.BITMASK_MASK
        xor_mask = offset >> swz.BITMASK_XOR_SHIFT & swz.BITMASK_MASK
        return ((lane & and_mask) | or_mask) ^ xor_mask
    raise _untranslated(f"ds_swizzle_b32 : offset réservé 0x{offset:04X}")


def _half(lane):
    """Première ligne de la moitié de 32 lignes qui contient `lane` (0 en wave32)."""
    return lane & ~(LANES - 1)


def _target(wave, addr, lane, offset):
    return _half(lane) | (wave.v[addr][lane] + offset) // DWORD % LANES


def execute(wave, name, addr, data0, vdst, offset):
    """Exécute ds_swizzle_b32, ds_permute_b32 ou ds_bpermute_b32."""
    active = set(wave.active())
    source = [wave.v[data0][lane] if lane in active else 0 for lane in range(wave.lanes)]
    if name == "ds_swizzle_b32":
        result = {lane: source[_half(lane) | swizzle_source(offset, lane % LANES)] for lane in active}
    elif name == "ds_bpermute_b32":
        result = {lane: source[_target(wave, addr, lane, offset)] for lane in active}
    else:
        forwarded = [0] * wave.lanes
        for lane in sorted(active):             # ordre croissant : la plus grande ligne gagne
            forwarded[_target(wave, addr, lane, offset)] = source[lane]
        result = {lane: forwarded[lane] for lane in active}
    for lane, value in result.items():
        wave.v[vdst][lane] = value


NAMES = frozenset({"ds_swizzle_b32", "ds_permute_b32", "ds_bpermute_b32"})
