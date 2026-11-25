"""Instructions MIMG dans l'interpréteur RDNA 2 : lecture/écriture et échantillonnage 2D.

Descripteurs et cas pris en charge : emu/rdna2_image.py. Couvre image_load[_mip],
image_store[_mip], image_get_resinfo, image_sample[_lz|_l][_o], image_sample_c[_lz|_l][_o] et
image_gather4[_lz|_l][_o] sur 6 formats 2D (8_UNORM, 8_8_UNORM, 8_8_8_8_UNORM, 32_FLOAT,
32_32_FLOAT, 32_32_32_32_FLOAT) avec filtrage au plus proche ou bilinéaire et adressage wrap,
mirror, clamp-to-edge, mirror-once ou bordure. Le HLSL généré (emu/rdna2_hlsl_image.py) fait
exactement le même calcul, au bit près.
"""
import math

from emu import rdna2_image as images
from emu.rdna2_exec_ops import MASK32, bits_float, float_bits

FAMILIES = frozenset({"MIMG"})
# Bornes float32 symétriques laissant une marge >= 32 pour les décalages _o (-32..+31) sans déborder int32.
INT_MIN, INT_MAX_FLOAT = -2147483520.0, 2147483520.0
BORDER_COLORS = {
    images.BORDER_TRANS_BLACK: [0, 0, 0, 0],
    images.BORDER_OPAQUE_BLACK: [0, 0, 0, images.ONE_BITS],
    images.BORDER_OPAQUE_WHITE: [images.ONE_BITS, images.ONE_BITS, images.ONE_BITS, images.ONE_BITS],
}


def supports(family, name):
    return family in FAMILIES and name in images.SUPPORTED_OPS


def _f32(value):
    return bits_float(float_bits(value))


def _unpack_offset(word):
    """Décalages signés 6 bits (off_x, off_y) dans vaddr[0] d'une forme _o (RDNA 2 Table 8.9)."""
    off_x = (word & 0x3F) - (64 if word & 0x20 else 0)
    off_y = ((word >> 8) & 0x3F) - (64 if word & 0x2000 else 0)
    return off_x, off_y


def _use_linear(sampler, lod_bits=None):
    """Sélectionne min_filter si lod > 0.0 (formes _l), sinon mag_filter."""
    if lod_bits is not None and bits_float(lod_bits) > 0.0:
        return sampler["min_filter"] == images.FILTER_LINEAR
    return sampler["mag_filter"] == images.FILTER_LINEAR


def clamp_index(index, size, mode):
    """Indice de texel après adressage, ou -1 si hors image en mode bordure."""
    if mode == images.CLAMP_WRAP:
        return index % size
    if mode == images.CLAMP_MIRROR:
        period = 2 * size
        pos = index % period
        return pos if pos < size else period - 1 - pos
    if mode == images.CLAMP_LAST_TEXEL:
        return min(max(index, 0), size - 1)
    if mode == images.CLAMP_MIRROR_ONCE_LAST_TEXEL:
        if index < 0:
            index = -1 - index
        return min(index, size - 1)
    if mode == images.CLAMP_MIRROR_ONCE_BORDER:
        if index < 0:
            index = -1 - index
        return index if index < size else -1
    return index if 0 <= index < size else -1


def texel_index(bits, size, unnormalized, mode, offset=0):
    """Indice de texel (filtrage au plus proche) sur un axe ; -1 si hors image en mode bordure."""
    value = bits_float(bits)
    if not unnormalized:
        value = _f32(value * size)
    if math.isnan(value):
        return clamp_index(offset, size, mode)
    index = int(math.floor(min(max(value, INT_MIN), INT_MAX_FLOAT))) + offset
    return clamp_index(index, size, mode)


def bilinear_axis(bits, size, unnormalized, offset=0):
    """(i0, i1, frac) sur un axe pour le filtrage bilinéaire ou gather4 ; NaN → (off, off, 0.0)."""
    value = bits_float(bits)
    if not unnormalized:
        value = _f32(value * size)
    if math.isnan(value):
        return offset, offset, 0.0
    shifted = _f32(value - 0.5)
    clamped = min(max(shifted, INT_MIN), INT_MAX_FLOAT)
    fl = math.floor(clamped)
    frac = _f32(clamped - fl)
    i0 = int(fl) + offset
    return i0, i0 + 1, frac


