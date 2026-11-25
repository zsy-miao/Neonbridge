"""Traduction HLSL des accès mémoire typés (buffer_*_format, MUBUF ; tbuffer_*_format, MTBUF).

Même calcul que l'interpréteur (emu/rdna2_exec_format.py), au bit près : le shader lit le V# dans
ses SGPR, le format dans le V# (MUBUF) ou dans l'instruction (MTBUF), et convertit avec des
opérations entières (UNORM et SNORM arrondis au plus proche sans division flottante, float16
élargi bit à bit). La table BUF_FORMAT_INFO vient de emu/rdna2_buffer_format.py.

Ce qui est refusé à la traduction (lds, tfe, format MTBUF non pris en charge, nombre de
composantes) lève Untranslatable. Ce qui ne se voit qu'à l'exécution (format du V# non pris en
charge, DST_SEL refusé, SWIZZLE_ENABLE, ADD_TID_ENABLE) donne des zéros en lecture et n'écrit
rien, alors que l'interpréteur le refuse en le nommant (comme emu/rdna2_hlsl_image.py). Les
éléments à cheval sur deux mots sont lus mot par mot ; les mots écrits en partie le sont par
InterlockedAnd/InterlockedOr, pour ne pas effacer les octets voisins écrits par d'autres lignes.
"""
from emu import rdna2_buffer_format as formats
from emu.rdna2_hlsl_ops import NULL, Untranslatable, source

STORE_BARRIER = "AllMemoryBarrierWithGroupSync();"
KIND_BITS, COUNT_SHIFT, SIZE_SHIFT, STORABLE_SHIFT, WIDTH_SHIFT, WIDTH_BITS = 3, 3, 6, 11, 12, 5
INLINE_ZERO = 128

supports = formats.supports


def info(fmt):
    """Mot de BUF_FORMAT_INFO : genre, composantes, taille, inscriptible, largeurs - 1 (5 bits chacune)."""
    word = fmt.kind | fmt.count << COUNT_SHIFT | fmt.size << SIZE_SHIFT | fmt.storable << STORABLE_SHIFT
    for index, width in enumerate(fmt.widths):
        word |= (width - 1) << (WIDTH_SHIFT + WIDTH_BITS * index)
    return word


_TABLE = ", ".join(f"0x{info(formats.FORMATS[code]) if code in formats.FORMATS else 0:08X}u"
                   for code in range(formats.FORMAT_CODES))

