"""Recompilation RDNA 2 → HLSL des vertex et pixel shaders (conventions : emu/rdna2_graphics.py).

Chaque invocation est un wave d'une ligne : `lane` vaut 0, EXEC vaut 1 et les opérations entre
lignes (ballot, readlane) se réduisent à la ligne elle-même. La LDS (DS) n'existe pas dans
ces étages : une instruction DS lève Untranslatable, comme tout export non pris en charge.
"""
from emu import (rdna2, rdna2_graphics as graphics, rdna2_hlsl_div, rdna2_hlsl_dot, rdna2_hlsl_f16, rdna2_hlsl_format,
                 rdna2_hlsl_image, rdna2_hlsl_sad, rdna2_hlsl_wide)
from emu.rdna2_hlsl import body
from emu.rdna2_hlsl_ops import HELPERS as ALU_HELPERS, Untranslatable

HELPERS = "\n".join((ALU_HELPERS, rdna2_hlsl_wide.HELPERS, rdna2_hlsl_div.HELPERS, rdna2_hlsl_image.HELPERS,
                     rdna2_hlsl_format.HELPERS, rdna2_hlsl_dot.HELPERS,
                     rdna2_hlsl_sad.HELPERS, rdna2_hlsl_f16.HELPERS))
ACTIVE = "((s[126] >> lane) & 1u) != 0"
FINISH = "pc = END_PC; break;"

COMMON = """// Généré par AnyXboxX (emu/rdna2_hlsl_graphics.py) : {stage} RDNA 2 recompilé en HLSL.
#define END_PC 0xFFFFFFFFu
RWByteAddressBuffer g_memory : register(u0);

{wave_helpers}
{images}
"""
SINGLE_LANE_HELPERS = """uint Ballot(uint lane, bool bit) { return bit ? 1u : 0u; }            // wave d'une ligne
uint ReadLane(uint lane, uint value, uint other) { return value; }"""
# SM 6.6 (ps_6_6) : `!IsHelperLane()` exclut les helper lanes des quads partiellement couverts
# de `s[126]` (EXEC) et de chaque `Ballot()` (appelé par rdna2_hlsl.py avec `t = active && cond`).
# Sur un wave de 64 lignes (ou plus), `WaveGetLaneIndex() >> 5` sélectionne la tranche de 32 lignes
# correspondante (`b.x` pour 0..31, `b.y` pour 32..63, etc.) et `ReadLane` ajoute la base
# `WaveGetLaneIndex() & ~31u`, sans recouvrement entre les lignes 32..63 et 0..31.
MULTI_LANE_HELPERS = """uint WaveBallot32(bool bit) {
  uint4 b = WaveActiveBallot(bit && !IsHelperLane());
  uint h = WaveGetLaneIndex() >> 5;
  return h == 0u ? b.x : (h == 1u ? b.y : (h == 2u ? b.z : b.w));
}
uint Ballot(uint lane, bool bit) { return WaveBallot32(bit); }
uint ReadLane(uint lane, uint value, uint other) {
  uint base = WaveGetLaneIndex() & ~31u;
  return WaveReadLaneAt(value, base + (other & 31u));
}"""
REGISTERS = """  uint lane = 0u;
  uint s[128];
  uint v[256];
  uint s_null = 0u;
  bool scc = false;
  for (uint i = 0; i < 128; ++i) s[i] = 0u;
  for (uint j = 0; j < 256; ++j) v[j] = 0u;
  s[126] = 1u;"""
MULTI_LANE_REGISTERS = """  uint lane = WaveGetLaneIndex() & 31u;
  uint s[128];
  uint v[256];
  uint s_null = 0u;
  bool scc = false;
  for (uint i = 0; i < 128; ++i) s[i] = 0u;
  for (uint j = 0; j < 256; ++j) v[j] = 0u;
  s[126] = WaveBallot32(true);"""
