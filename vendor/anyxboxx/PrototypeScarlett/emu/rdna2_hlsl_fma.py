"""FMA f32 arrondie une seule fois, en arithmétique entière HLSL (utilisé par emu/rdna2_hlsl.py).

`mad` ne suffit pas : HLSL le laisse fusionner ou non au gré du pilote. Mesuré : fusionné sur une RTX
et sur un Intel UHD, pas sur WARP (3FC00000 × 3F800001 + 99800000 y donne 3FC00002 au lieu de
3FC00001) ; la RTX et l'Intel UHD ramènent aussi les résultats sous-normaux à zéro (00800000 × 3F7FFFFF
donne 0 au lieu de 00800000), alors que V_FMA_F32 les garde ; et dxc replie les opérandes constants avec
un double arrondi. FmaF32 calcule donc le
résultat sur les bits, sans flottant, sans double (l'Intel UHD n'a pas de FP64) et sans entier 64 bits :
produit 24 × 24 bits en deux mots de 32 bits (MulHiU32), alignement exact avec bit collant, un seul
arrondi au plus proche pair. Les NaN sont reconnus sur les bits (dxc peut replier `x != x`).

Les deux termes (produit de 48 bits, c de 24 bits) sont normalisés au bit 61 d'un mot de 64 bits ; le
plus petit est décalé une seule fois avec bit collant, puis ajouté ou retranché du plus grand. Sous un
décalage de 2 bits ou plus, le résultat garde son bit de tête au moins en 60 et le bit collant reste
loin sous le bit d'arrondi ; sous 0 ou 1 bit, le décalage ne perd rien (bits bas nuls), même en cas
d'annulation. L'arrondi final (FmaRoundPack) suit `softfloat_roundPackToF32` de Berkeley SoftFloat 3e
(Copyright 2011-2018 The Regents of the University of California, BSD 3 clauses :
https://github.com/ucb-bar/berkeley-softfloat-3/blob/master/COPYING.txt), réécrit pour le seul arrondi
au plus proche pair. La sémantique (NaN, zéros signés, sous-normaux gardés) est celle de l'interpréteur
de référence (emu/rdna2_exec_ops._fma), contre lequel la traduction est vérifiée octet par octet.
"""

HELPERS = """// (y:x) >> d sur 64 bits (x : mot faible), les bits perdus posés dans le bit 0 (bit collant).
uint2 ShiftRightJam64(uint2 v, uint d) {
  if (d == 0u) return v;
  if (d < 32u) return uint2((v.x >> d) | (v.y << (32u - d)) | ((v.x << (32u - d)) != 0u ? 1u : 0u), v.y >> d);
  if (d < 64u) {
    uint lost = v.x | (d > 32u ? (v.y << (64u - d)) : 0u);
    return uint2((v.y >> (d - 32u)) | (lost != 0u ? 1u : 0u), 0u);
  }
  return uint2((v.x | v.y) != 0u ? 1u : 0u, 0u);
}
uint2 ShiftLeft64(uint2 v, uint s) {
  if (s == 0u) return v;
  if (s < 32u) return uint2(v.x << s, (v.y << s) | (v.x >> (32u - s)));
  return uint2(0u, v.x << (s - 32u));
}
uint LeadingZeros64(uint2 v) {
  return v.y != 0u ? 31u - firstbithigh(v.y) : 63u - firstbithigh(v.x);
}
// sig : bit 30 = 1 (si normal), 7 bits sous le dernier bit gardé ; valeur = sig × 2^(e - 156).
uint FmaRoundPack(uint sign, int e, uint sig) {
  if (e < 0) {
    uint d = uint(-e);
    sig = d < 31u ? ((sig >> d) | ((sig << (32u - d)) != 0u ? 1u : 0u)) : (sig != 0u ? 1u : 0u);
    e = 0;
  } else if (e > 0xFD || (e == 0xFD && sig >= 0x7FFFFFC0u)) {
    return (sign << 31) | 0x7F800000u;
  }
  uint rest = sig & 0x7Fu;
  sig = (sig + 0x40u) >> 7;
  if (rest == 0x40u) sig &= ~1u;
  if (sig == 0u) e = 0;
  return (sign << 31) + (uint(e) << 23) + sig;
}
uint FmaF32(uint a, uint b, uint c) {
  uint ea = (a >> 23) & 0xFFu, eb = (b >> 23) & 0xFFu, ec = (c >> 23) & 0xFFu;
  uint ma = a & 0x7FFFFFu, mb = b & 0x7FFFFFu, mc = c & 0x7FFFFFu;
  uint sp = (a ^ b) >> 31, sc = c >> 31;
  if (ea == 0xFFu && ma != 0u) return a | 0x400000u;
  if (eb == 0xFFu && mb != 0u) return b | 0x400000u;
  if (ec == 0xFFu && mc != 0u) return c | 0x400000u;
  bool za = (a & 0x7FFFFFFFu) == 0u, zb = (b & 0x7FFFFFFFu) == 0u, zc = (c & 0x7FFFFFFFu) == 0u;
  if (ea == 0xFFu || eb == 0xFFu) {
    if (za || zb || (ec == 0xFFu && sc != sp)) return 0x7FC00000u;
    return (sp << 31) | 0x7F800000u;
  }
  if (ec == 0xFFu) return c;
  if (za || zb) return zc ? ((sp & sc) << 31) : c;
  // Valeur = Z × 2^F ; bit implicite posé, exposant 1 pour les sous-normaux.
  ma |= ea != 0u ? 0x800000u : 0u;
  mb |= eb != 0u ? 0x800000u : 0u;
  mc |= ec != 0u ? 0x800000u : 0u;
  uint2 p = uint2(ma * mb, MulHiU32(ma, mb));
  uint sp61 = LeadingZeros64(p) - 2u;
  p = ShiftLeft64(p, sp61);
  int fp = int(max(ea, 1u) + max(eb, 1u)) - 300 - int(sp61);
  uint sc61 = zc ? 0u : 29u - firstbithigh(mc);
  uint2 q = uint2(0u, zc ? 0u : mc << sc61);
  int fq = zc ? fp : int(max(ec, 1u)) - 182 - int(sc61);
  bool qBig = fq > fp || (fq == fp && (q.y > p.y || (q.y == p.y && q.x > p.x)));
  uint2 x = qBig ? q : p, y = qBig ? p : q;
  int fx = max(fp, fq);
  uint sz = qBig ? sc : sp;
  y = ShiftRightJam64(y, uint(abs(fp - fq)));
  uint2 z = sp == sc ? uint2(x.x + y.x, x.y + y.y + (x.x + y.x < x.x ? 1u : 0u))
                     : uint2(x.x - y.x, x.y - y.y - (x.x < y.x ? 1u : 0u));
  if ((z.x | z.y) == 0u) return 0u;
  uint lz = LeadingZeros64(z);
  z = ShiftLeft64(z, lz - 1u);
  return FmaRoundPack(sz, fx - int(lz) + 189, z.y | (z.x != 0u ? 1u : 0u));
}
uint FmaLegacyF32(uint a, uint b, uint c) {
  // DX9 : 0 × n'importe quoi = 0, le résultat est alors c (rendu silencieux s'il est NaN).
  if ((a & 0x7FFFFFFFu) == 0u || (b & 0x7FFFFFFFu) == 0u) {
    return ((c >> 23) & 0xFFu) == 0xFFu && (c & 0x7FFFFFu) != 0u ? c | 0x400000u : c;
  }
  return FmaF32(a, b, c);
}
"""