HELPERS = f"""
static const uint BUF_FORMAT_INFO[{formats.FORMAT_CODES}] = {{ {_TABLE} }};

uint BufFormatInfo(uint code, uint4 d) {{
  if (((d.y >> {formats.SWIZZLE_ENABLE_BIT}) & 1u) != 0u || ((d.w >> {formats.ADD_TID_BIT}) & 1u) != 0u) return 0u;
  return BUF_FORMAT_INFO[code & 0x7Fu];
}}
uint BufWidth(uint info, uint c) {{ return ((info >> ({WIDTH_SHIFT}u + {WIDTH_BITS}u * c)) & 31u) + 1u; }}
uint BufCount(uint info) {{ return (info >> {COUNT_SHIFT}) & 7u; }}
uint BufSize(uint info) {{ return (info >> {SIZE_SHIFT}) & 31u; }}

// m / (2^n - 1) arrondi au plus proche, 1 <= n <= 16 : l'écriture binaire du quotient est m répété ;
// T (hi:lo, 64 bits) en garde les 64 premiers bits, la mantisse et le bit d'arrondi en sont tirés
// (jamais d'égalité : la suite répétée n'est jamais nulle). Méthode de norm_bits (Leonida), en mots de 32 bits.
uint BufUnorm(uint m, uint n) {{
  if (m == 0u) return 0u;
  if (m == (1u << n) - 1u) return 0x3F800000u;
  uint hi = 0u, lo = 0u;
  for (int top = 64 - int(n); top > -int(n); top -= int(n)) {{
    if (top >= 32) hi |= m << uint(top - 32);
    else if (top >= 0) {{
      lo |= m << uint(top);
      if (uint(top) + n > 32u) hi |= m >> uint(32 - top);
    }} else lo |= m >> uint(-top);
  }}
  uint p = firstbithigh(hi);
  uint q = p >= 23u ? hi >> (p - 23u) : (hi << (23u - p)) | (lo >> (p + 9u));
  uint r = p >= 24u ? (hi >> (p - 24u)) & 1u : (lo >> (p + 8u)) & 1u;
  return ((p + 95u) << 23) + (q - 0x800000u) + r;
}}

uint BufSigned(uint field, uint n) {{ return asuint(asint(field << (32u - n)) >> (32u - n)); }}

uint BufSnorm(uint field, uint n) {{
  int x = asint(BufSigned(field, n));
  int limit = int((1u << (n - 1u)) - 1u);
  if (x <= -limit) return 0xBF800000u;
  uint magnitude = BufUnorm(uint(abs(x)), n - 1u);
  return x < 0 ? magnitude | 0x80000000u : magnitude;
}}

uint BufHalf(uint h) {{
  uint sign = (h & 0x8000u) << 16, e = (h >> 10) & 31u, m = h & 0x3FFu;
  if (e == 31u) return sign | 0x7F800000u | (m << 13);
  if (e != 0u) return sign | ((e + 112u) << 23) | (m << 13);
  if (m == 0u) return sign;
  uint p = firstbithigh(m);
  return sign | ((p + 103u) << 23) | ((m << (23u - p)) & 0x7FFFFFu);
}}

uint BufComponent(uint kind, uint field, uint n) {{
  if (kind == {formats.UNORM}u) return BufUnorm(field, n);
  if (kind == {formats.SNORM}u) return BufSnorm(field, n);
  if (kind == {formats.USCALED}u) return asuint(float(field));
  if (kind == {formats.SSCALED}u) return asuint(float(asint(BufSigned(field, n))));
  if (kind == {formats.SINT}u) return BufSigned(field, n);
  if (kind == {formats.FLOAT}u && n == 16u) return BufHalf(field);
  return field;
}}

uint4 BufDecode(uint info, uint4 w) {{
  uint4 c = uint4(0u, 0u, 0u, 0u);
  uint position = 0u;
  for (uint i = 0u; i < BufCount(info); i++) {{
    uint n = BufWidth(info, i);
    uint word = w[position >> 5];
    // Champ par décalage à gauche puis à droite : avec (word >> p) & masque, le pilote AMD RDNA 2 perd le >> 16 des moitiés hautes.
    uint field = n >= 32u ? word : (word << (32u - n - (position & 31u))) >> (32u - n);
    c[i] = BufComponent(info & 7u, field, n);
    position += n;
  }}
  return c;
}}

uint4 BufRead(uint address, uint size) {{
  uint base = address & ~3u, shift = (address & 3u) * 8u, need = ((address & 3u) + size + 3u) >> 2;
  uint raw[5] = {{ 0u, 0u, 0u, 0u, 0u }};
  for (uint k = 0u; k < need; k++) raw[k] = g_memory.Load(base + 4u * k);
  uint4 w;
  for (uint j = 0u; j < 4u; j++) w[j] = shift == 0u ? raw[j] : (raw[j] >> shift) | (raw[j + 1u] << (32u - shift));
  return w;
}}

uint BufSelect(uint4 c, uint info, uint sel) {{
  if (info == 0u) return 0u;
  uint kind = info & 7u;
  if (sel == {formats.SELECT_ONE}u) return kind == {formats.UINT}u || kind == {formats.SINT}u ? 1u : 0x3F800000u;
  if (sel >= {formats.SELECT_X}u && sel - {formats.SELECT_X}u < BufCount(info)) return c[sel - {formats.SELECT_X}u];
  return 0u;
}}

uint BufField(uint kind, uint value, uint n) {{
  if (n >= 32u) return value;
  if (kind == {formats.UINT}u) return min(value, (1u << n) - 1u);
  int high = int((1u << (n - 1u)) - 1u);
  return uint(clamp(asint(value), -high - 1, high)) & ((1u << n) - 1u);
}}

uint4 BufEncode(uint info, uint4 v) {{
  uint4 w = uint4(0u, 0u, 0u, 0u);
  uint position = 0u;
  for (uint i = 0u; i < BufCount(info); i++) {{
    uint n = BufWidth(info, i);
    w[position >> 5] |= BufField(info & 7u, v[i], n) << (position & 31u);
    position += n;
  }}
  return w;
}}

bool BufStoreSelectsOk(uint info, uint word3, uint count) {{
  uint n = BufCount(info);
  if (((info >> {STORABLE_SHIFT}) & 1u) == 0u || n != count) return false;
  for (uint i = 0u; i < n; i++) if (((word3 >> (3u * i)) & 7u) != {formats.SELECT_X}u + i) return false;
  return n != 2u || ((word3 >> 6) & 7u) <= {formats.SELECT_ONE}u;
}}

void BufWrite(uint address, uint size, uint4 e) {{
  uint base = address & ~3u, a = address & 3u, shift = a * 8u, need = (a + size + 3u) >> 2;
  uint data[5] = {{ e.x, e.y, e.z, e.w, 0u }};
  uint mask[5] = {{ 0u, 0u, 0u, 0u, 0u }};
  for (uint j = 0u; j < 4u; j++)
    mask[j] = size >= 4u * j + 4u ? 0xFFFFFFFFu : (size > 4u * j ? (1u << (8u * (size - 4u * j))) - 1u : 0u);
  for (uint k = 0u; k < need; k++) {{
    uint value = data[k] << shift, bits = mask[k] << shift;
    if (shift != 0u && k > 0u) {{
      value |= data[k - 1u] >> (32u - shift);
      bits |= mask[k - 1u] >> (32u - shift);
    }}
    if (bits == 0xFFFFFFFFu) g_memory.Store(base + 4u * k, value);
    else if (bits != 0u) {{
      g_memory.InterlockedAnd(base + 4u * k, ~bits);
      g_memory.InterlockedOr(base + 4u * k, value & bits);
    }}
  }}
}}
"""


