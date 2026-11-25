"""Génère PrototypeScarlett/emu/rdna2_ops.py : (famille, opcode) → mnémonique RDNA 2.

Source : les tests d'encodage publics de LLVM (llvm/test/MC/AMDGPU/gfx10_asm_*.s,
gfx1030_new.s et mtbuf-gfx10.s, licence Apache 2.0 avec exception LLVM). Chaque ligne « encoding: [...] »
est passée au décodeur d'AnyXboxX ; on vérifie au passage que la longueur qu'il calcule
est celle de LLVM. Les noms de gfx1030 (RDNA 2) l'emportent sur ceux de gfx1010. Les lignes
sans « encoding: » (« // GFX10: [0xfa,...] », gfx10_asm_dpp16.s, dpp8.s, mimg.s) comptent aussi :
l'instruction est alors la ligne source qui les précède (seule source de v_sat_pk_u8_i16).

Seconde source : les fichiers .td du même dossier (llvm/lib/Target/AMDGPU/VOP3PInstructions.td et
VOP3Instructions.td), dont les lignes « defm V_… : VOP3P_Real_gfx10…<0xNN> » ou
« VOP3…_Real_…gfx10…<0xNNN> » donnent les opcodes VOP3P et VOP3 gfx10 ; v_div_scale_f64 et les
opcodes VOP3P n'ont aucun test d'encodage gfx10. Elles ne complètent que les opcodes que les
encodages ne nomment pas ; un désaccord entre les deux sources est signalé.

    python tools/rdna2_mnemonics.py <dossier des .s et .td> [--verifier]
"""
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "PrototypeScarlett"))
from emu import rdna2  # noqa: E402

OUTPUT = ROOT / "PrototypeScarlett" / "emu" / "rdna2_ops.py"
ENCODING = re.compile(r"^\s*(?://|;)\s*([A-Z0-9_,-]*?):\s*(.*?)\s*;?\s*encoding:\s*\[([^\]]*)\]")
BARE_ENCODING = re.compile(r"^\s*(?://|;)\s*([A-Z0-9_,-]+):\s*()\[(\s*0x[0-9a-fA-F]{2}(?:\s*,\s*0x[0-9a-fA-F]{2})*\s*)\]\s*$")
SUFFIX = re.compile(r"_(e32|e64|sdwa|dpp|dpp8)$")
RDNA2_FILE = "gfx1030"
# Les opcodes VOP3 reprennent VOPC (0x000), VOP2 (0x100) et VOP1 (0x180).
VOP3_BASES = (("VOPC", 0x000, 0x100), ("VOP2", 0x100, 0x40), ("VOP1", 0x180, 0x80))


def llvm_lines(folder, bare=False):
    """(fichier, préfixe, instruction, octets) de chaque ligne d'encodage des tests LLVM.

    Quand le commentaire ne contient que l'encodage (« // W32: encoding: [...] »),
    l'instruction est la dernière ligne source qui le précède. Avec `bare`, les octets seuls
    (« // GFX10: [...] ») comptent aussi ; leur instruction est le texte source, pas celui que
    LLVM écrit : bon pour le mnémonique, pas pour comparer le texte des opérandes."""
    for path in sorted(Path(folder).glob("*.s")):
        source = ""
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            stripped = line.strip()
            if stripped and not stripped.startswith(("//", ";", ".")):
                source = stripped.split("//")[0].strip()
                continue
            match = ENCODING.match(line) or (bare and BARE_ENCODING.match(line))
            if match:
                data = bytes(int(b, 16) for b in match.group(3).split(","))
                yield path.name, match.group(1), match.group(2) or source, data


def read_encodings(folder):
    """(fichier, mnémonique, octets) de chaque ligne d'encodage des tests LLVM."""
    for name, _, instruction, data in llvm_lines(folder, bare=True):
        if instruction and instruction[0].isalpha():
            yield name, SUFFIX.sub("", instruction.split()[0].rstrip(",")), data


def collect(folder):
    """Votes par (famille, opcode) et liste des longueurs que le décodeur calcule mal."""
    votes, preferred, wrong = defaultdict(Counter), defaultdict(Counter), []
    for name, mnemonic, data in read_encodings(folder):
        family, opcode, size = rdna2.classify(data + bytes(8))
        if size * rdna2.DWORD != len(data):
            wrong.append((mnemonic, len(data), size * rdna2.DWORD))
        base = family                     # FLAT, GLOBAL, SCRATCH : mêmes opcodes, noms distincts
        (preferred if name.startswith(RDNA2_FILE) else votes)[(base, opcode)][mnemonic] += 1
    return votes, preferred, wrong


