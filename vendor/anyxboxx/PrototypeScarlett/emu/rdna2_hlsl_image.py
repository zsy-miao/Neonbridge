"""Traduction HLSL des instructions MIMG : le même calcul logiciel que l'interpréteur.

Le shader lit le T# (et le S# pour l'échantillonnage) dans ses SGPR et accède aux texels dans
la mémoire u0, comme emu/rdna2_exec_image.py, pour rester identique au bit près. Les champs de
l'instruction sont vérifiés à la traduction ; un descripteur hors du cas pris en charge (connu
seulement à l'exécution) donne des zéros, alors que l'interpréteur le refuse en le nommant.
"""
from emu import rdna2_image as images
from emu.rdna2_hlsl_ops import Untranslatable

_TABLE = ", ".join(f"0x{value:08X}u" for value in images.UNORM8)
_COMP = ("x", "y", "z", "w")
STORE_BARRIER = "AllMemoryBarrierWithGroupSync();"

HELPERS = f"""
static const uint UNORM8[256] = {{ {_TABLE} }};

uint ImageWidth(uint4 t) {{ return (((t.y >> 30) & 3u) | ((t.z & 0xFFFu) << 2)) + 1u; }}
uint ImageHeight(uint4 t) {{ return ((t.z >> 14) & 0x3FFFu) + 1u; }}

int2 ImageOffset(uint off_word) {{
  int ox = (int(off_word & 0x3Fu) << 26) >> 26;
  int oy = (int((off_word >> 8) & 0x3Fu) << 26) >> 26;
  return int2(ox, oy);
}}

bool ImageUseLinear(uint4 q, uint has_lod, uint lod_bits) {{
  uint filt = (has_lod != 0u && asfloat(lod_bits) > 0.0f) ? ((q.z >> 22) & 3u) : ((q.z >> 20) & 3u);
  return filt == {images.FILTER_LINEAR}u;
}}

int ImageClamp(int index, uint size, uint mode) {{
  int s = int(size);
  if (mode == {images.CLAMP_WRAP}u) {{ int r = index % s; return r < 0 ? r + s : r; }}
  if (mode == {images.CLAMP_MIRROR}u) {{
    int p = 2 * s;
    int r = index % p;
    if (r < 0) r += p;
    return r < s ? r : p - 1 - r;
  }}
  if (mode == {images.CLAMP_LAST_TEXEL}u) return clamp(index, 0, s - 1);
  if (mode == {images.CLAMP_MIRROR_ONCE_LAST_TEXEL}u) {{
    if (index < 0) index = -1 - index;
    return min(index, s - 1);
  }}
  if (mode == {images.CLAMP_MIRROR_ONCE_BORDER}u) {{
    if (index < 0) index = -1 - index;
    return index < s ? index : -1;
  }}
  if (mode == {images.CLAMP_BORDER}u) return (index >= 0 && index < s) ? index : -1;
  return clamp(index, 0, s - 1);
}}

int ImageIndex(uint bits, uint size, uint unnormalized, uint mode, int off) {{
  precise float value = asfloat(bits);
  if (unnormalized == 0u) value = value * float(size);
  uint raw = asuint(value);
  if ((raw & 0x7F800000u) == 0x7F800000u && (raw & 0x007FFFFFu) != 0u) return ImageClamp(off, size, mode);
  int index = int(floor(clamp(value, -2147483520.0f, 2147483520.0f))) + off;
  return ImageClamp(index, size, mode);
}}

int2 ImageBilinearAxis(uint bits, uint size, uint unnormalized, int off, out float frac) {{
  precise float value = asfloat(bits);
  if (unnormalized == 0u) value = value * float(size);
  uint raw = asuint(value);
  if ((raw & 0x7F800000u) == 0x7F800000u && (raw & 0x007FFFFFu) != 0u) {{
    frac = 0.0f;
    return int2(off, off);
  }}
  precise float shifted = value - 0.5f;
  precise float clamped = clamp(shifted, -2147483520.0f, 2147483520.0f);
  precise float fl = floor(clamped);
  precise float f = clamped - fl;
  frac = f;
  int i0 = int(fl) + off;
  return int2(i0, i0 + 1);
}}

uint4 ImageBorderColor(uint border) {{
  if (border == {images.BORDER_OPAQUE_BLACK}u) return uint4(0u, 0u, 0u, {images.ONE_BITS:#x}u);
  if (border == {images.BORDER_OPAQUE_WHITE}u) return uint4({images.ONE_BITS:#x}u, {images.ONE_BITS:#x}u, {images.ONE_BITS:#x}u, {images.ONE_BITS:#x}u);
  return uint4(0u, 0u, 0u, 0u);
}}

uint4 ImageTexel(uint4 t, int x, int y) {{
  uint format = (t.y >> 20) & 0x1FFu;
  uint pixel = uint(y) * ImageWidth(t) + uint(x);
  uint base = t.x << 8;
  if (format == {images.FORMAT_32_32_32_32_FLOAT}u) return g_memory.Load4(base + pixel * 16u);
  if (format == {images.FORMAT_32_32_FLOAT}u) {{
    uint2 v2 = g_memory.Load2(base + pixel * 8u);
    return uint4(v2.x, v2.y, 0u, {images.ONE_BITS:#x}u);
  }}
  if (format == {images.FORMAT_32_FLOAT}u) {{
    return uint4(g_memory.Load(base + pixel * 4u), 0u, 0u, {images.ONE_BITS:#x}u);
  }}
  if (format == {images.FORMAT_8_8_8_8_UNORM}u) {{
    uint w = g_memory.Load(base + pixel * 4u);
    return uint4(UNORM8[w & 255u], UNORM8[(w >> 8) & 255u], UNORM8[(w >> 16) & 255u], UNORM8[w >> 24]);
  }}
  if (format == {images.FORMAT_8_8_UNORM}u) {{
    uint byte_addr = base + pixel * 2u;
    uint w = g_memory.Load(byte_addr & ~3u);
    uint shift = (byte_addr & 2u) * 8u;
    return uint4(UNORM8[(w >> shift) & 255u], UNORM8[(w >> (shift + 8u)) & 255u], 0u, {images.ONE_BITS:#x}u);
  }}
  if (format == {images.FORMAT_8_UNORM}u) {{
    uint byte_addr = base + pixel;
    uint w = g_memory.Load(byte_addr & ~3u);
    uint shift = (byte_addr & 3u) * 8u;
    return uint4(UNORM8[(w >> shift) & 255u], 0u, 0u, {images.ONE_BITS:#x}u);
  }}
  return uint4(0u, 0u, 0u, 0u);
}}

uint4 ImageFetch(uint4 t, int x, int y, uint border) {{
  if (x < 0 || y < 0) return ImageBorderColor(border);
  return ImageTexel(t, x, y);
}}

uint ImageSelect(uint4 c, uint sel) {{
  if (sel >= {images.SELECT_X}u) return c[sel - {images.SELECT_X}u];
  return sel == {images.SELECT_ONE}u ? {images.ONE_BITS:#x}u : 0u;
}}

uint4 ImageSwizzle(uint4 c, uint swz) {{
  return uint4(ImageSelect(c, swz & 7u), ImageSelect(c, (swz >> 3) & 7u),
               ImageSelect(c, (swz >> 6) & 7u), ImageSelect(c, (swz >> 9) & 7u));
}}

uint ImageLerp(uint a_bits, uint b_bits, float f_in) {{
  precise float a = asfloat(a_bits);
  precise float b = asfloat(b_bits);
  precise float f = f_in;
  precise float diff = b - a;
  precise float prod = diff * f;
  precise float res = a + prod;
  return asuint(res);
}}

uint ImageBilerp(uint c00, uint c10, uint c01, uint c11, float fx, float fy) {{
  return ImageLerp(ImageLerp(c00, c10, fx), ImageLerp(c01, c11, fx), fy);
}}

uint ImageCompare(uint z_bits, uint t_bits, uint func) {{
  float z = asfloat(z_bits);
  float t = asfloat(t_bits);
  bool p = false;
  if (func == {images.COMPARE_LESS}u) p = (z < t);
  else if (func == {images.COMPARE_EQUAL}u) p = (z == t);
  else if (func == {images.COMPARE_LEQUAL}u) p = (z <= t);
  else if (func == {images.COMPARE_GREATER}u) p = (z > t);
  else if (func == {images.COMPARE_NOTEQUAL}u) p = (z != t);
  else if (func == {images.COMPARE_GEQUAL}u) p = (z >= t);
  else if (func == {images.COMPARE_ALWAYS}u) p = true;
  return p ? {images.ONE_BITS:#x}u : 0u;
}}

uint4 ImageSampleColor(uint4 t, uint4 q, uint s_bits, uint t_bits, uint unorm_bit, int2 off, uint has_lod, uint lod_bits) {{
  uint un = unorm_bit | ((q.x >> 15) & 1u);
  uint cx = q.x & 7u;
  uint cy = (q.x >> 3) & 7u;
  uint border = (q.w >> 30) & 3u;
  uint w = ImageWidth(t);
  uint h = ImageHeight(t);
  uint4 raw;
  if (ImageUseLinear(q, has_lod, lod_bits)) {{
    float fx = 0.0f;
    float fy = 0.0f;
    int2 xb = ImageBilinearAxis(s_bits, w, un, off.x, fx);
    int2 yb = ImageBilinearAxis(t_bits, h, un, off.y, fy);
    int cx0 = ImageClamp(xb.x, w, cx);
    int cx1 = ImageClamp(xb.y, w, cx);
    int cy0 = ImageClamp(yb.x, h, cy);
    int cy1 = ImageClamp(yb.y, h, cy);
    uint4 c00 = ImageFetch(t, cx0, cy0, border);
    uint4 c10 = ImageFetch(t, cx1, cy0, border);
    uint4 c01 = ImageFetch(t, cx0, cy1, border);
    uint4 c11 = ImageFetch(t, cx1, cy1, border);
    raw = uint4(ImageBilerp(c00.x, c10.x, c01.x, c11.x, fx, fy),
                ImageBilerp(c00.y, c10.y, c01.y, c11.y, fx, fy),
                ImageBilerp(c00.z, c10.z, c01.z, c11.z, fx, fy),
                ImageBilerp(c00.w, c10.w, c01.w, c11.w, fx, fy));
  }} else {{
    int x = ImageIndex(s_bits, w, un, cx, off.x);
    int y = ImageIndex(t_bits, h, un, cy, off.y);
    raw = ImageFetch(t, x, y, border);
  }}
  return ImageSwizzle(raw, t.w);
}}

uint4 ImageSampleCompare(uint4 t, uint4 q, uint z_bits, uint s_bits, uint t_bits, uint unorm_bit, int2 off, uint has_lod, uint lod_bits) {{
  uint un = unorm_bit | ((q.x >> 15) & 1u);
  uint cx = q.x & 7u;
  uint cy = (q.x >> 3) & 7u;
  uint func = (q.x >> 12) & 7u;
  uint border = (q.w >> 30) & 3u;
  uint w = ImageWidth(t);
  uint h = ImageHeight(t);
  uint pcf;
  if (ImageUseLinear(q, has_lod, lod_bits)) {{
    float fx = 0.0f;
    float fy = 0.0f;
    int2 xb = ImageBilinearAxis(s_bits, w, un, off.x, fx);
    int2 yb = ImageBilinearAxis(t_bits, h, un, off.y, fy);
    int cx0 = ImageClamp(xb.x, w, cx);
    int cx1 = ImageClamp(xb.y, w, cx);
    int cy0 = ImageClamp(yb.x, h, cy);
    int cy1 = ImageClamp(yb.y, h, cy);
    uint r00 = ImageCompare(z_bits, ImageFetch(t, cx0, cy0, border).x, func);
    uint r10 = ImageCompare(z_bits, ImageFetch(t, cx1, cy0, border).x, func);
    uint r01 = ImageCompare(z_bits, ImageFetch(t, cx0, cy1, border).x, func);
    uint r11 = ImageCompare(z_bits, ImageFetch(t, cx1, cy1, border).x, func);
    pcf = ImageBilerp(r00, r10, r01, r11, fx, fy);
  }} else {{
    int x = ImageIndex(s_bits, w, un, cx, off.x);
    int y = ImageIndex(t_bits, h, un, cy, off.y);
    pcf = ImageCompare(z_bits, ImageFetch(t, x, y, border).x, func);
  }}
  return ImageSwizzle(uint4(pcf, pcf, pcf, {images.ONE_BITS:#x}u), t.w);
}}

uint4 ImageGather4(uint4 t, uint4 q, uint s_bits, uint t_bits, uint unorm_bit, uint channel, int2 off) {{
  uint un = unorm_bit | ((q.x >> 15) & 1u);
  uint cx = q.x & 7u;
  uint cy = (q.x >> 3) & 7u;
  uint border = (q.w >> 30) & 3u;
  uint w = ImageWidth(t);
  uint h = ImageHeight(t);
  float fx = 0.0f;
  float fy = 0.0f;
  int2 xb = ImageBilinearAxis(s_bits, w, un, off.x, fx);
  int2 yb = ImageBilinearAxis(t_bits, h, un, off.y, fy);
  int cx0 = ImageClamp(xb.x, w, cx);
  int cx1 = ImageClamp(xb.y, w, cx);
  int cy0 = ImageClamp(yb.x, h, cy);
  int cy1 = ImageClamp(yb.y, h, cy);
  uint sel = (t.w >> (3u * channel)) & 7u;
  return uint4(ImageSelect(ImageFetch(t, cx0, cy1, border), sel),
               ImageSelect(ImageFetch(t, cx1, cy1, border), sel),
               ImageSelect(ImageFetch(t, cx1, cy0, border), sel),
               ImageSelect(ImageFetch(t, cx0, cy0, border), sel));
}}

uint4 ImageLoad(uint4 t, uint x, uint y, uint mip) {{
  if (mip != 0u || x >= ImageWidth(t) || y >= ImageHeight(t)) return uint4(0u, 0u, 0u, 0u);
  return ImageSwizzle(ImageTexel(t, int(x), int(y)), t.w);
}}

uint ImageFloatToUnorm8(uint bits) {{
  if ((bits & 0x7F800000u) == 0x7F800000u && (bits & 0x007FFFFFu) != 0u) return 0u;
  precise float f = asfloat(bits);
  if (f <= 0.0f) return 0u;
  if (f >= 1.0f) return 255u;
  precise float scaled = f * 255.0f;
  precise float rounded = floor(scaled + 0.5f);
  return uint(rounded);
}}

void ImageStore(uint4 t, uint x, uint y, uint mip, uint dmask, uint4 vals) {{
  uint w = ImageWidth(t);
  uint h = ImageHeight(t);
  if (mip != 0u || x >= w || y >= h || dmask == 0u) return;
  uint format = (t.y >> 20) & 0x1FFu;
  uint pixel = y * w + x;
  uint base = t.x << 8;
  if (format == {images.FORMAT_32_32_32_32_FLOAT}u) {{
    uint addr = base + pixel * 16u;
    if ((dmask & 1u) != 0u) g_memory.Store(addr, vals.x);
    if ((dmask & 2u) != 0u) g_memory.Store(addr + 4u, vals.y);
    if ((dmask & 4u) != 0u) g_memory.Store(addr + 8u, vals.z);
    if ((dmask & 8u) != 0u) g_memory.Store(addr + 12u, vals.w);
  }} else if (format == {images.FORMAT_32_32_FLOAT}u) {{
    uint addr = base + pixel * 8u;
    if ((dmask & 1u) != 0u) g_memory.Store(addr, vals.x);
    if ((dmask & 2u) != 0u) g_memory.Store(addr + 4u, vals.y);
  }} else if (format == {images.FORMAT_32_FLOAT}u) {{
    if ((dmask & 1u) != 0u) g_memory.Store(base + pixel * 4u, vals.x);
  }} else if (format == {images.FORMAT_8_8_8_8_UNORM}u) {{
    uint addr = base + pixel * 4u;
    uint word = (dmask == 0xFu) ? 0u : g_memory.Load(addr);
    if ((dmask & 1u) != 0u) word = (word & 0xFFFFFF00u) | ImageFloatToUnorm8(vals.x);
    if ((dmask & 2u) != 0u) word = (word & 0xFFFF00FFu) | (ImageFloatToUnorm8(vals.y) << 8);
    if ((dmask & 4u) != 0u) word = (word & 0xFF00FFFFu) | (ImageFloatToUnorm8(vals.z) << 16);
    if ((dmask & 8u) != 0u) word = (word & 0x00FFFFFFu) | (ImageFloatToUnorm8(vals.w) << 24);
    g_memory.Store(addr, word);
  }} else if (format == {images.FORMAT_8_8_UNORM}u) {{
    uint byte_addr = base + pixel * 2u;
    uint aligned = byte_addr & ~3u;
    uint shift = (byte_addr & 2u) * 8u;
    uint clear_mask = 0u;
    uint bits = 0u;
    if ((dmask & 1u) != 0u) {{ clear_mask |= 0xFFu << shift; bits |= ImageFloatToUnorm8(vals.x) << shift; }}
    if ((dmask & 2u) != 0u) {{ clear_mask |= 0xFFu << (shift + 8u); bits |= ImageFloatToUnorm8(vals.y) << (shift + 8u); }}
    if (clear_mask != 0u) {{
      g_memory.InterlockedAnd(aligned, ~clear_mask);
      g_memory.InterlockedOr(aligned, bits);
    }}
  }} else if (format == {images.FORMAT_8_UNORM}u) {{
    if ((dmask & 1u) != 0u) {{
      uint byte_addr = base + pixel;
      uint aligned = byte_addr & ~3u;
      uint shift = (byte_addr & 3u) * 8u;
      g_memory.InterlockedAnd(aligned, ~(0xFFu << shift));
      g_memory.InterlockedOr(aligned, ImageFloatToUnorm8(vals.x) << shift);
    }}
  }}
}}

uint4 ImageResInfo(uint4 t, uint mip) {{
  return mip == 0u ? uint4(ImageWidth(t), ImageHeight(t), 1u, 1u) : uint4(0u, 0u, 0u, 1u);
}}
"""


