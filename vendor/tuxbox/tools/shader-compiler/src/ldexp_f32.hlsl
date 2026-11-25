uint tb_scale_f32(uint bits, int scale, uint mode) {
    uint sign = bits & 0x80000000u;
    uint significand = bits & 0x007fffffu;
    int exponent = int((bits >> 23) & 255u);
    if (exponent == 255) {
        if (significand != 0u && (mode & 0x200u) != 0u) return bits | 0x00400000u;
        return bits;
    }
    if (exponent == 0) {
        if (significand == 0u || (mode & 0x10u) == 0u) return sign;
        int normalize = 23 - int(firstbithigh(significand));
        significand <<= normalize;
        exponent = 1 - normalize;
    } else {
        significand |= 0x00800000u;
    }

    // Beyond this range every finite nonzero input already overflows or underflows.
    exponent += clamp(scale, -300, 300);
    uint rounding = mode & 3u;
    bool away = rounding == (sign == 0u ? 1u : 2u);
    if (exponent >= 255)
        return sign | ((rounding == 0u || away) ? 0x7f800000u : 0x7f7fffffu);
    if (exponent > 0)
        return sign | (uint(exponent) << 23) | (significand & 0x007fffffu);
    if ((mode & 0x20u) == 0u) return sign;

    // At a shift of 25 the entire significand is below half a subnormal ULP.
    uint shift = min(uint(1 - exponent), 25u);
    uint unit = 1u << shift;
    uint lost = significand & (unit - 1u);
    uint kept = significand >> shift;
    if (rounding == 0u)
        away = lost > (unit >> 1) || (lost == (unit >> 1) && (kept & 1u) != 0u);
    return sign | (kept + uint(away && lost != 0u));
}

uint tb_ldexp_f32(uint bits, int scale, uint mode, int outputScale, bool clampResult) {
    uint result = tb_scale_f32(bits, scale, mode);
    uint magnitude = result & 0x7fffffffu;
    if (magnitude > 0x7f800000u)
        return clampResult && (mode & 0x100u) != 0u ? 0u : result;
    // OMOD is disabled with IEEE mode or preserved output denormals.
    if (outputScale != 0 && (mode & 0x220u) == 0u)
        result = magnitude == 0u ? 0u : tb_scale_f32(result, outputScale, mode);
    if (!clampResult) return result;
    return (result & 0x80000000u) != 0u ? 0u : min(result, 0x3f800000u);
}