CROSS_LANE_OPS = frozenset({"v_readlane_b32", "v_readfirstlane_b32", "v_writelane_b32",
                            "v_mbcnt_lo_u32_b32", "v_mbcnt_hi_u32_b32", "s_wqm_b32",
                            "global_load_dword_addtid", "global_store_dword_addtid",     # lane id in the address
                            "ds_read_addtid_b32", "ds_write_addtid_b32"})


def _uses_wave_ops(data):
    """Vrai si le shader utilise des opérations inter-lignes (ballot, readlane, masques EXEC)."""
    for instruction in rdna2.decode(data):
        if instruction.family == "VOPC":
            return True
        name = rdna2.mnemonic(instruction.family, instruction.opcode) or ""
        if name in CROSS_LANE_OPS or name.startswith(("v_cmp_", "v_cmpx_", "s_and_saveexec", "s_or_saveexec",
                                                      "s_andn2_saveexec", "s_xor_saveexec")):
            return True
    return False


def _sources(fields):
    values = [f"v[{register}]" if fields["en"] >> channel & 1 else "0u"
              for channel, register in enumerate(fields["sources"])]
    return f"uint4({', '.join(values)})"


def _export(stage, fields):
    if fields["compr"]:
        raise Untranslatable(f"exp {fields['name']} compr : export compressé (16 bits) non traduit")
    value, target = _sources(fields), fields["target"]
    if stage == "vs":
        slot = graphics.vertex_slot(target)
        if slot is None:
            raise Untranslatable(f"exp {fields['name']} : cible non prise en charge dans un vertex shader")
        position = " o_out.position = asfloat(e);" if slot == 0 else ""
        store = f"g_memory.Store4(record + {slot * graphics.CHANNELS * graphics.DWORD}u, e);"
    elif target < graphics.RENDER_TARGETS:
        position = f" o_out.t{target} = asfloat(e);"
        offset = (graphics.PIXEL_HEADER + target * graphics.CHANNELS) * graphics.DWORD
        store = f"g_memory.Store4(record + {offset}u, e);"
    elif fields["name"] == "null":
        return []
    else:
        raise Untranslatable(f"exp {fields['name']} : cible non prise en charge dans un pixel shader")
    return [f"if ({ACTIVE}) {{ uint4 e = {value}; {store}{position} }}"]


def _interpolate(name, fields):
    if fields["attr"] >= graphics.PARAMETER_COUNT:
        raise Untranslatable(f"{name} : attribut {fields['attr']} au-delà de param{graphics.PARAMETER_COUNT - 1}")
    offset = graphics.parameter_address(0, fields["attr"], fields["chan"])
    load = f"uint3 p = g_memory.Load3({offset}u + primitive * {graphics.PRIMITIVE_STRIDE}u);"
    source, target = f"v[{fields['vsrc']}]", f"v[{fields['vdst']}]"
    if name == "v_interp_p1_f32":
        math = f"precise float m = asfloat(p.y) * asfloat({source}); precise float r = m + asfloat(p.x);"
    elif name == "v_interp_p2_f32":
        math = f"precise float m = asfloat(p.z) * asfloat({source}); precise float r = m + asfloat({target});"
    else:                                       # v_interp_mov_f32 : copie exacte des bits
        chosen = {0: "p.y", 1: "p.z", 2: "p.x"}.get(fields["vsrc"], "0u")
        return [f"if ({ACTIVE}) {{ {load} {target} = {chosen}; }}"]
    return [f"if ({ACTIVE}) {{ {load} {math} {target} = asuint(r); }}"]


def _special(stage):
    def translate(family, name, word):
        if family == "DS":
            raise Untranslatable(f"non traduite dans un {stage.upper()} : {name} (pas de LDS dans cet étage)")
        if family == "EXP":
            return _export(stage, graphics.exp_fields(word))
        if family == "VINTRP":
            if stage != "ps":
                raise Untranslatable(f"non traduite dans un VS : {name} (interpolation de pixel shader)")
            return _interpolate(name, graphics.vintrp_fields(word))
        return None
    return translate


