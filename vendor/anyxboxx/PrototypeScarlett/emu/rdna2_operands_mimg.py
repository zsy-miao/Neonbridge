"""Texte des instructions d'image RDNA 2 (MIMG) au format de LLVM :
« image_sample v[0:3], v[4:5], s[8:15], s[16:19] dmask:0xf dim:SQ_RSRC_IMG_2D ».

Le nombre de registres de données et d'adresses n'est pas encodé : il se déduit de dmask, d16,
tfe, de la dimension, de a16 et des arguments de l'opcode. Calcul repris de LLVM
(AMDGPUDisassembler::convertMIMGInst, AMDGPU::getAddrSizeMIMGOp, MIMGInstructions.td :
dimensions, variantes d'échantillonnage, intersect_ray ; AMDGPUInstPrinter::printDim),
llvm/llvm-project, Apache 2.0 avec exception LLVM. Cible gfx10.3 (G16 disponible).
"""
from emu import rdna2, rdna2_image

DWORD = 4
VGPR_FIRST = 256
# Encodage de dim → (suffixe, coordonnées, gradients) : AMDGPUDimProps de LLVM.
DIMS = (("1D", 1, 2), ("2D", 2, 4), ("3D", 3, 6), ("CUBE", 3, 4), ("1D_ARRAY", 2, 2),
        ("2D_ARRAY", 3, 4), ("2D_MSAA", 3, 4), ("2D_MSAA_ARRAY", 4, 4))
SAMPLERS = ("image_sample", "image_gather4")
WITH_SAMPLER = (*SAMPLERS, "image_get_lod")
GATHER = "image_gather4"
RESINFO = "image_get_resinfo"
EXTRA_ARGUMENTS = frozenset({"c", "b", "o"})       # comparaison, biais, décalage : un mot chacun
LOD_ARGUMENTS = frozenset({"l", "cl", "mip"})      # lod, clamp ou niveau de mip
GRADIENTS = frozenset({"d", "cd"})
G16 = "g16"
GATHER_CHANNELS = 4
# Atomiques : variantes 32 et 64 bits seulement (cmpswap : données et valeur comparée).
ATOMIC, CMPSWAP = "_atomic_", "cmpswap"
ATOMIC_WIDTHS, CMPSWAP_WIDTHS = (1, 2), (2, 4)
# intersect_ray : (adresses, adresses avec a16), données sur 4 registres, ressource sur 4.
BVH_ADDRESSES = {"image_bvh_intersect_ray": (11, 8), "image_bvh64_intersect_ray": (12, 9)}
BVH_DATA, BVH_RSRC = 4, 4
RSRC_WIDTH, SAMPLER_WIDTH = 8, 4
NSA_ADDRESSES_PER_DWORD = 4
LARGEST_PACKED, ROUNDED_ADDRESSES = 12, 16       # au-delà de 12 adresses consécutives : 16
DLC_BIT, GLC_BIT, SLC_BIT = 7, 13, 25
# Ordre d'écriture de LLVM : $dmask$dim$unorm$cpol$r128$a16$tfe$lwe puis $d16.
FLAGS_BEFORE_POLICY = ("unorm",)
FLAGS_AFTER_POLICY = ("r128", "a16", "tfe", "lwe", "d16")


def _arguments(name):
    """Mots de l'opcode : (arguments en plus, coordonnées, lod/clamp/mip, gradients, g16)."""
    tokens = frozenset(name.split("_")[2:])
    return (len(tokens & EXTRA_ARGUMENTS), name != RESINFO, bool(tokens & LOD_ARGUMENTS) or name == RESINFO,
            bool(tokens & GRADIENTS), G16 in tokens)


def address_count(name, dim, a16):
    """Registres d'adresse (getAddrSizeMIMGOp) ; les gradients ne sont regroupés par deux qu'en g16."""
    if name in BVH_ADDRESSES:
        return BVH_ADDRESSES[name][a16]
    extra, coordinates, lod, gradients, g16 = _arguments(name)
    _, axes, derivatives = DIMS[dim]
    components = (axes if coordinates else 0) + lod
    count = extra + ((components + 1) // 2 if a16 else components)
    if gradients:
        count += (derivatives // 2 + 1) // 2 * 2 if g16 else derivatives
    return count


def data_count(name, fields):
    """Registres de données : canaux de dmask (4 en gather4), divisés par deux en d16, +1 avec tfe."""
    if name in BVH_ADDRESSES:
        return BVH_DATA
    count = GATHER_CHANNELS if name.startswith(GATHER) else max(bin(fields["dmask"]).count("1"), 1)
    if ATOMIC in name:                             # pas de variante à cette largeur : LLVM garde la plus petite
        widths = CMPSWAP_WIDTHS if CMPSWAP in name else ATOMIC_WIDTHS
        count = count if count in widths else widths[0]
    if fields["d16"]:
        count = (count + 1) // 2
    return count + fields["tfe"]


def _addresses(fields, count, source):
    if fields["nsa"]:                              # adresses dispersées, une par octet
        listed = fields["vaddr"][:min(count, len(fields["vaddr"]))]
        return "[" + ", ".join(source(VGPR_FIRST + register) for register in listed) + "]"
    width = ROUNDED_ADDRESSES if count > LARGEST_PACKED else count
    return source(VGPR_FIRST + fields["vaddr"][0], width)


def _modifiers(word, fields):
    parts = [f"dmask:0x{fields['dmask']:x}"] if fields["dmask"] else []
    parts.append(f"dim:SQ_RSRC_IMG_{DIMS[fields['dim']][0]}")
    parts += [flag for flag in FLAGS_BEFORE_POLICY if fields[flag]]
    parts += [name for bit, name in ((GLC_BIT, "glc"), (SLC_BIT, "slc"), (DLC_BIT, "dlc")) if word >> bit & 1]
    return parts + [flag for flag in FLAGS_AFTER_POLICY if fields[flag]]


def text(data, offset=0):
    """Instruction MIMG désassemblée, ou None (opcode inconnu, instruction tronquée)."""
    from emu.rdna2_operands import source

    _, opcode, size = rdna2.classify(data, offset)
    name = rdna2.mnemonic("MIMG", opcode)
    if name is None or len(data) < offset + size * DWORD:
        return None
    word = int.from_bytes(data[offset:offset + size * DWORD], "little")
    fields = rdna2_image.instruction_fields(word)
    count = address_count(name, fields["dim"], fields["a16"])
    data_registers = source(VGPR_FIRST + fields["vdata"], data_count(name, fields))
    operands = [data_registers, _addresses(fields, count, source)]
    if name in BVH_ADDRESSES:                      # « $vdata, $vaddr, $srsrc$a16 »
        operands.append(source(fields["srsrc"], BVH_RSRC))
        return f"{name} {', '.join(operands)}" + (" a16" if fields["a16"] else "")
    operands.append(source(fields["srsrc"], RSRC_WIDTH))
    if name.startswith(WITH_SAMPLER):
        operands.append(source(fields["ssamp"], SAMPLER_WIDTH))
    return " ".join([f"{name} {', '.join(operands)}", *_modifiers(word, fields)])
