
uint tb_norm_f16(uint bits, uint mode, bool signedResult) {
    uint magnitude = bits & 0x7fffu;
    bool negative = (bits & 0x8000u) != 0u;
    if (magnitude > 0x7c00u || (negative && !signedResult)) return 0u;
    if (magnitude < 0x400u && (mode & 0x40u) == 0u) return 0u;

    uint scale = signedResult ? 32767u : 65535u;
    uint result = scale;
    if (magnitude < 0x3c00u) {
        // The scaled significand fits in 27 bits. Round this exact product
        // once: rounding it to float first can create a false tie near 0.5.
        uint exponent = magnitude >> 10;
        uint significand = (magnitude & 1023u) + (exponent == 0u ? 0u : 1024u);
        uint shift = 25u - max(exponent, 1u);
        uint product = significand * scale;
        result = product >> shift;
        uint remainder = product & ((1u << shift) - 1u);
        uint midpoint = 1u << (shift - 1u);
        if (remainder > midpoint || (remainder == midpoint && (result & 1u) != 0u)) ++result;
    }
    return negative ? ((0u - result) & 0xffffu) : result;
}
