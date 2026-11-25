#pragma OPENCL FP_CONTRACT OFF
__attribute__((reqd_work_group_size(64, 1, 1))) __kernel void
probe(__global uint *out) {
  uint id = __builtin_amdgcn_workgroup_id_x() * 64u +
            __builtin_amdgcn_workitem_id_x();
  float ox = .2f + (float)(id & 1023u) * .0001f;
  uint4 desc = (uint4)(0, 0x80000000u, 0xffffffffu, 0x810003ffu);
  ulong node = ((ulong)(out + 262144)) >> 3;
  *(__global uint4 *)(out + 4 * id) =
      __builtin_amdgcn_image_bvh_intersect_ray_l(
          node, 100.f, (float4)(ox, .3f, -1.f, 0), (float4)(.1f, .2f, 1.f, 0),
          (float4)(1, 1, 1, 0), desc);
}
