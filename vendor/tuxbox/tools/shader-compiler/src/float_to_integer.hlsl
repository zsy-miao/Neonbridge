bool tb_nan_bits(uint bits) { return (bits & 0x7fffffffu) > 0x7f800000u; }
uint tb_cvt_u32_f32(float x) {
    if (!(x > 0.0) || tb_nan_bits(asuint(x))) return 0u;
    if (x >= 4294967296.0) return 0xffffffffu;
    return uint(x);
}
uint tb_cvt_i32_f32(float x) {
    if (tb_nan_bits(asuint(x))) return 0u;
    if (x >= 2147483648.0) return 0x7fffffffu;
    if (x <= -2147483648.0) return 0x80000000u;
    return asuint(int(x));
}
uint tb_round_u8(float value, uint mode) {
    uint bits = asuint(value);
    if (bits > 0x7f800000u) return 0u; // Negative values and NaNs.
    if (bits >= asuint(255.0f)) return 255u;
    uint rounding = mode & 3u;
    // Host float arithmetic may flush a subnormal before ceil can see it.
    if (bits < 0x00800000u)
        return (bits != 0u && (mode & 0x10u) != 0u && rounding == 1u) ? 1u : 0u;
    if (rounding == 0u) return (uint)round(value);
    if (rounding == 1u) return (uint)ceil(value);
    return (uint)value; // Toward zero and toward -infinity agree for positive values.
}

float tb_integer_to_f32(uint bits, bool signedInput, uint mode) {
    bool negative = signedInput && asint(bits) < 0;
    uint magnitude = negative ? 0u - bits : bits;
    uint rounding = mode & 3u;
    if (rounding == 0u || magnitude <= 0x01000000u)
        return negative ? -float(magnitude) : float(magnitude);

    // Keep the leading 24 bits. The truncated integer is exactly representable,
    // even for UINT_MAX, whose nearest float would lie outside the integer range.
    uint discarded = (1u << (firstbithigh(magnitude) - 23u)) - 1u;
    uint result = asuint(float(magnitude & ~discarded));
    if ((magnitude & discarded) != 0u && rounding == (negative ? 2u : 1u))
        result += 1u;
    return asfloat(result | (negative ? 0x80000000u : 0u));
}
