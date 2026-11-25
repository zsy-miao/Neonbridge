"""Apprend la forme des opérandes RDNA 2 sur les tests de LLVM et mesure la conformité.

Pour chaque ligne « <mnémonique> <opérandes> ; encoding: [...] » des familles couvertes par
emu/rdna2_operands.py, on cherche quel champ de l'encodage produit chaque opérande (et avec
quelle largeur de registre ou quel format d'immédiat). La forme la plus fréquente par opcode
est écrite dans PrototypeScarlett/emu/rdna2_shapes.py. On compare ensuite le désassembleur
au texte de LLVM, ligne par ligne (mode wave32).

    python tools/rdna2_shapes.py <dossier des .s> [--verifier]
"""
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "PrototypeScarlett"))
from emu import rdna2, rdna2_operands as ops  # noqa: E402
from rdna2_mnemonics import llvm_lines  # noqa: E402

OUTPUT = ROOT / "PrototypeScarlett" / "emu" / "rdna2_shapes.py"
SKIPPED_SUFFIXES = ("_sdwa", "_dpp", "_dpp8")
RANGE = re.compile(r"^(?:s|v|ttmp)\[(\d+):(\d+)\]$")
PAIR_NAMES = {"vcc": 2, "exec": 2}
HWREG_TOKEN = re.compile(r"^hwreg\((HW_REG_[A-Z0-9_]+)")
SMEM_FLAG_WORDS = frozenset(name for _, name in ops.SMEM_FLAGS)
VOP3_OR_VMEM = frozenset({"VOP3"}) | ops.vmem.FAMILIES
SEGMENTS = ("GLOBAL", "SCRATCH", "FLAT")               # ordre de préférence des formes reprises


def split_operands(text):
    parts, depth, current = [], 0, ""
    for char in text.strip():
        depth += char in "[("
        depth -= char in "])"
        if char == "," and depth == 0:
            parts.append(current.strip())
            current = ""
        else:
            current += char
    return [p for p in parts + [current.strip()] if p]


def read_lines(folder):
    """(texte attendu, opérandes sans modificateurs finaux, octets) des lignes wave32 couvertes."""
    for _, prefix, instruction, data in llvm_lines(folder):
        if "W64" in prefix or not instruction or not instruction[0].isalpha():
            continue
        mnemonic, _, operands = instruction.partition(" ")
        if mnemonic.endswith(SKIPPED_SUFFIXES):
            continue
        family, _, size = rdna2.classify(data + bytes(8))
        covered = family in ops.FAMILIES or family in ops.FIXED_FORMAT
        if not covered or size * rdna2.DWORD != len(data):
            continue
        tokens = split_operands(operands)
        if family in VOP3_OR_VMEM and tokens:         # « v2 clamp mul:2 », « s3 offen glc » : modificateurs à part
            tokens[-1] = tokens[-1].split(" ")[0]
        if family == "SMEM" and tokens:               # « s0 offset:0x64 glc dlc » : garder l'offset
            tokens[-1] = " ".join(w for w in tokens[-1].split() if w not in SMEM_FLAG_WORDS)
        if family == "DS" and tokens:                 # « v1 offset:65535 gds » : suffixes à part
            tokens[-1] = " ".join(w for w in tokens[-1].split()
                                  if w != "gds" and not w.startswith("offset"))
            tokens = [t for t in tokens if t]
        expected = " ".join(f"{mnemonic.removesuffix('_e32')} {operands}".split())
        yield expected, tokens, data


def _width(token):
    found = RANGE.match(token)
    return int(found.group(2)) - int(found.group(1)) + 1 if found else PAIR_NAMES.get(token, 1)