def texel(memory, image, x, y):
    """Les 4 canaux (bits float32) du texel (x, y)."""
    fmt = image["format"]
    address = image["address"] + (y * image["width"] + x) * images.TEXEL_BYTES[fmt]
    if fmt == images.FORMAT_32_32_32_32_FLOAT:
        return list(memory.read_words(address, 4))
    if fmt == images.FORMAT_32_32_FLOAT:
        r, g = memory.read_words(address, 2)
        return [r, g, 0, images.ONE_BITS]
    if fmt == images.FORMAT_32_FLOAT:
        return [memory.read_words(address, 1)[0], 0, 0, images.ONE_BITS]
    if fmt == images.FORMAT_8_8_8_8_UNORM:
        word = memory.read_words(address, 1)[0]
        return [images.UNORM8[(word >> (8 * channel)) & 0xFF] for channel in range(4)]
    if fmt == images.FORMAT_8_8_UNORM:
        raw = memory.read(address, 2)
        return [images.UNORM8[raw[0]], images.UNORM8[raw[1]], 0, images.ONE_BITS]
    raw = memory.read(address, 1)
    return [images.UNORM8[raw[0]], 0, 0, images.ONE_BITS]


def fetch_texel(memory, image, x, y, border_color=images.BORDER_TRANS_BLACK):
    """Texel (x, y) ou couleur de bordure si un indice vaut -1."""
    if x < 0 or y < 0:
        return list(BORDER_COLORS[border_color])
    return texel(memory, image, x, y)


def swizzled(channels, swizzle):
    selected = {images.SELECT_ZERO: 0, images.SELECT_ONE: images.ONE_BITS}
    return [channels[sel - images.SELECT_X] if sel >= images.SELECT_X else selected.get(sel, 0) for sel in swizzle]


def _select(channels, sel):
    if sel >= images.SELECT_X:
        return channels[sel - images.SELECT_X]
    return images.ONE_BITS if sel == images.SELECT_ONE else 0


def _lerp_bits(a_bits, b_bits, frac):
    a, b = bits_float(a_bits), bits_float(b_bits)
    diff = _f32(b - a)
    prod = _f32(diff * frac)
    return float_bits(_f32(a + prod))


def _bilerp_bits(c00, c10, c01, c11, fx, fy):
    return _lerp_bits(_lerp_bits(c00, c10, fx), _lerp_bits(c01, c11, fx), fy)


def _compare_depth(z_bits, texel_bits, func):
    z, t = bits_float(z_bits), bits_float(texel_bits)
    if func == images.COMPARE_NEVER:
        passed = False
    elif func == images.COMPARE_LESS:
        passed = z < t
    elif func == images.COMPARE_EQUAL:
        passed = z == t
    elif func == images.COMPARE_LEQUAL:
        passed = z <= t
    elif func == images.COMPARE_GREATER:
        passed = z > t
    elif func == images.COMPARE_NOTEQUAL:
        passed = z != t
    elif func == images.COMPARE_GEQUAL:
        passed = z >= t
    else:
        passed = True
    return images.ONE_BITS if passed else 0


def float_to_unorm8(bits):
    """Convertit un float32 (bits) en entier UNORM 8 bits [0..255], identique au HLSL."""
    value = bits_float(bits)
    if math.isnan(value) or value <= 0.0:
        return 0
    if value >= 1.0:
        return 255
    scaled = _f32(value * 255.0)
    return int(math.floor(_f32(scaled + 0.5)))


def _unpack_dmask(dmask, values):
    packed = [0, 0, 0, 0]
    idx = 0
    for channel in range(4):
        if dmask >> channel & 1:
            packed[channel] = values[idx]
            idx += 1
    return packed


def _store_texel(memory, image, x, y, dmask, values):
    if x >= image["width"] or y >= image["height"] or dmask == 0:
        return
    fmt = image["format"]
    address = image["address"] + (y * image["width"] + x) * images.TEXEL_BYTES[fmt]
    packed = _unpack_dmask(dmask, values)
    if fmt == images.FORMAT_32_32_32_32_FLOAT:
        for channel in range(4):
            if dmask >> channel & 1:
                memory.write_words(address + channel * images.DWORD, [packed[channel]])
        return
    if fmt == images.FORMAT_32_32_FLOAT:
        for channel in range(2):
            if dmask >> channel & 1:
                memory.write_words(address + channel * images.DWORD, [packed[channel]])
        return
    if fmt == images.FORMAT_32_FLOAT:
        if dmask & 1:
            memory.write_words(address, [packed[0]])
        return
    if fmt == images.FORMAT_8_8_8_8_UNORM:
        word = 0 if dmask == 0xF else memory.read_words(address, 1)[0]
        for channel in range(4):
            if dmask >> channel & 1:
                shift = 8 * channel
                word = (word & ~(0xFF << shift)) | (float_to_unorm8(packed[channel]) << shift)
        memory.write_words(address, [word & MASK32])
        return
    if fmt == images.FORMAT_8_8_UNORM:
        raw = bytearray(memory.read(address, 2))
        if dmask & 1:
            raw[0] = float_to_unorm8(packed[0])
        if dmask & 2:
            raw[1] = float_to_unorm8(packed[1])
        memory.write(address, bytes(raw))
        return
    if dmask & 1:
        memory.write(address, bytes((float_to_unorm8(packed[0]),)))


