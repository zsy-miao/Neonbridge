"""Instructions graphiques de l'interpréteur RDNA 2 : interpolation (VINTRP) et exports (EXP).

Conventions (zones mémoire, entrées v0/v1) : emu/rdna2_graphics.py. Sémantique (AMD, « RDNA 2
Instruction Set Architecture ») : v_interp_p1_f32 D = P10 × S + P0 ; v_interp_p2_f32
D = P20 × S + D ; v_interp_mov_f32 D = P10, P20 ou P0. Produit et somme sont arrondis
séparément en float32, comme le HLSL généré (`precise`).
"""
from emu import rdna2_graphics as graphics
from emu.rdna2_exec_ops import bits_float, float_bits

FAMILIES = frozenset({"VINTRP", "EXP"})
SUPPORTED = frozenset({*graphics.INTERP_OPS.values(), "exp"})


def supports(family, name):
    return family in FAMILIES and name in SUPPORTED


def _multiply_add(a, b, c):
    return float_bits(bits_float(float_bits(bits_float(a) * bits_float(b))) + bits_float(c))


def _interpolate(wave, name, fields):
    from emu.rdna2_exec import Untranslated

    if fields["attr"] >= graphics.PARAMETER_COUNT:
        raise Untranslated(f"{name} : attribut {fields['attr']} au-delà de param{graphics.PARAMETER_COUNT - 1}")
    primitives = getattr(wave, "primitives", None)
    cache = {}
    destination, source = wave.v[fields["vdst"]], fields["vsrc"]
    for lane in wave.active():
        primitive = primitives.get(lane, wave.primitive) if primitives is not None else wave.primitive
        if primitive not in cache:
            address = graphics.parameter_address(primitive, fields["attr"], fields["chan"])
            cache[primitive] = wave.memory.read_words(address, 3)
        p0, p10, p20 = cache[primitive]
        if name == "v_interp_p1_f32":
            destination[lane] = _multiply_add(p10, wave.v[source][lane], p0)
        elif name == "v_interp_p2_f32":
            destination[lane] = _multiply_add(p20, wave.v[source][lane], destination[lane])
        else:
            destination[lane] = {0: p10, 1: p20, 2: p0}.get(source, 0)


def _export(wave, fields):
    from emu.rdna2_exec import Untranslated

    if fields["compr"]:
        raise Untranslated(f"exp {fields['name']} compr : export compressé (16 bits) non pris en charge")
    for lane in wave.active():
        values = [wave.v[register][lane] if fields["en"] >> channel & 1 else 0
                  for channel, register in enumerate(fields["sources"])]
        wave.exports.setdefault(fields["target"], {})[lane] = values


def execute(wave, family, name, word):
    """Exécute une instruction VINTRP (`word` 32 bits utiles) ou EXP (`word` 64 bits)."""
    if family == "VINTRP":
        _interpolate(wave, name, graphics.vintrp_fields(word))
    else:
        _export(wave, graphics.exp_fields(word))


def _single_lane_wave(sgprs, memory):
    from emu import rdna2_exec

    wave = rdna2_exec.Wave()
    wave.memory.write(0, memory)
    wave.s[rdna2_exec.EXEC_LO] = 1
    for number, value in (sgprs or {}).items():
        wave.s[number] = value
    wave.exports, wave.primitive = {}, 0
    return wave


def run_vertex(data, vertex, sgprs, memory):
    """Exports {cible: [4 mots]} du vertex shader pour un sommet (v0 = numéro du sommet)."""
    from emu import rdna2_exec

    wave = _single_lane_wave(sgprs, memory)
    wave.v[0][0] = vertex
    rdna2_exec.run(data, wave)
    return {target: lanes[0] for target, lanes in wave.exports.items()}


def run_pixel_wave(data, pixels, primitive, sgprs, memory, exec_mask=None):
    """Exports {cible: {ligne: [4 mots]}} du pixel shader pour un wave32 (1 à 32 pixels).

    `pixels` : liste [(i, j), ...] (lignes 0..n-1) ou dict {ligne: (i, j)}.
    `primitive` : indice de primitive commun (int), ou par ligne (dict {ligne: prim} ou liste).
    `exec_mask` : masque EXEC initial (par défaut les bits des lignes présentes dans `pixels`)."""
    from emu import rdna2_exec

    mapping = pixels if isinstance(pixels, dict) else dict(enumerate(pixels))
    if not mapping or any(not 0 <= lane < rdna2_exec.LANES for lane in mapping):
        raise ValueError(f"un wave de pixels contient entre 1 et {rdna2_exec.LANES} lignes valides")
    wave = rdna2_exec.Wave()
    wave.memory.write(0, memory)
    default_mask = 0
    for lane, (i_bits, j_bits) in mapping.items():
        wave.v[0][lane], wave.v[1][lane] = i_bits, j_bits
        default_mask |= 1 << lane
    wave.s[rdna2_exec.EXEC_LO] = default_mask if exec_mask is None else (exec_mask & 0xFFFF_FFFF)
    for number, value in (sgprs or {}).items():
        wave.s[number] = value
    wave.exports = {}
    if isinstance(primitive, dict):
        wave.primitive, wave.primitives = 0, dict(primitive)
    elif isinstance(primitive, (list, tuple)):
        wave.primitive, wave.primitives = 0, dict(enumerate(primitive))
    else:
        wave.primitive, wave.primitives = primitive, None
    rdna2_exec.run(data, wave)
    return {target: dict(lanes) for target, lanes in wave.exports.items()}


def run_pixel(data, i_bits, j_bits, primitive, sgprs, memory):
    """Exports {cible: [4 mots]} du pixel shader pour un pixel (v0 = i, v1 = j)."""
    exports = run_pixel_wave(data, [(i_bits, j_bits)], primitive, sgprs, memory)
    return {target: lanes[0] for target, lanes in exports.items()}
