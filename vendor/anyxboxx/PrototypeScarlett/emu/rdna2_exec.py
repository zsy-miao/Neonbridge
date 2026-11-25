"""Interpréteur de référence RDNA 2 (wave32, ou wave64 sur demande) : première brique du recompilateur.

Il exécute un programme RDNA 2 sur 32 lignes (64 avec `Wave(lanes=64)`, EXEC et VCC sur deux SGPR) : registres scalaires (SGPR, VCC, EXEC, M0),
registres vectoriels (VGPR), SCC et branchements. Le futur recompilateur RDNA 2 → DXIL
sera validé contre lui. La sémantique suit la documentation publique d'AMD (« RDNA 2
Instruction Set Architecture ») ; les instructions non encore décrites lèvent une erreur
qui les nomme, ce qui mesure ce qui reste à traduire.
"""
import struct

from emu import (rdna2, rdna2_exec_cmp64, rdna2_exec_cmpf16, rdna2_exec_dot, rdna2_exec_ext, rdna2_exec_f16,
                 rdna2_exec_format, rdna2_exec_graphics, rdna2_exec_image, rdna2_exec_memory, rdna2_exec_movrel,
                 rdna2_exec_pc, rdna2_exec_vop3, rdna2_exec_wide)
from emu.rdna2_exec_ops import SCALAR, VECTOR, VECTOR_COMPARE, VECTOR_NO_OPERATION, BRANCHES, float_bits

LANES = 32
WAVE_SIZES = (32, 64)
FULL = (1 << LANES) - 1
MASK32 = 0xFFFF_FFFF
SCALAR_REGISTERS = 128              # s0-s105, vcc, ttmp, m0, null, exec
VCC_LO, M0, NULL, EXEC_LO = 106, 124, 125, 126
VGPR_FIRST, LITERAL = 256, 255
INLINE_FLOATS = {240: 0.5, 241: -0.5, 242: 1.0, 243: -1.0, 244: 2.0, 245: -2.0, 246: 4.0, 247: -4.0,
                 248: 0.15915494309189535}
MAX_STEPS = 1_000_000
DECODED_FAMILIES = frozenset({"SOP1", "SOP2", "SOPC", "SOPK", "SOPP", "VOP1", "VOP2", "VOPC"})
MEMORY_FAMILIES = frozenset({"SMEM", "DS", "MUBUF", "FLAT", "GLOBAL", "SCRATCH"})
FORMAT_FAMILIES = frozenset({"MUBUF", "MTBUF"})            # accès typés : emu/rdna2_exec_format.py


class Untranslated(NotImplementedError):
    """Instruction que l'interpréteur ne sait pas encore exécuter."""


