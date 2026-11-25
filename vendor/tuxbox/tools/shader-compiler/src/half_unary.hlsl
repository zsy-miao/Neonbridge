
uint tb_finite_unary_f16(float x, uint mode, uint op) {
    // op is a compile-time constant for each guest instruction.
    if (op == 0u) return tb_f16_rne(floor(x));
    if (op == 1u) return tb_f16_rne(ceil(x));
    if (op == 2u) return tb_f16_rne(trunc(x));
    if (op == 3u) return tb_f16_rne(round(x));
    precise float fraction = x - floor(x);
    uint halfBits = tb_mode_half(fraction, mode);
    // FRACT bounds its result below 1 before the output multiplier.
    return halfBits == 0x3c00u ? 0x3bffu : halfBits;
}

uint tb_unary_f16(uint bits, uint mode, uint op) {
    bits &= 0xffffu;
    uint magnitude = bits & 0x7fffu;
    // The host's float conversion can canonicalize NaNs. RDNA2 retains the
    // sign and payload, and quiets a signaling NaN only when IEEE mode is set.
    if (magnitude > 0x7c00u) return bits | (mode & 0x200u);
    if (magnitude == 0x7c00u) return op == 4u ? 0xfe00u : bits;
    if (magnitude < 0x400u && (mode & 0x40u) == 0u) bits &= 0x8000u;
    return tb_finite_unary_f16(f16tof32(bits), mode, op);
}
