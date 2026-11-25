float tb_fract_f32(float x, uint mode) {
    uint bits = asuint(x);
    uint magnitude = bits & 0x7fffffffu;
    if (magnitude < 0x00800000u) {
        if (magnitude == 0u || (mode & 0x10u) == 0u) return 0.0;
        if (bits == magnitude) return (mode & 0x20u) != 0u ? x : 0.0;
    }
    // For -2^-25 <= x < 0, rounding x - floor(x) to nearest can give 1.0.
    // RDNA returns the largest binary32 below 1, before any output modifier.
    if (bits > 0x80000000u && bits <= 0xb3000000u) return asfloat(0x3f7fffffu);
    // DXC can fold frac(x) to the fractional magnitude for a negative constant.
    precise float result = x - floor(x);
    return result;
}