class Wave:
    """État d'un wave32 ou wave64. `v[n][ligne]` : VGPR ; `s[n]` : registres scalaires 32 bits.

    En wave64, EXEC et VCC occupent deux SGPR (exec_hi = s127, vcc_hi = s107) et un masque de lignes
    écrit dans un SGPR en occupe deux (documentation RDNA 2 d'AMD, chapitre 3)."""

    def __init__(self, memory=None, lanes=LANES):
        if lanes not in WAVE_SIZES:
            raise ValueError(f"taille de wave non prise en charge : {lanes} (32 ou 64)")
        self.lanes = lanes
        self.mask_width = lanes // 32               # SGPR par masque de lignes
        self.memory = memory if memory is not None else rdna2_exec_memory.Memory()
        self.lds = bytearray(rdna2_exec_memory.LDS_SIZE)
        self.steps = 0
        self.s = [0] * SCALAR_REGISTERS
        self.v = [[0] * lanes for _ in range(VGPR_FIRST)]
        self.write_mask(EXEC_LO, (1 << lanes) - 1)
        self.v[0] = list(range(lanes))     # v0 : numéro de ligne, comme un shader de calcul
        self.scc = False
        self.pc = 0

    @property
    def exec(self):
        return self.read_mask(EXEC_LO)

    @property
    def vcc(self):
        return self.read_mask(VCC_LO)

    def active(self):
        mask = self.exec
        return [lane for lane in range(self.lanes) if mask >> lane & 1]

    def read_mask(self, code, literal=None):
        """Masque de lignes lu dans un SGPR (wave32) ou une paire de SGPR (wave64)."""
        return self.read_scalar(code, literal, self.mask_width)

    def write_mask(self, code, mask):
        """Écrit un masque de lignes : un SGPR en wave32, une paire en wave64."""
        self.write_scalar(code, mask, self.mask_width)

    def read_scalar(self, code, literal, width=1):
        if code == LITERAL:
            return literal
        if code == NULL:
            return 0
        if code < SCALAR_REGISTERS:
            low = self.s[code]
            return low | (self.s[code + 1] << 32 if width == 2 and code + 1 < SCALAR_REGISTERS else 0)
        if 128 <= code <= 192:
            return code - 128
        if 193 <= code <= 208:
            return (192 - code) & (MASK32 if width == 1 else (1 << 64) - 1)
        if code in INLINE_FLOATS and width == 2:
            # ISA RDNA 2 §5.2 (p. 32) : codes 240-248 « single or double floats », 1/(2π) double
            # 0x3fc45f306dc9c882 ; une source 64 bits lit le double (LLVM : isInlinableLiteral64).
            return rdna2_exec_cmp64.double_bits(rdna2_exec_cmp64.INLINE_DOUBLES[code])
        if code in INLINE_FLOATS:
            return float_bits(INLINE_FLOATS[code])
        raise Untranslated(f"opérande scalaire {code} non pris en charge")

    def write_scalar(self, code, value, width=1):
        if code == NULL:
            return
        self.s[code] = value & MASK32
        if width == 2:
            self.s[code + 1] = (value >> 32) & MASK32

    def read_lane(self, code, lane, literal):
        """Source 9 bits d'une instruction vectorielle, pour une ligne."""
        if code >= VGPR_FIRST:
            return self.v[code - VGPR_FIRST][lane]
        return self.read_scalar(code, literal)


def _fields(family, word):
    from emu.rdna2_operands import FIELDS

    return {name: (word >> shift) & mask for name, (shift, mask, _) in FIELDS[family].items()}


def _signed16(value):
    return value - 0x10000 if value & 0x8000 else value