def _checked(name, word):
    try:
        access = formats.access(name, word)
        fmt = formats.format_for(access.fixed, access.store) if access.typed else None
    except formats.FormatError as error:
        raise Untranslatable(f"non traduite en HLSL : {error}") from None
    if fmt is not None and (access.count > fmt.count or access.store and access.count != fmt.count):
        raise Untranslatable(f"non traduite en HLSL : {name}, {access.count} composantes au format {fmt.name}")
    return access


def _address_lines(access):
    """Adresse et test des bornes, comme emu/rdna2_exec_memory.py (_buffer) avec la taille de l'élément."""
    r = access.srsrc
    code = f"{access.fixed}u" if access.typed else f"((d.w >> {formats.WORD3_FORMAT_SHIFT}) & 0x7Fu)"
    soffset = "" if access.soffset in (NULL, INLINE_ZERO) else f" + {source(access.soffset)}"
    index = f"v[{access.vaddr}]" if access.idxen else "0u"
    offset = f"{access.offset}u + v[{access.vaddr + access.idxen}]" if access.offen else f"{access.offset}u"
    enabled = " && ((d.w >> 12) & 0x7Fu) != 0u" if access.typed else ""
    return [f"uint4 d = uint4(s[{r}], s[{r + 1}], s[{r + 2}], s[{r + 3}]);",
            f"uint info = BufFormatInfo({code}, d), size = BufSize(info), stride = (d.y >> 16) & 0x3FFFu;",
            f"uint index = {index}, offset = {offset};",
            f"bool inside = info != 0u{enabled} && (stride != 0u ? index < d.z"
            f" : (offset >= {access.offset}u && offset <= d.z && size <= d.z - offset));",
            f"uint address = d.x{soffset} + index * stride + offset;"]


def statements(name, word, active):
    """HLSL d'un accès typé ; `word` : les 64 bits de l'instruction."""
    access = _checked(name, word)
    lines = _address_lines(access)
    if access.store:
        values = ", ".join(f"v[{access.vdata + i}]" if i < access.count else "0u" for i in range(4))
        allowed = "true" if access.typed else f"BufStoreSelectsOk(info, d.w, {access.count}u)"
        lines.append(f"if (inside && {allowed}) BufWrite(address, size, BufEncode(info, uint4({values})));")
        return [f"if ({active}) {{ " + " ".join(lines) + " }", STORE_BARRIER]
    lines.append("uint4 c = uint4(0u, 0u, 0u, 0u); if (inside) c = BufDecode(info, BufRead(address, size));")
    for i in range(access.count):
        value = f"c[{i}]" if access.typed else f"BufSelect(c, info, (d.w >> {3 * i}) & 7u)"
        lines.append(f"uint r{i} = {value};")
    lines += [f"v[{access.vdata + i}] = r{i};" for i in range(access.count)]
    return [f"if ({active}) {{ " + " ".join(lines) + " }"]
