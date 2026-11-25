"""Mesure, sans llvm-mc, ce que le désassembleur d'AnyXboxX et le traducteur SPIR-V font de programmes FSSM.

Entrée : un dossier où <nom>.dxbc (conteneur) et fssm/<nom>.fssm (programme RDNA 2 décompressé) ont été
extraits localement, par exemple avec tools/pe_shader_containers.py et tools/fssm_unpack.py de Leonida.
Sortie : des comptes seulement (instructions, mots sans texte, programmes traduits, causes d'échec) ;
aucun shader, désassemblage ou SPIR-V n'est écrit. Ces fichiers sont des données de jeu : ils restent locaux.

    python tools/fssm_mesure.py <dossier> [--etage cs]
"""
import argparse
import collections
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "PrototypeScarlett"))
from emu import rdna2, rdna2_operands, rdna2_spirv  # noqa: E402
from emu.rdna2_hlsl_ops import Untranslatable  # noqa: E402

ENDPGM = b"\x00\x00\x81\xbf"
CODE_OFFSET_FIELD, CODE_HEADER = 0xC0, 0x28       # comme tools/shader_disasm.py de Leonida


def code_range(blob):
    """(début, fin) du code d'un FSSM : u32 à 0xC0, plus 0x28, jusqu'au dernier s_endpgm aligné ; ou None."""
    if len(blob) < CODE_OFFSET_FIELD + 4:
        return None
    start = struct.unpack_from("<I", blob, CODE_OFFSET_FIELD)[0] + CODE_HEADER
    last = blob.rfind(ENDPGM, start)
    while last >= 0 and (last - start) % 4:
        last = blob.rfind(ENDPGM, start, last)
    return (start, last + 4) if start > CODE_HEADER and last >= 0 else None


def measure(folder, stage="cs"):
    """Comptes sur tous les fssm/<nom>.fssm du dossier."""
    folder = Path(folder)
    counts = {"instructions": 0, "sans texte": 0, "traduction": collections.Counter()}
    for fssm in sorted((folder / "fssm").glob("*.fssm")):
        blob = fssm.read_bytes()
        found = code_range(blob)
        if found is None:
            counts["traduction"]["code introuvable"] += 1
            continue
        start, end = found
        code = blob[start:end]
        for instruction in rdna2.decode(code):
            counts["instructions"] += 1
            counts["sans texte"] += rdna2_operands.text(code, instruction.offset) is None
        container = folder / f"{fssm.stem}.dxbc"
        try:
            rdna2_spirv.translate(code, stage, container.read_bytes() if container.exists() else None,
                                  fssm=blob, code_start=start)
            counts["traduction"]["traduit"] += 1
        except Untranslatable as error:
            counts["traduction"][str(error)[:80]] += 1
    return counts


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("dossier")
    parser.add_argument("--etage", default="cs", choices=rdna2_spirv.STAGES)
    args = parser.parse_args(argv)
    counts = measure(args.dossier, args.etage)
    print(f"instructions : {counts['instructions']}, mots sans texte : {counts['sans texte']}")
    for outcome, number in counts["traduction"].most_common():
        print(f"{number:6}  {outcome}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
