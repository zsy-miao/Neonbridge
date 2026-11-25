static uint4 intersect(ulong node, float4 origin, float4 direction,
                       float4 inverse) {
  uint4 descriptor = (uint4)(0u, 0x80000000u, 0xffffffffu, 0x810003ffu);
  return __builtin_amdgcn_image_bvh_intersect_ray_l(
      node, 100.f, origin, direction, inverse, descriptor);
}
__kernel void probe(__global unsigned *out) {
  // Seeded by the host: triangle candidates have no near-distance operand.
  // Max distance is deliberately loaded at runtime for each call.
  for (uint i = 0; i < 4; ++i) {
    uint4 descriptor = (uint4)(0u, 0x80000000u, 0xffffffffu, 0x890003ffu);
    float4 endpoint_origin = (float4)(as_float(out[8404]), as_float(out[8405]),
                                      as_float(out[8406]), 0.f);
    *(__global uint4 *)(out + 8416 + 4 * i) =
        __builtin_amdgcn_image_bvh_intersect_ray_l(
            ((ulong)(out + 3200)) >> 3, as_float(out[8400 + i]),
            endpoint_origin, (float4)(0.f, 0.f, 1.f, 0.f),
            (float4)(as_float(0x7f800000u), as_float(0x7f800000u), 1.f, 0.f),
            descriptor);
  }
  for (uint i = 0; i < 8; ++i) {
    uint4 descriptor = (uint4)(0u, 0x80000000u, 0xffffffffu, 0x890003ffu);
    float x = as_float(out[8500 + i * 3]);
    float y = as_float(out[8501 + i * 3]);
    float zdir = as_float(out[8502 + i * 3]);
    *(__global uint4 *)(out + 8448 + 4 * i) =
        __builtin_amdgcn_image_bvh_intersect_ray_l(
            ((ulong)(out + 3200)) >> 3, as_float(out[8524]),
            (float4)(x, y, 0.f, 0.f), (float4)(0.f, 0.f, zdir, 0.f),
            (float4)(as_float(0x7f800000u), as_float(0x7f800000u), zdir, 0.f),
            descriptor);
  }
  float4 origin = (float4)(0.f, 0.f, -4.f, 0.f),
         inverse = (float4)(10.f, 10.f, 1.f, 0.f);
  float4 dir = (float4)(0.1f, 0.1f, 1.f, 0.f);
  *(__global uint4 *)(out + 0) =
      intersect(((ulong)(out + 3072) >> 3) | 5ul, origin, dir, inverse);
  *(__global uint4 *)(out + 4) =
      intersect(((ulong)(out + 3136) >> 3) | 4ul, origin, dir, inverse);
  dir = (float4)(0.f, 0.f, 1.f, 0.f);
  *(__global uint4 *)(out + 8) =
      intersect(((ulong)(out + 3200) >> 3) | 0ul,
                (float4)(0.25f, 0.25f, -1.f, 0.f), dir, inverse);
  *(__global uint4 *)(out + 12) =
      intersect(((ulong)(out + 3200) >> 3) | 1ul,
                (float4)(0.75f, 0.75f, -1.f, 0.f), dir, inverse);
  *(__global uint4 *)(out + 16) =
      intersect(((ulong)(out + 3200) >> 3) | 0ul,
                (float4)(0.5f, 0.5f, 0.f, 0.f), dir, inverse);
  *(__global uint4 *)(out + 20) =
      intersect(((ulong)(out + 3200) >> 3) | 0ul, (float4)(2.f, 2.f, -1.f, 0.f),
                dir, inverse);
  dir = (float4)(0.f, 0.f, -1.f, 0.f);
  *(__global uint4 *)(out + 24) =
      intersect(((ulong)(out + 3200) >> 3) | 0ul,
                (float4)(0.5f, 0.5f, 0.f, 0.f), dir, inverse);
  *(__global uint4 *)(out + 28) =
      intersect(((ulong)(out + 3200) >> 3) | 0ul, (float4)(2.f, 2.f, 1.f, 0.f),
                dir, inverse);
  *(__global uint4 *)(out + 32) =
      intersect(((ulong)(out + 3200) >> 3) | 0ul,
                (float4)(0.5f, 0.5f, -1.f, 0.f), -dir, inverse);
  *(__global uint4 *)(out + 36) =
      intersect(((ulong)(out + 3200) >> 3) | 0ul,
                (float4)(0.f, 0.5f, -1.f, 0.f), -dir, inverse);
  *(__global uint4 *)(out + 40) =
      intersect(((ulong)(out + 3200) >> 3) | 0ul,
                (float4)(0.5f, 0.f, -1.f, 0.f), -dir, inverse);
  *(__global uint4 *)(out + 44) =
      intersect(((ulong)(out + 3200) >> 3) | 0ul,
                (float4)(0.25f, 0.25f, 0.f, 0.f), -dir, inverse);
  *(__global uint4 *)(out + 48) =
      intersect(((ulong)(out + 3264) >> 3) | 0ul, (float4)(2.f, 2.f, -1.f, 0.f),
                -dir, inverse);
  *(__global uint4 *)(out + 52) =
      intersect(((ulong)(out + 3264) >> 3) | 0ul, (float4)(2.f, 2.f, 1.f, 0.f),
                dir, inverse);
  const float2 points[3][6] = {
      {(float2)(0.f, .5f), (float2)(.5f, 0.f), (float2)(.5f, .5f),
       (float2)(0.f, 0.f), (float2)(1.f, 0.f), (float2)(0.f, 1.f)},
      {(float2)(0.f, -.5f), (float2)(.5f, 0.f), (float2)(.5f, -.5f),
       (float2)(0.f, 0.f), (float2)(1.f, 0.f), (float2)(0.f, -1.f)},
      {(float2)(-.5f, .5f), (float2)(.5f, .5f), (float2)(0.f, 1.f),
       (float2)(0.f, 0.f), (float2)(1.f, 1.f), (float2)(-1.f, 1.f)}};
  for (uint shape = 0; shape < 3; ++shape) {
    for (uint permutation = 0; permutation < 6; ++permutation) {
      for (uint facing = 0; facing < 2; ++facing) {
        for (uint p = 0; p < 6; ++p) {
          float z = facing ? 1.f : -1.f;
          float4 ray = (float4)(0.f, 0.f, -z, 0.f);
          uint node_base = shape == 0 ? 3328 : 3520 + (shape - 1) * 96;
          uint result_base = shape == 0 ? 56 : 368 + (shape - 1) * 288;
          ulong node = ((ulong)(out + node_base + permutation * 16)) >> 3;
          *(__global uint4 *)(out + result_base +
                              4 * (permutation * 12 + facing * 6 + p)) =
              intersect(
                  node,
                  (float4)(points[shape][p].x, points[shape][p].y, z, 0.f), ray,
                  inverse);
        }
      }
    }
  }
  for (uint side = 0; side < 2; ++side) {
    for (uint rotation = 0; rotation < 3; ++rotation) {
      uint index = side * 3 + rotation;
      float4 start = side ? (float4)(.75f, .625f, -1.f, 0.f)
                          : (float4)(.25f, .125f, -1.f, 0.f);
      ulong node = (((ulong)(out + 3424 + index * 16)) >> 3) | side;
      *(__global uint4 *)(out + 344 + index * 4) =
          intersect(node, start, (float4)(0.f, 0.f, 1.f, 0.f), inverse);
    }
  }
  const float3 directions[6] = {(float3)(1, 2, 1),  (float3)(2, 1, 1),
                                (float3)(1, 1, 2),  (float3)(-1, 2, 1),
                                (float3)(1, -2, 1), (float3)(1, 1, -2)};
  for (uint shape = 0; shape < 3; shape++) {
    for (uint d = 0; d < 6; d++) {
      for (uint facing = 0; facing < 2; facing++) {
        float3 ray = directions[d] * (facing ? -1.f : 1.f);
        for (uint p = 0; p < 3; p++) {
          float3 hit = (float3)(points[shape][p].x, points[shape][p].y, 0.f);
          uint base = shape == 0 ? 3328 : 3520 + (shape - 1) * 96;
          ulong node = ((ulong)(out + base)) >> 3;
          uint index = ((shape * 6 + d) * 2 + facing) * 3 + p;
          *(__global uint4 *)(out + 1040 + 4 * index) = intersect(
              node, (float4)(hit - ray, 0.f), (float4)(ray, 0.f), inverse);
        }
      }
    }
  }
  const float3 tied[6] = {(float3)(1, 1, 1),   (float3)(1, -1, 1),
                          (float3)(-1, 1, 1),  (float3)(1, 1, -1),
                          (float3)(-1, -1, 1), (float3)(1, -1, -1)};
  for (uint shape = 0; shape < 3; shape++)
    for (uint d = 0; d < 6; d++)
      for (uint facing = 0; facing < 2; facing++)
        for (uint p = 0; p < 3; p++) {
          float3 ray = tied[d] * (facing ? -1.f : 1.f);
          float3 hit = (float3)(points[shape][p].x, points[shape][p].y, 0.f);
          uint base = shape == 0 ? 3328 : 3520 + (shape - 1) * 96;
          uint index = ((shape * 6 + d) * 2 + facing) * 3 + p;
          *(__global uint4 *)(out + 1472 + 4 * index) =
              intersect(((ulong)(out + base)) >> 3, (float4)(hit - ray, 0.f),
                        (float4)(ray, 0.f), inverse);
        }
  ulong regular = ((ulong)(out + 3200)) >> 3,
        degenerate = ((ulong)(out + 3840)) >> 3;
  *(__global uint4 *)(out + 1904) = intersect(
      regular, (float4)(.25f, .25f, -1, 0), (float4)(1, 0, 0, 0), inverse);
  *(__global uint4 *)(out + 1908) = intersect(
      regular, (float4)(.25f, .25f, 0, 0), (float4)(1, 0, 0, 0), inverse);
  *(__global uint4 *)(out + 1912) =
      intersect(regular, (float4)(2, 2, -1, 0), (float4)(1, 0, 0, 0), inverse);
  *(__global uint4 *)(out + 1916) = intersect(
      degenerate, (float4)(.25f, .25f, -1, 0), (float4)(0, 0, 1, 0), inverse);
  *(__global uint4 *)(out + 1920) = intersect(
      degenerate, (float4)(.25f, 0, -1, 0), (float4)(0, 0, 1, 0), inverse);
  *(__global uint4 *)(out + 1924) = intersect(
      degenerate, (float4)(.25f, 0, 0, 0), (float4)(1, 0, 0, 0), inverse);
  *(__global uint4 *)(out + 1928) = intersect(
      regular, (float4)(.25f, .25f, -1, 0), (float4)(-1, 0, 0, 0), inverse);
  *(__global uint4 *)(out + 1932) = intersect(
      regular, (float4)(.25f, .25f, -1, 0), (float4)(0, -1, 0, 0), inverse);
  *(__global uint4 *)(out + 1936) = intersect(
      degenerate, (float4)(.25f, .25f, -1, 0), (float4)(0, 0, -1, 0), inverse);
  *(__global uint4 *)(out + 1940) =
      intersect(regular, (float4)(.25f, .25f, -1, 0),
                (float4)(0, 0, as_float(1u), 0), inverse);
  *(__global uint4 *)(out + 1944) =
      intersect(regular, (float4)(.25f, .25f, -1, 0),
                (float4)(0, 0, as_float(0x00800000u), 0), inverse);
  *(__global uint4 *)(out + 1948) =
      intersect(regular, (float4)(.25f, .25f, -1, 0),
                (float4)(0, 0, as_float(0x7f800000u), 0), inverse);
  *(__global uint4 *)(out + 1952) = intersect(
      regular, (float4)(.25f, .25f, -1, 0), (float4)(0, 0, 0, 0), inverse);
  *(__global uint4 *)(out + 1956) =
      intersect(regular, (float4)(.25f, .25f, as_float(0xff800000u), 0),
                (float4)(0, 0, 1, 0), inverse);
  const float4 slab_origins[10] = {
      (float4)(0, 0, -4, 0), (float4)(-1, 0, -4, 0),  (float4)(1, 0, -4, 0),
      (float4)(2, 0, -4, 0), (float4)(0, -1, -4, 0),  (float4)(0, 1, -4, 0),
      (float4)(0, 2, -4, 0), (float4)(-1, -1, -4, 0), (float4)(1, 1, -4, 0),
      (float4)(0, 0, 2, 0)};
  for (uint sign = 0; sign < 2; sign++)
    for (uint n = 0; n < 10; n++) {
      float inf = as_float(sign ? 0xff800000u : 0x7f800000u);
      *(__global uint4 *)(out + 1960 + 4 * (sign * 10 + n)) =
          intersect((((ulong)(out + 3872)) >> 3) | 5ul, slab_origins[n],
                    (float4)(0, 0, 1, 0), (float4)(inf, inf, 1, 0));
    }
  const uint grows[6] = {0, 1, 2, 3, 4, 8};
  for (uint test = 0; test < 15; test++)
    for (uint g = 0; g < 6; g++) {
      uint4 descriptor =
          (uint4)(0, 0x80000000u | (grows[g] << 23), 0xffffffffu, 0x810003ffu);
      ulong node = (((ulong)(out + 5504 + test * 32)) >> 3) | 5ul;
      *(__global uint4 *)(out + 2040 + 4 * (test * 6 + g)) =
          __builtin_amdgcn_image_bvh_intersect_ray_l(
              node, 10000.f, (float4)(0, 0, 0, 0), (float4)(1, 1, 1, 0),
              (float4)(1, 1, 1, 0), descriptor);
    }
  const float precision_origins[4] = {.25f, .125f, .375f, .1251f};
  for (uint origin = 0; origin < 4; origin++)
    for (uint test = 0; test < 16; test++) {
      ulong node = ((ulong)(out + 7296 + test * 16)) >> 3;
      float x = precision_origins[origin];
      *(__global uint4 *)(out + 2540 + 4 * (origin * 16 + test)) =
          intersect(node, (float4)(x, x, -1.f, 0), (float4)(0, 0, 1.f, 0),
                    (float4)(1, 1, 1, 0));
    }
  *(__global uint4 *)(out + 2800) =
      intersect(((ulong)(out + 7552)) >> 3, (float4)(.2f, .3f, -1.f, 0.f),
                (float4)(.1f, .2f, 1.f, 0.f), (float4)(1.f, 1.f, 1.f, 0.f));
  const int scale_exponents[8] = {-120, -100, -60, -20, 0, 20, 60, 100};
  for (uint i = 0; i < 8; i++) {
    float scale = as_float((uint)(scale_exponents[i] + 127) << 23);
    ulong node = ((ulong)(out + 8000 + i * 16)) >> 3;
    *(__global uint4 *)(out + 2816 + i * 4) =
        intersect(node, (float4)(scale * .25f, scale * .25f, -scale, 0),
                  (float4)(0, 0, 1, 0), (float4)(1, 1, 1, 0));
  }
  for (uint i = 0; i < 10; i++) {
    __global uint *record = out + 8192 + i * 16;
    float sx = as_float(record[3]), sy = as_float(record[7]);
    float sz = as_float(record[11]);
    *(__global uint4 *)(out + 2852 + i * 4) =
        intersect(((ulong)record) >> 3, (float4)(sx * .25f, sy * .25f, -sz, 0),
                  (float4)(0, 0, 1, 0), (float4)(1, 1, 1, 0));
  }
  if (out[3000] == 1u) {
    const float4 origins[8] = {
        (float4)(.25f, .25f, -4.f, 0.f), (float4)(0, 0, -4, 0),
        (float4)(100, 100, -4, 0),       (float4)(0, 0, -4, 0),
        (float4)(0, 0, 12, 0),           (float4)(0, 0, 3, 0),
        (float4)(-10, 5, -40, 0),        (float4)(5, -10, 40, 0)};
    const float4 inverses[8] = {
        (float4)(1, 1, 1, 0),          (float4)(.25f, .25f, 1, 0),
        (float4)(1, 1, 1, 0),          (float4)(.25f, .25f, 1, 0),
        (float4)(-.25f, -.25f, -1, 0), (float4)(.25f, .25f, 1, 0),
        (float4)(-2, .5f, 1, 0),       (float4)(2, -.5f, -1, 0)};
    for (uint i = 0; i < 8; i++) {
      ulong node = (((ulong)(out + 7680)) >> 3) | (out[3001] ? 5ul : 4ul);
      *(__global uint4 *)(out + 2500 + 4 * i) =
          intersect(node, origins[i], (float4)(1, 1, 1, 0), inverses[i]);
    }
  }
  if (out[3000] == 2u)
    for (uint test = 0; test < 6; test++) {
      __global uint *record = out + 7680 + test * 32;
      float4 origin = (float4)(as_float(record[16]), as_float(record[17]),
                               as_float(record[18]), 0.f);
      float4 dir = (float4)(as_float(record[19]), as_float(record[20]),
                            as_float(record[21]), 0.f);
      *(__global uint4 *)(out + 2500 + test * 4) =
          intersect((((ulong)record) >> 3) | record[22], origin, dir,
                    (float4)(1, 1, 1, 0));
    }
  for (uint test = 0; test < 48; test++) {
    uint base = test < 24 ? 4032 + test * 48 : 6144 + (test - 24) * 48;
    __global uint *record = out + base;
    float4 origin = (float4)(as_float(record[32]), as_float(record[33]),
                             as_float(record[34]), 0.f);
    float4 inverse = (float4)(as_float(record[35]), as_float(record[36]),
                              as_float(record[37]), 0.f);
    uint4 descriptor = (uint4)(0u, record[39] << 31, 0xffffffffu, 0x810003ffu);
    ulong node = (((ulong)record) >> 3) | (record[40] ? 5ul : 4ul);
    uint result_base = test < 24 ? 944 + test * 4 : 2400 + (test - 24) * 4;
    *(__global uint4 *)(out + result_base) =
        __builtin_amdgcn_image_bvh_intersect_ray_l(
            node, as_float(record[38]), origin, (float4)(1.f, 1.f, 1.f, 0.f),
            inverse, descriptor);
  }
}