def step(wave, data):
    """Exécute l'instruction à wave.pc. Renvoie False après s_endpgm."""
    family, opcode, size = rdna2.classify(data, wave.pc)
    name = rdna2.mnemonic(family, opcode) or f"{family}#{opcode}"
    if name == "s_endpgm":
        return False
    if not executable(family, name):
        raise Untranslated(f"instruction non traduite : {name} ({family}, opcode 0x{opcode:X})")
    wave.steps += 1
    if family in rdna2_exec_graphics.FAMILIES:
        word = int.from_bytes(data[wave.pc:wave.pc + 2 * rdna2.DWORD].ljust(8, b"\0"), "little")
        rdna2_exec_graphics.execute(wave, family, name, word)
        wave.pc += size * rdna2.DWORD
        return True
    if family in rdna2_exec_image.FAMILIES:
        rdna2_exec_image.execute(wave, name, int.from_bytes(data[wave.pc:wave.pc + size * rdna2.DWORD], "little"))
        wave.pc += size * rdna2.DWORD
        return True
    if family in FORMAT_FAMILIES and rdna2_exec_format.supports(name):
        rdna2_exec_format.execute(wave, name, int.from_bytes(data[wave.pc:wave.pc + 2 * rdna2.DWORD], "little"))
        wave.pc += size * rdna2.DWORD
        return True
    if family in MEMORY_FAMILIES:
        word = int.from_bytes(data[wave.pc:wave.pc + 2 * rdna2.DWORD], "little")
        rdna2_exec_memory.execute(wave, family, name, word)
        wave.pc += size * rdna2.DWORD
        return True
    if family == "VOP3":
        word = int.from_bytes(data[wave.pc:wave.pc + 2 * rdna2.DWORD], "little")
        literal = struct.unpack_from("<I", data, wave.pc + 2 * rdna2.DWORD)[0] if size > 2 else None
        rdna2_exec_vop3.execute(wave, name, opcode, word, literal)
        wave.pc += size * rdna2.DWORD
        return True
    if family == "VOP3P":
        word = int.from_bytes(data[wave.pc:wave.pc + 2 * rdna2.DWORD], "little")
        literal = struct.unpack_from("<I", data, wave.pc + 2 * rdna2.DWORD)[0] if size > 2 else None
        rdna2_exec_dot.execute(wave, name, word, literal)
        wave.pc += size * rdna2.DWORD
        return True
    word = struct.unpack_from("<I", data, wave.pc)[0]
    literal = struct.unpack_from("<I", data, wave.pc + rdna2.DWORD)[0] if size > 1 else None
    fields = _fields(family, word)
    next_pc = wave.pc + size * rdna2.DWORD
    if name in BRANCHES:
        if BRANCHES[name](wave):
            next_pc = wave.pc + rdna2.DWORD + _signed16(fields["simm16"]) * rdna2.DWORD
    elif rdna2_exec_pc.FAMILIES.get(name) == family:
        next_pc = rdna2_exec_pc.execute(wave, name, fields, next_pc, len(data))
    elif name in rdna2_exec_movrel.SCALAR and family == "SOP1":
        rdna2_exec_movrel.scalar(wave, name, fields, literal)
    elif name in SCALAR:
        SCALAR[name](wave, fields, literal)
    elif name in VECTOR_NO_OPERATION:
        pass
    elif name in rdna2_exec_wide.OPERATIONS and family == "VOP1":
        if rdna2_exec_ext.is_ext(family, fields.get("src0")):
            raise Untranslated(f"instruction non traduite : {name} (SDWA/DPP)")
        rdna2_exec_wide.execute(wave, name, (fields["src0"],), fields["vdst"], literal)
    elif name in rdna2_exec_cmp64.COMPARE64 and family == "VOPC":
        rdna2_exec_cmp64.execute(wave, name, (fields["src0"], VGPR_FIRST + fields["vsrc1"]), literal)
    elif name in rdna2_exec_cmpf16.COMPAREF16 and family == "VOPC":   # before is_ext: refuses SDWA/DPP
        rdna2_exec_cmpf16.execute(wave, name, fields, literal)
    elif name in rdna2_exec_movrel.VECTOR and family == "VOP1":
        if rdna2_exec_ext.is_ext(family, fields.get("src0")):
            raise Untranslated(f"instruction non traduite : {name} (SDWA/DPP)")
        rdna2_exec_movrel.vector(wave, name, fields["vdst"], fields["src0"], literal)
    elif name in VOP2_CARRY and family == "VOP2":
        if rdna2_exec_ext.is_ext(family, fields.get("src0")):
            raise Untranslated(f"instruction non traduite : {name} (SDWA/DPP)")
        _carry(wave, name, fields, literal)
    elif name in rdna2_exec_f16.OPERATIONS:     # avant is_ext : les formes SDWA/DPP du f16 sont refusées par rdna2_exec_f16
        rdna2_exec_f16.execute(wave, name, fields, literal)
    elif rdna2_exec_ext.is_ext(family, fields.get("src0")):
        rdna2_exec_ext.execute(wave, family, name, fields, literal or 0)
    elif name in VECTOR:
        _vector(wave, name, fields, literal)
    elif name in VECTOR_COMPARE:
        _compare(wave, name, fields, literal)
    else:
        raise Untranslated(f"instruction non traduite : {name} ({family}, opcode 0x{opcode:X})")
    wave.pc = next_pc
    return True


def _vector(wave, name, fields, literal):
    operation = VECTOR[name]
    if name == "v_readfirstlane_b32":
        lanes = wave.active() or [0]
        wave.write_scalar(fields["sdst"], wave.read_lane(fields["src0"], lanes[0], literal))
        return
    destination = wave.v[fields["vdst"]]
    for lane in wave.active():
        a = wave.read_lane(fields["src0"], lane, literal)
        b = wave.v[fields["vsrc1"]][lane] if "vsrc1" in fields else 0
        destination[lane] = operation(a, b, destination[lane], literal, wave.vcc >> lane & 1) & MASK32