def _sample_color(memory, image, sampler, s_bits, t_bits, unnormalized, off_x=0, off_y=0, lod_bits=None):
    border = sampler["border_color"]
    if _use_linear(sampler, lod_bits):
        x0, x1, fx = bilinear_axis(s_bits, image["width"], unnormalized, off_x)
        y0, y1, fy = bilinear_axis(t_bits, image["height"], unnormalized, off_y)
        cx0 = clamp_index(x0, image["width"], sampler["clamp_x"])
        cx1 = clamp_index(x1, image["width"], sampler["clamp_x"])
        cy0 = clamp_index(y0, image["height"], sampler["clamp_y"])
        cy1 = clamp_index(y1, image["height"], sampler["clamp_y"])
        c00 = fetch_texel(memory, image, cx0, cy0, border)
        c10 = fetch_texel(memory, image, cx1, cy0, border)
        c01 = fetch_texel(memory, image, cx0, cy1, border)
        c11 = fetch_texel(memory, image, cx1, cy1, border)
        raw = [_bilerp_bits(c00[k], c10[k], c01[k], c11[k], fx, fy) for k in range(4)]
    else:
        x = texel_index(s_bits, image["width"], unnormalized, sampler["clamp_x"], off_x)
        y = texel_index(t_bits, image["height"], unnormalized, sampler["clamp_y"], off_y)
        raw = fetch_texel(memory, image, x, y, border)
    return swizzled(raw, image["swizzle"])


def _sample_compare(memory, image, sampler, z_bits, s_bits, t_bits, unnormalized, off_x=0, off_y=0, lod_bits=None):
    border = sampler["border_color"]
    func = sampler["depth_compare"]
    if _use_linear(sampler, lod_bits):
        x0, x1, fx = bilinear_axis(s_bits, image["width"], unnormalized, off_x)
        y0, y1, fy = bilinear_axis(t_bits, image["height"], unnormalized, off_y)
        cx0 = clamp_index(x0, image["width"], sampler["clamp_x"])
        cx1 = clamp_index(x1, image["width"], sampler["clamp_x"])
        cy0 = clamp_index(y0, image["height"], sampler["clamp_y"])
        cy1 = clamp_index(y1, image["height"], sampler["clamp_y"])
        r00 = _compare_depth(z_bits, fetch_texel(memory, image, cx0, cy0, border)[0], func)
        r10 = _compare_depth(z_bits, fetch_texel(memory, image, cx1, cy0, border)[0], func)
        r01 = _compare_depth(z_bits, fetch_texel(memory, image, cx0, cy1, border)[0], func)
        r11 = _compare_depth(z_bits, fetch_texel(memory, image, cx1, cy1, border)[0], func)
        pcf = _bilerp_bits(r00, r10, r01, r11, fx, fy)
    else:
        x = texel_index(s_bits, image["width"], unnormalized, sampler["clamp_x"], off_x)
        y = texel_index(t_bits, image["height"], unnormalized, sampler["clamp_y"], off_y)
        pcf = _compare_depth(z_bits, fetch_texel(memory, image, x, y, border)[0], func)
    return swizzled([pcf, pcf, pcf, images.ONE_BITS], image["swizzle"])


def _gather4(memory, image, sampler, s_bits, t_bits, unnormalized, dmask, off_x=0, off_y=0):
    border = sampler["border_color"]
    x0, x1, _ = bilinear_axis(s_bits, image["width"], unnormalized, off_x)
    y0, y1, _ = bilinear_axis(t_bits, image["height"], unnormalized, off_y)
    cx0 = clamp_index(x0, image["width"], sampler["clamp_x"])
    cx1 = clamp_index(x1, image["width"], sampler["clamp_x"])
    cy0 = clamp_index(y0, image["height"], sampler["clamp_y"])
    cy1 = clamp_index(y1, image["height"], sampler["clamp_y"])
    sel = image["swizzle"][images.gather_channel(dmask)]
    # Ordre matériel RDNA 2 / D3D12 / Vulkan : (x0, y1), (x1, y1), (x1, y0), (x0, y0)
    coords = ((cx0, cy1), (cx1, cy1), (cx1, cy0), (cx0, cy0))
    return [_select(fetch_texel(memory, image, cx, cy, border), sel) for cx, cy in coords]


