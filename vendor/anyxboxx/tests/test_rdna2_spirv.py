"""Traducteur RDNA 2 → SPIR-V de Leonida (third_party/leonida), appelé sur notre désassemblage."""
import pytest

from emu import rdna2_operands, rdna2_spirv
from emu.rdna2_hlsl_ops import Untranslatable
from test_rdna2_exec import program, sopp, vop1
from test_rdna2_graphics import PIXEL_SHADER, exp, vintrp

VERTEX_SHADER = program(vop1("v_cvt_f32_u32", 1, 256), vop1("v_mov_b32", 2, 0xF0), vop1("v_mov_b32", 3, 128),
                        vop1("v_mov_b32", 4, 0xF2), *exp(12, (1, 2, 3, 4), done=True), *exp(32, (2, 2, 3, 4)))
needs_tools = pytest.mark.skipif(rdna2_spirv.tools() is None, reason="spirv-as / spirv-val absents (SDK Vulkan)")


def test_vintrp_text_matches_llvm():
    assert rdna2_operands.text(bytes((0x00, 0x00, 0x04, 0xC8))) == "v_interp_p1_f32_e32 v1, v0, attr0.x"
    assert rdna2_operands.text(bytes((0x00, 0x00, 0x02, 0xC8))) == "v_interp_mov_f32_e32 v0, p10, attr0.x"
    assert rdna2_operands.text(bytes(vintrp("v_interp_p2_f32", 3, 1, 2, 3)[0].to_bytes(4, "little"))) == \
        "v_interp_p2_f32_e32 v3, v1, attr2.w"


def test_disassembly_is_the_llvm_listing_leonida_reads():
    listing = rdna2_spirv.disassembly(PIXEL_SHADER).splitlines()
    assert listing[0] == "\tv_interp_p1_f32_e32 v2, v0, attr0.x ; encoding: [0x00,0x00,0x08,0xc8]"
    assert listing[-1] == "\ts_endpgm ; encoding: [0x00,0x00,0x81,0xbf]"


@pytest.mark.parametrize("data, stage, entry, interface", [
    (PIXEL_SHADER, "ps", "OpEntryPoint Fragment", ("%out_mrt0", "%in_attr0")),
    (VERTEX_SHADER, "vs", "OpEntryPoint Vertex", ("%out_pos", "%out_param0", "%vertex_index")),
])
def test_leonida_translates_our_shaders(data, stage, entry, interface):
    asm = rdna2_spirv.translate(data, stage)
    line = next(text for text in asm.splitlines() if entry in text)
    assert all(name in line for name in interface)


def test_refusals_are_named():
    with pytest.raises(Untranslatable, match="pas de texte LLVM"):
        rdna2_spirv.disassembly(program(0x7E0A02FA, 0x00013001))         # DPP : dpp_ctrl 0x130 réservé
    with pytest.raises(ValueError, match="étage inconnu"):
        rdna2_spirv.translate(PIXEL_SHADER, "hs")


@needs_tools
@pytest.mark.parametrize("data, stage", [(PIXEL_SHADER, "ps"), (VERTEX_SHADER, "vs")])
def test_spirv_is_valid(data, stage):
    assert rdna2_spirv.validate(rdna2_spirv.translate(data, stage))[:4] == b"\x03\x02\x23\x07"


COMPUTE_SHADER = program(vop1("v_mov_b32", 1, 0xF2))            # v_mov_b32 v1, 1.0 puis s_endpgm
GROUP = (64, 1, 1)


def test_compute_shader_takes_its_dispatch_state():
    """Leonida lit rsrc2 et la taille du groupe dans l'en-tête FSSM ; sans FSSM, on la lui donne."""
    asm = rdna2_spirv.translate(COMPUTE_SHADER, "cs", dispatch=(0, *GROUP))
    assert "OpEntryPoint GLCompute" in asm and "LocalSize 64 1 1" in asm


def test_compute_shader_reads_a_real_fssm_header():
    header = bytearray(0x148)
    header[0x138:0x148] = (0).to_bytes(4, "little") + b"".join(n.to_bytes(4, "little") for n in (8, 8, 1))
    assert "LocalSize 8 8 1" in rdna2_spirv.translate(COMPUTE_SHADER, "cs", fssm=bytes(header))


def test_compute_shader_without_dispatch_state_is_refused():
    with pytest.raises(Untranslatable, match="état de dispatch"):
        rdna2_spirv.translate(COMPUTE_SHADER, "cs")


DATA_WORD = b"\x00\x00\x00\x00"                                   # mot de données entre deux programmes
# Données intégrées au code, sautées par un s_branch : le chemin exécuté ne les traverse jamais.
CODE_WITH_DATA = program(sopp("s_branch", 1), 0, vop1("v_mov_b32", 1, 0xF2))


def test_data_words_are_skipped_like_llvm_mc_when_the_fssm_is_known():
    """Avec la position du code dans le FSSM, un mot sans texte est sauté et signalé, comme llvm-mc."""
    code = COMPUTE_SHADER[:4] + DATA_WORD + COMPUTE_SHADER[4:]
    listing = rdna2_spirv.disassembly(code, code_start=0x200).splitlines()
    assert listing[0] == "; programme  code 0x200..0x20c"
    assert [line.split(" ;")[0].strip() for line in listing[1:3]] == ["v_mov_b32_e32 v1, 1.0", "s_endpgm"]
    assert listing[3:] == ["; --- llvm-mc messages ---", "; invalid instruction encoding"]
    with pytest.raises(Untranslatable, match="pas de texte LLVM"):
        rdna2_spirv.disassembly(code)                                 # sans FSSM : refus, comme avant


def test_leonida_reanchors_on_the_fssm_past_data_words():
    code = CODE_WITH_DATA
    header = bytearray(0x200)
    header[0x138:0x148] = b"".join(n.to_bytes(4, "little") for n in (0, 64, 1, 1))
    asm = rdna2_spirv.translate(code, "cs", fssm=bytes(header) + code, code_start=0x200)
    assert "OpEntryPoint GLCompute" in asm


def test_leonida_refuses_to_run_into_data_words():
    """Un chemin qui tombe sur un mot sans texte est refusé et nommé, pas tronqué (règle 3)."""
    code = COMPUTE_SHADER[:4] + DATA_WORD + COMPUTE_SHADER[4:]
    header = bytearray(0x200)
    header[0x138:0x148] = b"".join(n.to_bytes(4, "little") for n in (0, 64, 1, 1))
    with pytest.raises(Untranslatable, match="fall-through into undecodable words"):
        rdna2_spirv.translate(code, "cs", fssm=bytes(header) + code, code_start=0x200)
