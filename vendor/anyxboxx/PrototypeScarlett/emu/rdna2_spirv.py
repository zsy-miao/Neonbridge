"""Second traducteur RDNA 2 → SPIR-V (Vulkan) : celui de Leonida, appelé sur notre désassemblage.

Leonida (github.com/ItsNotRexed/Leonida, GPL-3.0) traduit le texte de désassemblage de LLVM
(`<instruction> ; encoding: [...]`) en SPIR-V. Son code est copié tel quel dans
third_party/leonida/ ; notre désassembleur (emu/rdna2_operands.py) produit ce même texte, sans
llvm-mc. Le nom du fichier donne l'étage (vs_, ps_, cs_, ms_, gs_).

    asm = rdna2_spirv.translate(programme, "ps")    # texte SPIR-V (.spvasm)
    rdna2_spirv.validate(asm)                        # spirv-as + spirv-val s'ils sont installés
"""
import json
import shutil
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

from emu import rdna2, rdna2_operands
from emu.rdna2_hlsl_ops import Untranslatable

LEONIDA = Path(__file__).resolve().parents[2] / "third_party" / "leonida"
STAGES = ("vs", "ps", "cs", "ms", "gs")
TARGET_ENV = "vulkan1.2"
TIMEOUT = 60


def disassembly(data, code_start=None):
    """Texte LLVM du programme, une instruction par ligne avec son encodage (format de shader_ir.py).

    Sans `code_start`, un mot sans texte LLVM est refusé. Avec `code_start` (position du code dans le blob
    FSSM donné à Leonida), on fait comme llvm-mc : le mot est sauté (4 octets) et signalé après « ; --- »,
    et Leonida se recale sur le FSSM (données entre plusieurs programmes d'un même blob)."""
    lines, invalid, offset = [], 0, 0
    while offset < len(data):
        family, opcode, size = rdna2.classify(data, offset)
        end = offset + size * rdna2.DWORD
        text = rdna2_operands.text(data, offset) if end <= len(data) else None
        if text is None:
            if code_start is None:
                if end > len(data):
                    raise Untranslatable("programme tronqué : dernière instruction incomplète")
                name = rdna2.mnemonic(family, opcode) or family
                raise Untranslatable(f"pas de texte LLVM pour {name} ({family}) : opérandes non décodés")
            invalid += 1
            offset += rdna2.DWORD
            continue
        lines.append(f"\t{text} ; encoding: [{','.join(f'0x{b:02x}' for b in data[offset:end])}]")
        offset = end
    if code_start is not None:
        lines.insert(0, f"; programme  code 0x{code_start:x}..0x{code_start + len(data):x}")
    if invalid:
        lines += ["; --- llvm-mc messages ---", *["; invalid instruction encoding"] * invalid]
    return "\n".join(lines) + "\n"


def _leonida():
    if str(LEONIDA) not in sys.path:
        sys.path.insert(0, str(LEONIDA))
    import rdna2spirv

    return rdna2spirv


DXBC_HEADER = 32                       # "DXBC", hachage (16), version, taille, nombre de sections
SIGNATURE_SECTIONS = (b"ISG1", b"OSG1")


def empty_container():
    """Conteneur DXBC minimal à signatures vides : Leonida y lit ISG1/OSG1 (absentes de nos tests).

    Un vrai shader console apporte son propre conteneur (sa signature d'entrée et de sortie)."""
    sections = [fourcc + struct.pack("<I", 8) + struct.pack("<II", 0, 8) for fourcc in SIGNATURE_SECTIONS]
    offsets, position = [], DXBC_HEADER + 4 * len(sections)
    for section in sections:
        offsets.append(position)
        position += len(section)
    header = b"DXBC" + bytes(16) + struct.pack("<III", 1, position, len(sections))
    return header + struct.pack(f"<{len(offsets)}I", *offsets) + b"".join(sections)


def _native_sources(folder, name, stage, fssm, dispatch):
    """Ce que Leonida relit à côté du listing : le blob FSSM (fssm/<nom>.fssm), ou pour un compute shader
    sans FSSM l'état de dispatch (fspx/<nom>.json : rsrc2 et taille du groupe, mots 0x138-0x144 du FSSM)."""
    if fssm is not None:
        (folder / "fssm").mkdir()
        (folder / "fssm" / f"{name}.fssm").write_bytes(fssm)
    elif stage == "cs":
        if dispatch is None:
            raise Untranslatable("compute shader : état de dispatch requis (FSSM, ou rsrc2 et taille du groupe)")
        rsrc2, *threads = dispatch
        (folder / "fspx").mkdir()
        (folder / "fspx" / f"{name}.json").write_text(json.dumps({"rsrc2": rsrc2, "threads": threads}),
                                                      encoding="utf-8")


def translate(data, stage, container=None, fssm=None, dispatch=None, code_start=None):
    """Texte SPIR-V du programme pour l'étage `stage` ; Untranslatable si Leonida le refuse.

    `container` : octets du conteneur DXBC du shader (signatures), sinon un conteneur vide.
    `fssm` : blob FSSM d'origine (shaders du jeu) ; `dispatch` : (rsrc2, x, y, z) d'un compute shader sans FSSM ;
    `code_start` : position du code dans `fssm` (mots de données sautés, voir disassembly)."""
    if stage not in STAGES:
        raise ValueError(f"étage inconnu : {stage} (attendu : {', '.join(STAGES)})")
    translator = _leonida()
    with tempfile.TemporaryDirectory(prefix="anyxboxx-spirv-") as folder:
        name = f"{stage}_shader"
        listing = Path(folder) / "s"                  # Leonida cherche <dossier>/../<nom>.dxbc
        listing.mkdir()
        source = listing / f"{name}.s"
        source.write_text(disassembly(data, code_start if fssm is not None else None), encoding="utf-8")
        (Path(folder) / f"{name}.dxbc").write_bytes(container or empty_container())
        _native_sources(Path(folder), name, stage, fssm, dispatch)
        try:
            return translator.translate(str(source))
        except translator.Unsupported as error:
            raise Untranslatable(f"Leonida : {error}") from None


def tools():
    """(spirv-as, spirv-val) trouvés dans le PATH ou le SDK Vulkan, sinon None."""
    found = (shutil.which("spirv-as"), shutil.which("spirv-val"))
    return found if all(found) else None


def validate(asm):
    """Assemble et valide le SPIR-V ; renvoie le module binaire. None si les outils sont absents."""
    found = tools()
    if found is None:
        return None
    assembler, validator = found
    with tempfile.TemporaryDirectory(prefix="anyxboxx-spirv-") as folder:
        text, binary = Path(folder) / "shader.spvasm", Path(folder) / "shader.spv"
        text.write_text(asm, encoding="utf-8")
        for command in ([assembler, str(text), "-o", str(binary), "--target-env", TARGET_ENV],
                        [validator, "--target-env", TARGET_ENV, str(binary)]):
            result = subprocess.run(command, capture_output=True, text=True, timeout=TIMEOUT)
            if result.returncode != 0:
                raise RuntimeError(f"SPIR-V refusé par {Path(command[0]).name} :\n{result.stderr}")
        return binary.read_bytes()