def _word_and_literal(data, family):
    return ops._words(data, 0, family, len(data) // rdna2.DWORD)


def candidates(family, word, literal, token):
    """Éléments de forme qui produisent exactement `token`."""
    found = []
    for name, (_, _, kind) in ops.FIELDS[family].items():
        options = ops.IMMEDIATE_FORMATS if kind == "imm" else (_width(token),)
        found += [(name, o) for o in options if ops.render(family, word, literal, (name, o)) == token]
    if literal is not None and ops.render(family, word, literal, ("literal", None)) == token:
        found.append(("literal", None))
    return found


def _fallback(family, token):
    """Texte fixe pour un registre implicite (vcc_lo) ; immédiat brut pour un texte symbolique
    (hwreg(...), vmcnt(0)...) : figé, il serait faux pour les autres valeurs."""
    symbolic = "(" in token or " " in token
    if symbolic and "simm16" in ops.FIELDS[family]:
        return ("simm16", "hex")
    return ("=", token)


def learn_shape(family, word, literal, tokens):
    shape, used = [], set()
    for token in tokens:
        options = candidates(family, word, literal, token)
        fresh = [o for o in options if o[0] not in used]
        element = (fresh or options or [_fallback(family, token)])[0]
        used.add(element[0])
        shape.append(element)
    return tuple(shape)


def _vop3_shape(word, literal, tokens):
    """VOP3 ou VOP3b (destination scalaire dans le champ abs) : la forme sans texte figé."""
    plain = learn_shape("VOP3", word, literal, tokens)
    carry = learn_shape("VOP3B", word, literal, tokens)
    fixed = [sum(name == "=" for name, _ in shape) for shape in (plain, carry)]
    return plain if fixed[0] <= fixed[1] else (ops.VOP3B_MARK, *carry)


def learn(folder):
    """Formes par (famille, opcode) et noms des registres matériels (hwreg)."""
    votes, names = defaultdict(Counter), {}
    for _, tokens, data in read_lines(folder):
        family, opcode, _ = rdna2.classify(data)
        if family in ops.FIXED_FORMAT:                # texte fixe (EXP, MIMG) : rien à apprendre
            continue
        word, literal = _word_and_literal(data, family)
        shape = _vop3_shape(word, literal, tokens) if family == "VOP3" else learn_shape(family, word, literal, tokens)
        if family in ops.vmem.FAMILIES:               # lds, atomiques avec retour : forme de base
            shape = ops.vmem.base_shape(family, rdna2.mnemonic(family, opcode) or "", word, shape)
        votes[(family, opcode)][shape] += 1
        for token in tokens:
            found = HWREG_TOKEN.match(token)
            if found:
                names[word & 0x3F] = found.group(1)
    shapes = {key: counter.most_common(1)[0][0] for key, counter in votes.items()}
    complete_segments(shapes)
    return dict(sorted(shapes.items())), dict(sorted(names.items()))


def complete_segments(shapes):
    """FLAT, GLOBAL et SCRATCH partagent leurs opcodes : une forme connue dans un segment donne celle
    des autres (au saddr près), pour les opcodes absents des tests de LLVM."""
    known = sorted((key for key in shapes if key[0] in SEGMENTS), key=lambda key: (SEGMENTS.index(key[0]), key[1]))
    for family, opcode in known:
        for segment in SEGMENTS:
            if (segment, opcode) not in shapes and rdna2.mnemonic(segment, opcode) is not None:
                shapes[(segment, opcode)] = ops.vmem.segment_shape(segment, shapes[(family, opcode)])


def conformity(folder):
    """Par famille : (lignes identiques à LLVM, total), et quelques écarts."""
    scores, samples = defaultdict(lambda: [0, 0]), []
    for expected, _, data in read_lines(folder):
        got = ops.text(data)
        family = rdna2.classify(data)[0]
        scores[family][1] += 1
        if got == expected:
            scores[family][0] += 1
        elif len(samples) < 12:
            samples.append((expected, got))
    return dict(sorted(scores.items())), samples


def render_module(shapes, names):
    lines = ['"""Forme des opérandes RDNA 2 par (famille, opcode). Généré par tools/rdna2_shapes.py',
             "depuis les tests d'encodage de LLVM (Apache 2.0 avec exception LLVM). Ne pas modifier.\"\"\"",
             "", "SHAPES = {"]
    lines += [f'    ("{family}", 0x{opcode:03X}): {shape!r},' for (family, opcode), shape in shapes.items()]
    lines += ["}", "", "# Registres matériels nommés par LLVM (s_getreg/s_setreg).", "HWREG_NAMES = {"]
    lines += [f'    {ident}: "{name}",' for ident, name in names.items()]
    return "\n".join(lines + ["}", ""])


def main(argv):
    if not argv:
        print(__doc__)
        return 2
    if "--verifier" not in argv:
        shapes, names = learn(argv[0])
        OUTPUT.write_text(render_module(shapes, names), encoding="utf-8")
        print(f"{len(shapes)} formes écrites dans {OUTPUT.relative_to(ROOT)}")
    scores, samples = conformity(argv[0])
    for family, (same, total) in scores.items():
        print(f"{family:6} {same}/{total} ({100 * same / total:.1f} %)")
    for expected, got in samples:
        print(f"  LLVM : {expected}\n  nous : {got}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