def render_targets(data):
    """Cibles mrt0-7 exportées par le programme (pour la structure de sortie du PS)."""
    found = set()
    for instruction in rdna2.decode(data):
        if instruction.family == "EXP":
            word = int.from_bytes(data[instruction.offset:instruction.offset + 8], "little")
            target = graphics.exp_fields(word)["target"]
            if target < graphics.RENDER_TARGETS:
                found.add(target)
    return sorted(found)


def translate_vertex(data, sgprs=None):
    """Vertex shader HLSL (vs_6_0) : v0 = SV_VertexID, exports enregistrés dans VERTEX_RECORDS."""
    lines = [COMMON.format(stage="vertex shader", wave_helpers=SINGLE_LANE_HELPERS,
                           images=HELPERS),
             "struct VSOut { float4 position : SV_Position; float2 bary : TEXCOORD0; };",
             "VSOut main(uint vertex : SV_VertexID) {", REGISTERS,
             "  VSOut o_out;", "  o_out.position = float4(0.0f, 0.0f, 0.0f, 1.0f);",
             "  uint corner = vertex % 3u;     // i et j du rastériseur : (0,0), (1,0), (0,1)",
             "  o_out.bary = float2(corner == 1u ? 1.0f : 0.0f, corner == 2u ? 1.0f : 0.0f);",
             f"  uint record = {graphics.VERTEX_RECORDS}u + vertex * {graphics.VERTEX_STRIDE}u;",
             "  v[0] = vertex;"]
    lines += body(data, sgprs, "return o_out;", _special("vs"))
    return "\n".join(lines + ["  return o_out;", "}", ""])


def translate_pixel(data, width, sgprs=None, multi_lane=None):
    """Pixel shader HLSL (ps_6_0) : v0 = i, v1 = j ; entrées et exports dans PIXEL_RECORDS.

    `multi_lane` : active les intrinsèques SM 6.0 (`WaveActiveBallot`, `WaveReadLaneAt`,
    `WaveGetLaneIndex`) ; par défaut, détecté automatiquement si le shader emploie ballot/readlane."""
    wave_mode = _uses_wave_ops(data) if multi_lane is None else bool(multi_lane)
    targets = render_targets(data)
    outputs = " ".join(f"float4 t{t} : SV_Target{t};" for t in targets)
    returned = "PSOut" if targets else "void"
    lines = [COMMON.format(stage="pixel shader",
                           wave_helpers=MULTI_LANE_HELPERS if wave_mode else SINGLE_LANE_HELPERS,
                           images=HELPERS)]
    if targets:
        lines.append(f"struct PSOut {{ {outputs} }};")
    lines += [f"{returned} main(float4 position : SV_Position, float2 bary : TEXCOORD0, "
              "uint primitive : SV_PrimitiveID) {",
              MULTI_LANE_REGISTERS if wave_mode else REGISTERS]
    if targets:
        lines += ["  PSOut o_out;"] + [f"  o_out.t{t} = float4(0.0f, 0.0f, 0.0f, 0.0f);" for t in targets]
    lines += [f"  uint record = {graphics.PIXEL_RECORDS}u + (uint(position.y) * {width}u + uint(position.x)) * "
              f"{graphics.PIXEL_STRIDE}u;",
              "  v[0] = asuint(bary.x);", "  v[1] = asuint(bary.y);",
              "  if (!IsHelperLane()) g_memory.Store4(record, uint4(1u, v[0], v[1], primitive));"
              if wave_mode else "  g_memory.Store4(record, uint4(1u, v[0], v[1], primitive));"]
    lines += body(data, sgprs, "return o_out;" if targets else "return;", _special("ps"))
    return "\n".join(lines + ["  return o_out;" if targets else "  return;", "}", ""])