def _write_masked(wave, vdata, lane, dmask, result):
    written = 0
    for component in range(4):
        if dmask >> component & 1:
            wave.v[vdata + written][lane] = result[component]
            written += 1


def execute(wave, name, word):
    """Exécute une instruction MIMG ; `word` : l'instruction entière."""
    from emu.rdna2_exec import Untranslated

    fields = images.instruction_fields(word)
    try:
        images.check_instruction(name, fields)
        image = images.image([wave.s[fields["srsrc"] + i] for i in range(8)])
        sampler = images.sampler([wave.s[fields["ssamp"] + i] for i in range(4)]) if name in images.SAMPLER_OPS else None
    except images.ImageError as error:
        raise Untranslated(str(error)) from None
    addrs = images.address_registers(name, fields)
    dmask, vdata = fields["dmask"], fields["vdata"]
    if name in ("image_load", "image_load_mip"):
        for lane in wave.active():
            x, y = wave.v[addrs[0]][lane], wave.v[addrs[1]][lane]
            mip = wave.v[addrs[2]][lane] if name == "image_load_mip" else 0
            if mip != 0 or x >= image["width"] or y >= image["height"]:
                result = [0, 0, 0, 0]
            else:
                result = swizzled(texel(wave.memory, image, x, y), image["swizzle"])
            _write_masked(wave, vdata, lane, dmask, result)
        return
    if name in ("image_store", "image_store_mip"):
        count = bin(dmask).count("1")
        for lane in wave.active():
            x, y = wave.v[addrs[0]][lane], wave.v[addrs[1]][lane]
            mip = wave.v[addrs[2]][lane] if name == "image_store_mip" else 0
            if mip == 0:
                values = [wave.v[vdata + i][lane] for i in range(count)]
                _store_texel(wave.memory, image, x, y, dmask, values)
        return
    if name == "image_get_resinfo":
        for lane in wave.active():
            mip = wave.v[addrs[0]][lane]
            result = [image["width"], image["height"], 1, 1] if mip == 0 else [0, 0, 0, 1]
            _write_masked(wave, vdata, lane, dmask, result)
        return
    unnormalized = fields["unorm"] or sampler["unnormalized"]
    off_reg, z_reg, s_reg, t_reg, lod_reg = images.parse_sampler_addrs(name, addrs)
    if name in images.GATHER_OPS:
        for lane in wave.active():
            off_x, off_y = _unpack_offset(wave.v[off_reg][lane]) if off_reg is not None else (0, 0)
            s_bits, t_bits = wave.v[s_reg][lane], wave.v[t_reg][lane]
            result = _gather4(wave.memory, image, sampler, s_bits, t_bits, unnormalized, dmask, off_x, off_y)
            for idx in range(4):
                wave.v[vdata + idx][lane] = result[idx]
        return
    if name in images.COMPARE_OPS:
        for lane in wave.active():
            off_x, off_y = _unpack_offset(wave.v[off_reg][lane]) if off_reg is not None else (0, 0)
            z_bits, s_bits, t_bits = wave.v[z_reg][lane], wave.v[s_reg][lane], wave.v[t_reg][lane]
            lod_bits = wave.v[lod_reg][lane] if lod_reg is not None else None
            result = _sample_compare(
                wave.memory, image, sampler, z_bits, s_bits, t_bits, unnormalized, off_x, off_y, lod_bits
            )
            _write_masked(wave, vdata, lane, dmask, result)
        return
    for lane in wave.active():
        off_x, off_y = _unpack_offset(wave.v[off_reg][lane]) if off_reg is not None else (0, 0)
        s_bits, t_bits = wave.v[s_reg][lane], wave.v[t_reg][lane]
        lod_bits = wave.v[lod_reg][lane] if lod_reg is not None else None
        result = _sample_color(
            wave.memory, image, sampler, s_bits, t_bits, unnormalized, off_x, off_y, lod_bits
        )
        _write_masked(wave, vdata, lane, dmask, result)
