"""Confronte le désassembleur RDNA 2 de TomBox (tombox-xshader, via rust/anyxboxx_rdna2) aux
encodages publics des tests de LLVM, comme le décodeur d'AnyXboxX (tools/rdna2_mnemonics.py).

Pour chaque encodage : longueur (mots de 32 bits) et mnémonique de TomBox comparés à LLVM.
Le rapport classe les écarts par famille ; les mnémoniques inconnus de TomBox (« vop3<0x…> »)
sont comptés à part.

    cargo build --release --manifest-path rust/Cargo.toml
    python tools/compare_tombox.py <dossier des .s LLVM> [--md rapport.md]
"""
import re
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from rdna2_mnemonics import llvm_lines  # noqa: E402

BINARY = ROOT / "rust" / "target" / "release" / "anyxboxx_rdna2.exe"
SUFFIX = re.compile(r"_(e32|e64|sdwa|dpp|dpp8)$")
UNKNOWN = re.compile(r"^[a-z0-9]+<")
MAX_EXAMPLES = 3


def encodings(folder):
    """{octets: mnémonique LLVM} sans doublon (première occurrence)."""
    found = {}
    for _, _, instruction, data in llvm_lines(folder):
        if instruction and instruction[0].isalpha():
            found.setdefault(data, SUFFIX.sub("", instruction.split()[0].rstrip(",")))
    return found


def decode_with_tombox(blobs):
    lines = "\n".join(blob.hex(" ") for blob in blobs) + "\n"
    result = subprocess.run([str(BINARY)], input=lines, capture_output=True, text=True, timeout=600, check=True)
    return [line.split(" ", 2) for line in result.stdout.splitlines()]


def compare(folder):
    table = encodings(folder)
    blobs = list(table)
    stats = defaultdict(Counter)
    examples = defaultdict(list)
    for blob, (dwords, family, mnemonic) in zip(blobs, decode_with_tombox(blobs)):
        expected = table[blob]
        if int(dwords) * 4 != len(blob):
            verdict = "longueur"
        elif UNKNOWN.match(mnemonic):
            verdict = "inconnu"
        elif SUFFIX.sub("", mnemonic) != expected:
            verdict = "nom"
        else:
            verdict = "identique"
        stats[family][verdict] += 1
        if verdict != "identique" and len(examples[family, verdict]) < MAX_EXAMPLES:
            examples[family, verdict].append(f"{blob.hex(' ')} : LLVM {expected}, TomBox {dwords} mot(s) {mnemonic}")
    return dict(sorted(stats.items())), examples


def report(stats, examples):
    lines = ["| Famille | Encodages | Identiques | Longueur fausse | Nom différent | Inconnu de TomBox |",
             "|---|---:|---:|---:|---:|---:|"]
    totals = Counter()
    for family, counts in stats.items():
        totals.update(counts)
        lines.append(f"| {family} | {sum(counts.values())} | {counts['identique']} | {counts['longueur']} | "
                     f"{counts['nom']} | {counts['inconnu']} |")
    lines.append(f"| **Total** | {sum(totals.values())} | {totals['identique']} | {totals['longueur']} | "
                 f"{totals['nom']} | {totals['inconnu']} |")
    lines += ["", "Exemples d'écarts :", ""]
    lines += [f"- {family} / {verdict} : {text}" for (family, verdict), items in sorted(examples.items())
              for text in items]
    return "\n".join(lines)


def main(argv):
    if not argv:
        print(__doc__)
        return 2
    if not BINARY.is_file():
        print(f"Binaire absent : {BINARY}. Lancez cargo build --release --manifest-path rust/Cargo.toml",
              file=sys.stderr)
        return 1
    text = report(*compare(argv[0]))
    if "--md" in argv:
        Path(argv[argv.index("--md") + 1]).write_text(text + "\n", encoding="utf-8")
    print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