def _write_masked(vdata, dmask):
    components = [k for k in range(4) if dmask >> k & 1]
    return [f"v[{vdata + n}] = res.{_COMP[k]};" for n, k in enumerate(components)]


def statements(name, word, active):
    """HLSL d'une instruction MIMG ; `word` : l'instruction entière (mot NSA compris)."""
    fields = images.instruction_fields(word)
    try:
        images.check_instruction(name, fields)
    except images.ImageError as error:
        raise Untranslatable(str(error)) from None
    r, q = fields["srsrc"], fields["ssamp"]
    dmask, vdata = fields["dmask"], fields["vdata"]
    addrs = images.address_registers(name, fields)
    t_decl = f"uint4 t = uint4(s[{r}], s[{r + 1}], s[{r + 2}], s[{r + 3}]);"
    if name in ("image_store", "image_store_mip"):
        slot, packed = 0, []
        for k in range(4):
            if dmask >> k & 1:
                packed.append(f"v[{vdata + slot}]")
                slot += 1
            else:
                packed.append("0u")
        mip_expr = f"v[{addrs[2]}]" if name == "image_store_mip" else "0u"
        body = f"{t_decl} ImageStore(t, v[{addrs[0]}], v[{addrs[1]}], {mip_expr}, {dmask}u, uint4({', '.join(packed)}));"
        return [f"if ({active}) {{ {body} }}", STORE_BARRIER]
    if name in ("image_load", "image_load_mip"):
        mip_expr = f"v[{addrs[2]}]" if name == "image_load_mip" else "0u"
        lines = [t_decl, f"uint4 res = ImageLoad(t, v[{addrs[0]}], v[{addrs[1]}], {mip_expr});",
                 *_write_masked(vdata, dmask)]
        return [f"if ({active}) {{ " + " ".join(lines) + " }"]
    if name == "image_get_resinfo":
        lines = [t_decl, f"uint4 res = ImageResInfo(t, v[{addrs[0]}]);", *_write_masked(vdata, dmask)]
        return [f"if ({active}) {{ " + " ".join(lines) + " }"]
    q_decl = f"uint4 q = uint4(s[{q}], s[{q + 1}], s[{q + 2}], s[{q + 3}]);"
    off_reg, z_reg, s_reg, t_reg, lod_reg = images.parse_sampler_addrs(name, addrs)
    off_expr = f"ImageOffset(v[{off_reg}])" if off_reg is not None else "int2(0, 0)"
    has_lod_u = "1u" if lod_reg is not None else "0u"
    lod_expr = f"v[{lod_reg}]" if lod_reg is not None else "0u"
    if name in images.GATHER_OPS:
        channel = images.gather_channel(dmask)
        lines = [t_decl, q_decl,
                 f"uint4 res = ImageGather4(t, q, v[{s_reg}], v[{t_reg}], {fields['unorm']}u, {channel}u, {off_expr});",
                 *(f"v[{vdata + i}] = res.{_COMP[i]};" for i in range(4))]
        return [f"if ({active}) {{ " + " ".join(lines) + " }"]
    if name in images.COMPARE_OPS:
        lines = [t_decl, q_decl,
                 f"uint4 res = ImageSampleCompare(t, q, v[{z_reg}], v[{s_reg}], v[{t_reg}], "
                 f"{fields['unorm']}u, {off_expr}, {has_lod_u}, {lod_expr});",
                 *_write_masked(vdata, dmask)]
        return [f"if ({active}) {{ " + " ".join(lines) + " }"]
    lines = [t_decl, q_decl,
             f"uint4 res = ImageSampleColor(t, q, v[{s_reg}], v[{t_reg}], "
             f"{fields['unorm']}u, {off_expr}, {has_lod_u}, {lod_expr});",
             *_write_masked(vdata, dmask)]
    return [f"if ({active}) {{ " + " ".join(lines) + " }"]