def choose(votes, preferred):
    table = {}
    for key in set(votes) | set(preferred):
        source = preferred.get(key) or votes[key]
        table[key] = source.most_common(1)[0][0]
    for family, base, count in VOP3_BASES:
        for opcode in range(count):
            if (family, opcode) in table and ("VOP3", base + opcode) not in table:
                table[("VOP3", base + opcode)] = table[(family, opcode)]
    _complete_segments(table)
    return dict(sorted(table.items()))


GLOBAL_ONLY = ("_addtid", "atomic_csub", "atomic_fcmpswap")


def _complete_segments(table):
    """FLAT, GLOBAL et SCRATCH partagent leurs opcodes (ISA RDNA 2, chapitre FLAT) : un nom connu
    dans un segment donne celui des autres. SCRATCH n'a pas d'atomiques ; addtid, csub et
    fcmpswap n'existent qu'en GLOBAL."""
    segments = ("FLAT", "GLOBAL", "SCRATCH")
    operations = {}
    for (family, opcode), name in table.items():
        if family in segments:
            operations.setdefault(opcode, name.split("_", 1)[1])
    for opcode, operation in operations.items():
        for segment in segments:
            if (segment, opcode) in table:
                continue
            if segment == "SCRATCH" and "atomic" in operation:
                continue
            if segment != "GLOBAL" and any(marker in f"_{operation}" for marker in GLOBAL_ONLY):
                continue
            table[(segment, opcode)] = f"{segment.lower()}_{operation}"


# « defm V_X : VOP3be_Real_gfx6_gfx7_gfx10<0x16e>; », « … : VOP3OpSel_Real_gfx10_with_name<0x14e,
# "V_ALIGNBIT_B32_opsel", "v_alignbit_b32">; » : famille, opcode, et nom d'assembleur s'il diffère.
TD_GFX10 = re.compile(r'^\s*defm\s+(V_\w+)\s*:\s*(VOP3P?)(?:[A-Za-z]*)_Real_\w*?gfx10\w*\s*'
                      r'<\s*(0x[0-9A-Fa-f]+)\s*(?:,\s*"\w+"\s*,\s*"(\w+)"\s*)?>')


def td_mnemonics(folder):
    """(« VOP3P » ou « VOP3 », opcode) → mnémonique, d'après les définitions gfx10 des .td du dossier."""
    table = {}
    for path in sorted(Path(folder).glob("*.td")):
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            match = TD_GFX10.match(line)
            if match:
                name = match.group(4) or SUFFIX.sub("", match.group(1).lower())
                table[(match.group(2), int(match.group(3), 16))] = name
    return table


def merge_td(table, td):
    """Ajoute à `table` les opcodes que seule la .td nomme ; renvoie les désaccords (clé, encodages, td)."""
    disagreements = []
    for key, name in sorted(td.items()):
        if key not in table:
            table[key] = name
        elif table[key] != name:
            disagreements.append((key, table[key], name))
    return disagreements


def render(table):
    lines = ['"""Mnémoniques RDNA 2 par (famille, opcode). Généré par tools/rdna2_mnemonics.py',
             "depuis les tests d'encodage de LLVM (Apache 2.0 avec exception LLVM). Ne pas modifier.\"\"\"",
             "", "MNEMONICS = {"]
    lines += [f'    ("{family}", 0x{opcode:03X}): "{name}",' for (family, opcode), name in table.items()]
    return "\n".join(lines + ["}", ""])


def main(argv):
    if not argv:
        print(__doc__)
        return 2
    votes, preferred, wrong = collect(argv[0])
    total = sum(sum(c.values()) for c in votes.values()) + sum(sum(c.values()) for c in preferred.values())
    print(f"{total} encodages lus, {len(wrong)} longueurs fausses")
    for mnemonic, expected, got in wrong[:10]:
        print(f"  {mnemonic} : LLVM {expected} octets, décodeur {got}")
    if "--verifier" in argv:
        return 1 if wrong else 0
    table = choose(votes, preferred)
    disagreements = merge_td(table, td_mnemonics(argv[0]))
    for (family, opcode), encoded, declared in disagreements:
        print(f"  désaccord {family} 0x{opcode:02X} : encodages « {encoded} », .td « {declared} »")
    if disagreements:
        return 1
    table = dict(sorted(table.items()))
    OUTPUT.write_text(render(table), encoding="utf-8")
    print(f"{len(table)} mnémoniques écrits dans {OUTPUT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