def _carry(wave, name, fields, literal):
    """Forme VOP2 de v_add/sub/subrev_co_ci_u32 : retenue lue et écrite dans VCC.

    Les bits des lignes inactives sont mis à 0. Source RDNA 2 : AMD, « "RDNA 2" Instruction Set
    Architecture », novembre 2020, section 3.9 « Vector Compares: VCC and VCCZ » (p. 19-20) :
    « The VCC indicates which executing threads passed the conditional test, or which threads
    generated a carry-out from an integer add or subtract » et « VCC is fully written; there are no
    partial mask updates ». (PDF archivé : web.archive.org, capture 20230214072857 de
    developer.amd.com/wp-content/resources/RDNA2_Shader_ISA_November2020.pdf, SHA-256 22aec03f…4795.)

    Mesuré sur RDNA 2 par Lyrnc (iGPU gfx1036, Wave32, pilote AMD 32.0.21042.62, 3 essais sur 3) sur
    le cas de la revue de #133 : EXEC = 1, retenue préchargée à 0xFFFFFFFE → 0x00000001 pour
    v_add_co_ci_u32 en VOP2 (VCC) et en VOP3 (SGPR) ; github.com/akimfomin/PortBridge/issues/19
    (commentaire 7136020663). Portée : ces deux additions, en Wave32, sur gfx1036.
    """
    carries = 0
    for lane in wave.active():
        a, b = wave.read_lane(fields["src0"], lane, literal), wave.v[fields["vsrc1"]][lane]
        result, carry = rdna2_exec_vop3.CARRY[name](a, b, wave.vcc >> lane & 1)
        wave.v[fields["vdst"]][lane] = result & MASK32
        carries |= bool(carry) << lane
    wave.write_mask(VCC_LO, carries)


def _compare(wave, name, fields, literal):
    test, writes_exec = VECTOR_COMPARE[name]
    mask = 0
    for lane in wave.active():
        if test(wave.read_lane(fields["src0"], lane, literal), wave.v[fields["vsrc1"]][lane]):
            mask |= 1 << lane
    wave.write_mask(EXEC_LO if writes_exec else VCC_LO, mask)


def run(data, wave=None, max_steps=MAX_STEPS):
    """Exécute un programme jusqu'à s_endpgm et renvoie l'état final du wave."""
    wave = wave or Wave()
    for _ in range(max_steps):
        if wave.pc + rdna2.DWORD > len(data) or not step(wave, data):
            return wave
    raise RuntimeError(f"programme interrompu après {max_steps} instructions (boucle infinie ?)")


VOP2_CARRY = frozenset({"v_add_co_ci_u32", "v_sub_co_ci_u32", "v_subrev_co_ci_u32"})
EXECUTABLE = frozenset({"s_endpgm", *SCALAR, *VECTOR, *VECTOR_COMPARE, *VECTOR_NO_OPERATION, *BRANCHES,
                        *VOP2_CARRY, *rdna2_exec_cmp64.COMPARE64, *rdna2_exec_wide.OPERATIONS,
                        *rdna2_exec_movrel.VECTOR, *rdna2_exec_movrel.SCALAR, *rdna2_exec_pc.OPERATIONS,
                        *rdna2_exec_f16.OPERATIONS, *rdna2_exec_cmpf16.COMPAREF16})


def executable(family, name):
    """Vrai si l'interpréteur exécute cette instruction dans cet encodage."""
    if family == "VOP3":
        return name in rdna2_exec_vop3.EXECUTABLE
    if family == "VOP3P":
        return rdna2_exec_dot.supports(name)
    if family in rdna2_exec_graphics.FAMILIES:
        return rdna2_exec_graphics.supports(family, name)
    if family in rdna2_exec_image.FAMILIES:
        return rdna2_exec_image.supports(family, name)
    if family in FORMAT_FAMILIES and rdna2_exec_format.supports(name):
        return True
    if family in MEMORY_FAMILIES:
        return rdna2_exec_memory.supports(name)
    return family in DECODED_FAMILIES and name in EXECUTABLE


def coverage():
    """(encodages exécutables, encodages RDNA 2 connus), un par (famille, opcode)."""
    from emu.rdna2_ops import MNEMONICS

    done = sum(executable(family, name) for (family, _), name in MNEMONICS.items())
    return done, len(MNEMONICS)
