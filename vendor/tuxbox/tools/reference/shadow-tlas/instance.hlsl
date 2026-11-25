// Experimental per-instance encoder. This is a standalone control and is not
// wired into the runtime. The caller supplies already-validated BLAS metadata.
// Float32 matrix inversion can diverge from the CPU reference for ill-conditioned
// transforms; metadata provenance and runtime queue integration remain open.
// Status: 0 = disabled, 1 = encoded, 2 = invalid/unsupported input.
uint tb_encode_instance(uint src[16], uint root, uint compression, uint index,
                        uint parent, out uint dst[32]) {
    for (uint n = 0; n < 32; ++n) dst[n] = 0;
    if ((src[12] >> 24) == 0 || (src[14] | src[15]) == 0) return 0;
    if ((src[14] & 255) != 0 || src[15] >= 256 || root >= (1u << 29) ||
        ((root & 7) != 0 && (root & 7) != 4 && (root & 7) != 5) ||
        compression != 0x007f7f7f) return 2;
    float m[12];
    for (uint n = 0; n < 12; ++n) {
        m[n] = asfloat(src[n]);
        if (!isfinite(m[n])) return 2;
    }
    float a=m[0], b=m[1], c=m[2], d=m[4], e=m[5], f=m[6], g=m[8], h=m[9], i=m[10];
    float det = a*(e*i-f*h) - b*(d*i-f*g) + c*(d*h-e*g);
    if (det == 0 || !isfinite(det)) return 2;
    float inv[9] = {(e*i-f*h)/det, (c*h-b*i)/det, (b*f-c*e)/det,
                    (f*g-d*i)/det, (a*i-c*g)/det, (c*d-a*f)/det,
                    (d*h-e*g)/det, (b*g-a*h)/det, (a*e-b*d)/det};
    for (uint n = 0; n < 9; ++n) if (!isfinite(inv[n])) return 2;
    float3 translation = float3(m[3], m[7], m[11]);
    float3 t = -float3(dot(float3(inv[0],inv[1],inv[2]),translation),
                       dot(float3(inv[3],inv[4],inv[5]),translation),
                       dot(float3(inv[6],inv[7],inv[8]),translation));
    if (!all(isfinite(t))) return 2;
    for (uint n = 0; n < 14; ++n) dst[n] = src[n];
    bool identity = a == 1 && e == 1 && i == 1 && b == 0 && c == 0 &&
                    d == 0 && f == 0 && g == 0 && h == 0;
    dst[14] = root | (identity ? 0x80000000 : 0);
    dst[15] = (src[14] >> 8) | (src[15] << 24);
    dst[16] = asuint(t.x); dst[17] = asuint(t.y); dst[18] = asuint(t.z);
    for (uint n = 0; n < 9; ++n) dst[19+n] = asuint(inv[n]);
    dst[28] = index; dst[29] = parent; dst[30] = compression;
    return 1;
}

// Outward interval rounding around each FP32 operation. Values in the
// subnormal range are widened to the smallest normal magnitude so that FTZ
// cannot shrink the interval. Overflow is rejected rather than clamped.
float tb_down(float v) {
    if (abs(v) <= 1.17549435e-38f) return -1.17549435e-38f;
    return asfloat(v > 0 ? asuint(v) - 1 : asuint(v) + 1);
}
float tb_up(float v) {
    if (abs(v) <= 1.17549435e-38f) return 1.17549435e-38f;
    return asfloat(v > 0 ? asuint(v) + 1 : asuint(v) - 1);
}
bool tb_input_finite_normal_or_zero(float v) {
    uint bits = asuint(v) & 0x7fffffffu;
    return bits < 0x7f800000u && (bits == 0 || bits >= 0x00800000u);
}
// Status 1 = conservative bounds, 2 = unsupported input or overflow.
uint tb_instance_bounds(uint src[16], float3 objectMin, float3 objectMax,
                        out float3 worldMin, out float3 worldMax) {
    worldMin = 0; worldMax = 0;
    for (uint k = 0; k < 3; ++k) {
        if (!tb_input_finite_normal_or_zero(objectMin[k]) ||
            !tb_input_finite_normal_or_zero(objectMax[k]) || objectMin[k] > objectMax[k]) return 2;
    }
    for (uint row = 0; row < 3; ++row) {
        float origin = asfloat(src[row*4+3]);
        if (!tb_input_finite_normal_or_zero(origin)) return 2;
        float lo = origin, hi = origin;
        for (uint col = 0; col < 3; ++col) {
            float coefficient = asfloat(src[row*4+col]);
            if (!tb_input_finite_normal_or_zero(coefficient)) return 2;
            float p = coefficient < 0 ? objectMax[col] : objectMin[col];
            float q = coefficient < 0 ? objectMin[col] : objectMax[col];
            // precise prohibits contraction of a product and following sum.
            precise float lowProduct = coefficient * p;
            precise float highProduct = coefficient * q;
            if (!isfinite(lowProduct) || !isfinite(highProduct)) return 2;
            float lowTerm = coefficient == 0 || p == 0 ? 0 : tb_down(lowProduct);
            float highTerm = coefficient == 0 || q == 0 ? 0 : tb_up(highProduct);
            precise float lower = lo + lowTerm;
            precise float upper = hi + highTerm;
            if (!isfinite(lower) || !isfinite(upper)) return 2;
            lo = lowTerm == 0 ? lo : tb_down(lower);
            hi = highTerm == 0 ? hi : tb_up(upper);
            if (!isfinite(lo) || !isfinite(hi)) return 2;
        }
        worldMin[row] = lo; worldMax[row] = hi;
    }
    return 1;
}
