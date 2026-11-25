#include <hsa/hsa.h>
#include <hsa/hsa_ext_amd.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define CHK(x)                                                                 \
  do {                                                                         \
    hsa_status_t s = (x);                                                      \
    if (s != HSA_STATUS_SUCCESS) {                                             \
      const char *msg = 0;                                                     \
      hsa_status_string(s, &msg);                                              \
      fprintf(stderr, "%s: %s (%d)\n", #x, msg, s);                            \
      exit(1);                                                                 \
    }                                                                          \
  } while (0)
static hsa_agent_t gpu, cpu;
static hsa_region_t region;
static hsa_status_t each_agent(hsa_agent_t a, void *p) {
  hsa_device_type_t type;
  hsa_agent_get_info(a, HSA_AGENT_INFO_DEVICE, &type);
  if (type == HSA_DEVICE_TYPE_GPU)
    gpu = a;
  if (type == HSA_DEVICE_TYPE_CPU)
    cpu = a;
  return HSA_STATUS_SUCCESS;
}
static hsa_status_t each_region(hsa_region_t r, void *p) {
  hsa_region_segment_t seg;
  uint32_t flags;
  hsa_region_get_info(r, HSA_REGION_INFO_SEGMENT, &seg);
  if (seg == HSA_REGION_SEGMENT_GLOBAL) {
    hsa_region_get_info(r, HSA_REGION_INFO_GLOBAL_FLAGS, &flags);
    if (flags & HSA_REGION_GLOBAL_FLAG_KERNARG)
      region = r;
  }
  return HSA_STATUS_SUCCESS;
}
static uint32_t next_random(uint32_t *state) {
  *state = *state * 1664525u + 1013904223u;
  return *state;
}
static float smaller(float a, float b) { return a < b ? a : b; }
static float larger(float a, float b) { return a > b ? a : b; }
static uint32_t bits(float value) {
  uint32_t result;
  memcpy(&result, &value, 4);
  return result;
}
static int write_edge_record(FILE *file, const uint32_t *node,
                             const float origin[3], const float direction[3],
                             const uint32_t *result) {
  uint32_t side = 0;
  return fwrite(node, 4, 16, file) == 16 && fwrite(origin, 4, 3, file) == 3 &&
         fwrite(direction, 4, 3, file) == 3 && fwrite(&side, 4, 1, file) == 1 &&
         fwrite(result, 4, 4, file) == 4;
}
static int dump_edges(FILE *file, const uint32_t *out) {
  static const float points[3][6][2] = {
      {{0, .5f}, {.5f, 0}, {.5f, .5f}, {0, 0}, {1, 0}, {0, 1}},
      {{0, -.5f}, {.5f, 0}, {.5f, -.5f}, {0, 0}, {1, 0}, {0, -1}},
      {{-.5f, .5f}, {.5f, .5f}, {0, 1}, {0, 0}, {1, 1}, {-1, 1}}};
  static const float dirs[2][6][3] = {
      {{1, 2, 1}, {2, 1, 1}, {1, 1, 2}, {-1, 2, 1}, {1, -2, 1}, {1, 1, -2}},
      {{1, 1, 1},
       {1, -1, 1},
       {-1, 1, 1},
       {1, 1, -1},
       {-1, -1, 1},
       {1, -1, -1}}};
  if (fwrite("TXBVE001", 1, 8, file) != 8)
    return 0;
  for (unsigned shape = 0; shape < 3; shape++)
    for (unsigned p = 0; p < 6; p++)
      for (unsigned face = 0; face < 2; face++)
        for (unsigned n = 0; n < 6; n++) {
          unsigned node = shape == 0 ? 3328 : 3520 + (shape - 1) * 96;
          unsigned result = shape == 0 ? 56 : 368 + (shape - 1) * 288;
          float z = face ? 1.f : -1.f;
          float origin[3] = {points[shape][n][0], points[shape][n][1], z};
          float direction[3] = {0, 0, -z};
          if (!write_edge_record(file, out + node + p * 16, origin, direction,
                                 out + result + 4 * (p * 12 + face * 6 + n)))
            return 0;
        }
  for (unsigned family = 0; family < 2; family++)
    for (unsigned shape = 0; shape < 3; shape++)
      for (unsigned d = 0; d < 6; d++)
        for (unsigned face = 0; face < 2; face++)
          for (unsigned n = 0; n < 3; n++) {
            float sign = face ? -1.f : 1.f;
            float direction[3] = {sign * dirs[family][d][0],
                                  sign * dirs[family][d][1],
                                  sign * dirs[family][d][2]};
            float origin[3] = {points[shape][n][0] - direction[0],
                               points[shape][n][1] - direction[1],
                               -direction[2]};
            unsigned node = shape == 0 ? 3328 : 3520 + (shape - 1) * 96;
            unsigned result = (family ? 1472 : 1040) +
                              4 * (((shape * 6 + d) * 2 + face) * 3 + n);
            if (!write_edge_record(file, out + node, origin, direction,
                                   out + result))
              return 0;
          }
  return 1;
}
static int dump_degenerate(FILE *file, const uint32_t *out) {
  static const float origins[9][3] = {
      {.25f, .25f, -1}, {.25f, .25f, 0},  {2, 2, -1},
      {.25f, .25f, -1}, {.25f, 0, -1},    {.25f, 0, 0},
      {.25f, .25f, -1}, {.25f, .25f, -1}, {.25f, .25f, -1}};
  static const float directions[9][3] = {{1, 0, 0},  {1, 0, 0},  {1, 0, 0},
                                         {0, 0, 1},  {0, 0, 1},  {1, 0, 0},
                                         {-1, 0, 0}, {0, -1, 0}, {0, 0, -1}};
  if (fwrite("TXBVD001", 1, 8, file) != 8)
    return 0;
  for (unsigned i = 0; i < 9; i++) {
    unsigned node = (i >= 3 && i <= 5) || i == 8 ? 3840 : 3200;
    if (!write_edge_record(file, out + node, origins[i], directions[i],
                           out + 1904 + 4 * i))
      return 0;
  }
  return 1;
}
static int dump_scales(FILE *file, const uint32_t *out) {
  static const int exponent[8] = {-120, -100, -60, -20, 0, 20, 60, 100};
  if (fwrite("TXBVS001", 1, 8, file) != 8)
    return 0;
  for (unsigned i = 0; i < 8; i++) {
    uint32_t value = (uint32_t)(exponent[i] + 127) << 23;
    float scale;
    memcpy(&scale, &value, 4);
    float origin[3] = {scale * .25f, scale * .25f, -scale};
    float direction[3] = {0, 0, 1};
    if (!write_edge_record(file, out + 8000 + i * 16, origin, direction,
                           out + 2816 + i * 4))
      return 0;
  }
  return 1;
}
static const int mixed_exponents[10][3] = {
    {100, -100, 0}, {120, -120, 0}, {110, -100, 0}, {-100, 100, 0},
    {100, 0, -100}, {0, 100, -100}, {-120, 120, 0}, {60, -60, 0},
    {0, 0, -120},   {0, 0, 100}};
static const int underflow_exponents[10][3] = {
    {-63, -64, 0}, {-63, -63, 0}, {-62, -63, 0},   {-30, -96, 0}, {-80, -46, 0},
    {-126, 0, 0},  {-125, 0, 0},  {-100, -30, 20}, {0, 0, -126},  {0, 0, 125}};
static int dump_mixed(FILE *file, const uint32_t *out, int underflow) {
  if (fwrite(underflow ? "TXBVU001" : "TXBVM001", 1, 8, file) != 8)
    return 0;
  for (unsigned i = 0; i < 10; i++) {
    const uint32_t *node = out + 8192 + i * 16;
    float sx, sy, sz;
    memcpy(&sx, node + 3, 4);
    memcpy(&sy, node + 7, 4);
    memcpy(&sz, node + 11, 4);
    float origin[3] = {sx * .25f, sy * .25f, -sz};
    float direction[3] = {0, 0, 1};
    if (!write_edge_record(file, node, origin, direction, out + 2852 + i * 4))
      return 0;
  }
  return 1;
}
int main(int argc, char **argv) {
  if (argc != 2 &&
      !(argc == 4 && (strcmp(argv[2], "--dump") == 0 ||
                      strcmp(argv[2], "--dump-triangles") == 0 ||
                      strcmp(argv[2], "--dump-precision") == 0 ||
                      strcmp(argv[2], "--dump-fractional") == 0 ||
                      strcmp(argv[2], "--dump-edges") == 0 ||
                      strcmp(argv[2], "--dump-degenerate") == 0 ||
                      strcmp(argv[2], "--dump-scales") == 0 ||
                      strcmp(argv[2], "--dump-mixed") == 0 ||
                      strcmp(argv[2], "--dump-underflow") == 0)) &&
      !(argc == 5 && (strcmp(argv[2], "--retail") == 0 ||
                      strcmp(argv[2], "--retail-triangles") == 0 ||
                      strcmp(argv[2], "--retail-oblique") == 0 ||
                      strcmp(argv[2], "--retail-tied") == 0 ||
                      strcmp(argv[2], "--retail-miss") == 0 ||
                      strcmp(argv[2], "--retail-edge") == 0 ||
                      strcmp(argv[2], "--retail-nearedge") == 0))) {
    fprintf(stderr,
            "usage: %s probe.hsaco "
            "[--dump|--dump-triangles|--dump-precision|--dump-fractional|--"
            "dump-edges|--dump-degenerate|--dump-scales|--dump-mixed|--dump-"
            "underflow "
            "output.bin | "
            "--retail|--retail-triangles|--retail-oblique|--retail-tied|--"
            "retail-miss|--retail-edge|--retail-nearedge "
            "serialized.bin "
            "output.bin]\n",
            argv[0]);
    return 2;
  }
  CHK(hsa_init());
  CHK(hsa_iterate_agents(each_agent, 0));
  if (!gpu.handle || !cpu.handle) {
    fprintf(stderr, "agent/region absent\n");
    return 1;
  }
  CHK(hsa_agent_iterate_regions(cpu, each_region, 0));
  if (!region.handle) {
    fprintf(stderr, "kernarg region absent\n");
    return 1;
  }
  char agent_name[64] = {0};
  CHK(hsa_agent_get_info(gpu, HSA_AGENT_INFO_NAME, agent_name));
  if (strcmp(agent_name, "gfx1036") != 0) {
    fprintf(stderr, "this probe was validated for gfx1036, got %s\n",
            agent_name);
    return 1;
  }
  hsa_queue_t *queue;
  CHK(hsa_queue_create(gpu, 64, HSA_QUEUE_TYPE_SINGLE, NULL, NULL, UINT32_MAX,
                       UINT32_MAX, &queue));
  FILE *f = fopen(argv[1], "rb");
  if (!f)
    return 2;
  if (fseek(f, 0, SEEK_END) != 0)
    return 2;
  long file_size = ftell(f);
  if (file_size <= 0)
    return 2;
  size_t len = (size_t)file_size;
  rewind(f);
  void *data = malloc(len);
  if (!data || fread(data, 1, len, f) != len)
    return 2;
  fclose(f);
  hsa_code_object_reader_t reader;
  CHK(hsa_code_object_reader_create_from_memory(data, len, &reader));
  hsa_executable_t exe;
  CHK(hsa_executable_create_alt(
      HSA_PROFILE_FULL, HSA_DEFAULT_FLOAT_ROUNDING_MODE_DEFAULT, "", &exe));
  CHK(hsa_executable_load_agent_code_object(exe, gpu, reader, "", NULL));
  CHK(hsa_executable_freeze(exe, ""));
  hsa_executable_symbol_t sym;
  CHK(hsa_executable_get_symbol_by_name(exe, "probe.kd", &gpu, &sym));
  uint64_t obj;
  uint32_t group, priv, arg_size;
  CHK(hsa_executable_symbol_get_info(
      sym, HSA_EXECUTABLE_SYMBOL_INFO_KERNEL_OBJECT, &obj));
  CHK(hsa_executable_symbol_get_info(
      sym, HSA_EXECUTABLE_SYMBOL_INFO_KERNEL_GROUP_SEGMENT_SIZE, &group));
  CHK(hsa_executable_symbol_get_info(
      sym, HSA_EXECUTABLE_SYMBOL_INFO_KERNEL_PRIVATE_SEGMENT_SIZE, &priv));
  CHK(hsa_executable_symbol_get_info(
      sym, HSA_EXECUTABLE_SYMBOL_INFO_KERNEL_KERNARG_SEGMENT_SIZE, &arg_size));
  void *out, *args;
  CHK(hsa_memory_allocate(region, 49152, &out));
  if ((uintptr_t)out % 256 != 0) {
    fprintf(stderr, "HSA allocation is not 256-byte aligned\n");
    return 1;
  }
  CHK(hsa_amd_agents_allow_access(1, &gpu, NULL, out));
  CHK(hsa_memory_allocate(region, arg_size, &args));
  CHK(hsa_amd_agents_allow_access(1, &gpu, NULL, args));
  *(void **)args = out;
  memset(out, 0, 49152);
  uint32_t *node = (uint32_t *)out + 3072;
  node[0] = 0x9;
  node[1] = 0x12;
  node[2] = 0x1b;
  node[3] = 0x24;
  float bounds[24] = {10, 10, 2, 12, 12, 4, -1, -1, 2, 1, 1, 4,
                      -1, -1, 2, 1,  1,  4, -1, -1, 2, 1, 1, 4};
  memcpy(node + 4, bounds, sizeof(bounds));
  uint32_t *halfnode = (uint32_t *)out + 3136;
  memcpy(halfnode, node, 16);
  _Float16 *halfbounds = (_Float16 *)(halfnode + 4);
  for (int i = 0; i < 24; i++)
    halfbounds[i] = bounds[i];
  uint32_t *tri = (uint32_t *)out + 3200;
  float verts[12] = {0, 0, 0, 1, 0, 0, 0, 1, 0, 1, 1, 0};
  memcpy(tri, verts, sizeof(verts));
  tri[12] = 0x80;
  tri[13] = 1;
  tri[14] = 2;
  tri[15] = 0x0909;
  const float endpoint_inputs[7] = {.5f, 1.f, 2.f, 100.f, .25f, .25f, -1.f};
  memcpy((uint32_t *)out + 8400, endpoint_inputs, sizeof(endpoint_inputs));
  const float zero_rays[24] = {.25f, .25f, 1, .25f, .25f, -1,
                               0, .5f, 1, 0, .5f, -1,
                               .5f, 0, 1, .5f, 0, -1,
                               .5f, .5f, 1, .5f, .5f, -1};
  memcpy((uint32_t *)out + 8500, zero_rays, sizeof(zero_rays));
  ((uint32_t *)out)[8524] = bits(2.f);
  uint32_t *scaled = (uint32_t *)out + 3264;
  float scaled_verts[12] = {0, 0, 0, 2, 0, 0, 0, 2, 0, 2, 2, 0};
  memcpy(scaled, scaled_verts, sizeof(scaled_verts));
  scaled[12] = 0x80;
  scaled[13] = 1;
  scaled[14] = 2;
  scaled[15] = 0x0909;
  static const unsigned permutations[6][3] = {{0, 1, 2}, {0, 2, 1}, {1, 0, 2},
                                              {1, 2, 0}, {2, 0, 1}, {2, 1, 0}};
  static const float vertices[3][3][3] = {{{0, 0, 0}, {1, 0, 0}, {0, 1, 0}},
                                          {{0, 0, 0}, {1, 0, 0}, {0, -1, 0}},
                                          {{0, 0, 0}, {1, 1, 0}, {-1, 1, 0}}};
  for (unsigned shape = 0; shape < 3; shape++) {
    for (unsigned p = 0; p < 6; p++) {
      unsigned base = shape == 0 ? 3328 : 3520 + (shape - 1) * 96;
      uint32_t *dst = (uint32_t *)out + base + p * 16;
      for (unsigned v = 0; v < 3; v++)
        memcpy(dst + v * 3, vertices[shape][permutations[p][v]],
               3 * sizeof(float));
      dst[15] = 9;
    }
  }
  const unsigned selectors[3] = {9, 2, 4};
  for (unsigned side = 0; side < 2; side++) {
    for (unsigned r = 0; r < 3; r++) {
      uint32_t *dst = (uint32_t *)out + 3424 + (side * 3 + r) * 16;
      memcpy(dst, tri, 64);
      unsigned shift = side * 8;
      dst[15] = (0x0909u & ~(0xffu << shift)) | (selectors[r] << shift);
    }
  }
  // Seeded finite inputs. Each child is placed around
  // the ray, with some deliberately missed, invalid, or clipped by extent.
  uint32_t random_expected[48][4];
  uint32_t seed = 0x74757862u;
  static const float inverses[6] = {-4.f, -2.f, -1.f, 1.f, 2.f, 4.f};
  static const float fractional[6] = {-2.3f, -1.3f, -.7f, .3f, 1.3f, 2.3f};
  for (unsigned test = 0; test < 48; test++) {
    unsigned base = test < 24 ? 4032 + test * 48 : 6144 + (test - 24) * 48;
    uint32_t *record = (uint32_t *)out + base;
    float origin[3], inverse[3], keys[4];
    for (unsigned axis = 0; axis < 3; axis++) {
      origin[axis] = (float)((int)(next_random(&seed) % 9) - 4);
      if (test >= 24)
        origin[axis] += .1f;
      unsigned choice = next_random(&seed) % 6;
      inverse[axis] = test < 24 ? inverses[choice] : fractional[choice];
      record[32 + axis] = bits(origin[axis]);
      record[35 + axis] = bits(inverse[axis]);
    }
    float extent = (test % 3 == 0) ? 3.f : 16.f;
    int sort = test % 3 != 1;
    record[38] = bits(extent);
    record[39] = sort;
    record[40] = test & 1;
    for (unsigned child = 0; child < 4; child++) {
      record[child] = 9 + child * 9;
      if (child == 3 && test % 4 == 0)
        record[child] = 0xffffffffu;
      float lower[3], upper[3];
      float at = (float)(1 + next_random(&seed) % 7);
      for (unsigned axis = 0; axis < 3; axis++) {
        float center = origin[axis] + at / inverse[axis];
        if (child == 2 && axis == 0 && test % 2 == 0)
          center += 24.f;
        lower[axis] = center - 1.f;
        upper[axis] = center + 1.f;
        if (test & 1) {
          ((float *)(record + 4 + child * 6))[axis] = lower[axis];
          ((float *)(record + 4 + child * 6))[axis + 3] = upper[axis];
        } else {
          ((_Float16 *)(record + 4 + child * 3))[axis] = (_Float16)lower[axis];
          ((_Float16 *)(record + 4 + child * 3))[axis + 3] =
              (_Float16)upper[axis];
          lower[axis] = (float)((_Float16)lower[axis]);
          upper[axis] = (float)((_Float16)upper[axis]);
        }
      }
      float near = 0.f, far = extent;
      for (unsigned axis = 0; axis < 3; axis++) {
        float t0 = (lower[axis] - origin[axis]) * inverse[axis];
        float t1 = (upper[axis] - origin[axis]) * inverse[axis];
        near = larger(near, smaller(t0, t1));
        far = smaller(far, larger(t0, t1));
      }
      random_expected[test][child] = near <= far ? record[child] : 0xffffffffu;
      keys[child] = near;
    }
    if (sort) {
      static const unsigned left[5] = {0, 1, 0, 2, 1},
                            right[5] = {2, 3, 1, 3, 2};
      for (unsigned i = 0; i < 5; i++) {
        unsigned a = left[i], b = right[i];
        if (random_expected[test][a] == 0xffffffffu ||
            (random_expected[test][b] != 0xffffffffu && keys[b] < keys[a])) {
          uint32_t temp = random_expected[test][a];
          random_expected[test][a] = random_expected[test][b];
          random_expected[test][b] = temp;
          float key = keys[a];
          keys[a] = keys[b];
          keys[b] = key;
        }
      }
    }
  }
  float degenerate[12] = {0, 0, 0, 1, 0, 0, 2, 0, 0, 0, 0, 0};
  uint32_t *degenerate_node = (uint32_t *)out + 3840;
  memcpy(degenerate_node, degenerate, sizeof(degenerate));
  degenerate_node[15] = 9;
  uint32_t *slab = (uint32_t *)out + 3872;
  slab[0] = 9;
  slab[1] = 18;
  slab[2] = 27;
  slab[3] = 36;
  float slab_bounds[24] = {-1, -1, 2, 1, 1, 4, -1, -1, 3, 1,  1, 5,
                           1,  -1, 2, 2, 1, 4, -2, -2, 2, -1, 2, 4};
  memcpy(slab + 4, slab_bounds, sizeof(slab_bounds));
  static const float grow_values[5] = {.5f, 1.f, 1.5f, 2.f, 1024.f};
  for (unsigned test = 0; test < 15; test++) {
    uint32_t *box = (uint32_t *)out + 5504 + test * 32;
    box[0] = 9;
    box[1] = box[2] = box[3] = 0xffffffffu;
    float f = grow_values[test / 3];
    float values[24] = {f, 0, 0, 2 * f, f, f};
    memcpy(box + 4, values, sizeof(values));
    box[4] += test % 3;
  }
  if (argc == 5) {
    FILE *source = fopen(argv[3], "rb");
    if (!source) {
      perror("open serialized BLAS");
      return 1;
    }
    if (fseek(source, 0, SEEK_END) != 0)
      return 1;
    long length = ftell(source);
    if (length < 120)
      return 1;
    if (strcmp(argv[2], "--retail") == 0) {
      uint32_t root;
      if (fseek(source, 112, SEEK_SET) != 0 ||
          fread(&root, sizeof(root), 1, source) != 1)
        return 1;
      uint32_t kind = root & 7u;
      uint64_t offset = 64u + (uint64_t)(root >> 3) * 64u;
      uint64_t size = kind == 4 ? 64u : kind == 5 ? 128u : 0u;
      if (!size || offset > (uint64_t)length ||
          size > (uint64_t)length - offset ||
          fseek(source, (long)offset, SEEK_SET) != 0 ||
          fread((uint32_t *)out + 7680, 1, size, source) != size) {
        fprintf(stderr, "invalid serialized root record\n");
        return 1;
      }
      ((uint32_t *)out)[3000] = 1;
      ((uint32_t *)out)[3001] = kind == 5;
    } else {
      uint32_t leaf_offset, leaf_count;
      if (fseek(source, 76, SEEK_SET) != 0 ||
          fread(&leaf_count, 4, 1, source) != 1 ||
          fseek(source, 116, SEEK_SET) != 0 ||
          fread(&leaf_offset, 4, 1, source) != 1)
        return 1;
      uint64_t start = 64u + (uint64_t)leaf_offset;
      if ((leaf_offset & 63u) || start > (uint64_t)length ||
          leaf_count > ((uint64_t)length - start) / 64u) {
        fprintf(stderr, "invalid serialized leaf range\n");
        return 1;
      }
      unsigned found = 0;
      for (uint32_t n = 0; n < leaf_count && found < 6; n++) {
        uint32_t leaf[16];
        float vertices[12];
        if (fseek(source, (long)(start + (uint64_t)n * 64u), SEEK_SET) != 0 ||
            fread(leaf, 4, 16, source) != 16)
          return 1;
        unsigned side = found & 1u;
        unsigned selector = (leaf[15] >> (side * 8)) & 0xffu;
        if (!(leaf[12] & 0x80u) || (leaf[12] & 0x7fu) ||
            (selector != 9 && selector != 2 && selector != 4) ||
            leaf[14] == 0xffffffffu || (leaf[15] >> 16))
          continue;
        memcpy(vertices, leaf, sizeof(vertices));
        int finite = 1;
        for (unsigned i = 0; i < 12; i++)
          finite &= isfinite(vertices[i]);
        if (!finite)
          continue;
        unsigned ia = side ? 1 : 0, ib = side ? 3 : 1, ic = 2;
        float area = (vertices[3 * ib] - vertices[3 * ia]) *
                         (vertices[3 * ic + 1] - vertices[3 * ia + 1]) -
                     (vertices[3 * ib + 1] - vertices[3 * ia + 1]) *
                         (vertices[3 * ic] - vertices[3 * ia]);
        if (!isfinite(area) || fabsf(area) < .0001f)
          continue;
        uint32_t *record = (uint32_t *)out + 7680 + found * 32;
        memcpy(record, leaf, 64);
        static const float directions[6][3] = {{0, 0, 2}, {0, 0, .5f},
                                               {1, 2, 1}, {2, 1, 1},
                                               {1, 1, 2}, {-1, -2, -1}};
        static const float tied_directions[6][3] = {{1, 1, 1},   {1, -1, 1},
                                                    {-1, 1, 1},  {1, 1, -1},
                                                    {-1, -1, 1}, {1, -1, -1}};
        int miss = strcmp(argv[2], "--retail-miss") == 0;
        int near_edge = strcmp(argv[2], "--retail-nearedge") == 0;
        int edge = strcmp(argv[2], "--retail-edge") == 0 || near_edge;
        int oblique = strcmp(argv[2], "--retail-oblique") == 0 || miss || edge;
        int tied = strcmp(argv[2], "--retail-tied") == 0;
        for (unsigned axis = 0; axis < 3; axis++) {
          float direction = oblique ? directions[found][axis]
                            : tied  ? tied_directions[found][axis]
                                    : (axis == 2 ? 1.f : 0.f);
          float target = edge ? .5f * vertices[3 * ib + axis] +
                                    .5f * vertices[3 * ic + axis]
                              : .5f * vertices[3 * ia + axis] +
                                    .375f * vertices[3 * ib + axis] +
                                    .125f * vertices[3 * ic + axis];
          float origin = target - 16.f * direction;
          if (miss && axis == 0)
            origin += 300.f;
          uint32_t origin_bits = bits(origin);
          if (near_edge && axis == 0) {
            int up = (found & 1u) == 0;
            if ((origin_bits & 0x7fffffffu) == 0)
              origin_bits = up ? 1u : 0x80000001u;
            else if (((origin_bits >> 31) == 0) == up)
              origin_bits++;
            else
              origin_bits--;
          }
          record[16 + axis] = origin_bits;
          record[19 + axis] = bits(direction);
        }
        record[22] = side;
        found++;
      }
      if (found != 6) {
        fprintf(stderr, "six supported paired leaves not found\n");
        return 1;
      }
      ((uint32_t *)out)[3000] = 2;
    }
    if (fclose(source) != 0)
      return 1;
  }
  for (unsigned test = 0; test < 16; test++) {
    uint32_t *precision = (uint32_t *)out + 7296 + test * 16;
    float vertices[12] = {0, 0, 0, 1, 0, 0, 0, 1, 0, 1, 1, 0};
    memcpy(precision, vertices, sizeof(vertices));
    precision[3] = 0x3f800000u + test;
    precision[15] = 9;
  }
  uint32_t *fractional_node = (uint32_t *)out + 7552;
  float fractional_vertices[12] = {0, 0,      0, 1.127f, 0, 0,
                                   0, 1.133f, 0, 1,      1, 0};
  memcpy(fractional_node, fractional_vertices, sizeof(fractional_vertices));
  fractional_node[12] = 0x80;
  fractional_node[14] = 1;
  fractional_node[15] = 0x0909;
  static const int scale_exponents[8] = {-120, -100, -60, -20, 0, 20, 60, 100};
  for (unsigned i = 0; i < 8; i++) {
    uint32_t *node = (uint32_t *)out + 8000 + i * 16;
    uint32_t scale = (uint32_t)(scale_exponents[i] + 127) << 23;
    node[3] = scale;
    node[7] = scale;
    node[15] = 9;
  }
  for (unsigned i = 0; i < 10; i++) {
    uint32_t *node = (uint32_t *)out + 8192 + i * 16;
    const int (*exponents)[3] =
        argc == 4 && strcmp(argv[2], "--dump-underflow") == 0
            ? underflow_exponents
            : mixed_exponents;
    node[3] = ((uint32_t)(exponents[i][0] + 127) << 23) + 1u;
    node[7] = ((uint32_t)(exponents[i][1] + 127) << 23) + 3u;
    node[11] = (uint32_t)(exponents[i][2] + 127) << 23;
    node[15] = 9;
  }
  hsa_signal_t signal;
  CHK(hsa_signal_create(1, 0, NULL, &signal));
  uint64_t index = hsa_queue_load_write_index_relaxed(queue);
  hsa_kernel_dispatch_packet_t *packet =
      (hsa_kernel_dispatch_packet_t *)queue->base_address +
      (index & (queue->size - 1));
  memset(packet, 0, sizeof(*packet));
  packet->setup = 1 << HSA_KERNEL_DISPATCH_PACKET_SETUP_DIMENSIONS;
  packet->workgroup_size_x = 1;
  packet->workgroup_size_y = 1;
  packet->workgroup_size_z = 1;
  packet->grid_size_x = 1;
  packet->grid_size_y = 1;
  packet->grid_size_z = 1;
  packet->private_segment_size = priv;
  packet->group_segment_size = group;
  packet->kernel_object = obj;
  packet->kernarg_address = args;
  packet->completion_signal = signal;
  uint16_t header =
      (HSA_PACKET_TYPE_KERNEL_DISPATCH << HSA_PACKET_HEADER_TYPE) |
      (HSA_FENCE_SCOPE_SYSTEM << HSA_PACKET_HEADER_SCACQUIRE_FENCE_SCOPE) |
      (HSA_FENCE_SCOPE_SYSTEM << HSA_PACKET_HEADER_SCRELEASE_FENCE_SCOPE);
  __atomic_store_n(&packet->header, header, __ATOMIC_RELEASE);
  hsa_queue_store_write_index_relaxed(queue, index + 1);
  hsa_signal_store_relaxed(queue->doorbell_signal, index);
  hsa_signal_value_t result =
      hsa_signal_wait_scacquire(signal, HSA_SIGNAL_CONDITION_LT, 1,
                                5000000000ull, HSA_WAIT_STATE_BLOCKED);
  static const uint32_t expected[56] = {
      0x1b,       0x12,       0x24,       0xffffffff, 0x1b,       0x12,
      0x24,       0xffffffff, 0xbf800000, 0xbf800000, 0xbe800000, 0xbe800000,
      0xbf800000, 0xbf800000, 0xbf000000, 0xbe800000, 0xff800000, 0xbf800000,
      0xbf000000, 0xbf000000, 0xff800000, 0xbf800000, 0xc0000000, 0xc0000000,
      0x7f800000, 0x3f800000, 0x3f000000, 0x3f000000, 0x7f800000, 0x3f800000,
      0x40000000, 0x40000000, 0xff800000, 0xbf800000, 0xbf000000, 0xbf000000,
      0xbf800000, 0xbf800000, 0x00000000, 0xbf000000, 0xff800000, 0xbf800000,
      0xbf000000, 0x00000000, 0x00000000, 0xbf800000, 0xbe800000, 0xbe800000,
      0xff800000, 0xc0800000, 0xc0800000, 0xc0800000, 0x7f800000, 0x40800000,
      0x40800000, 0x40800000};
  int failed = result != 0;
  printf("agent=%s\n", agent_name);
  for (int i = 0; i < 56; i++) {
    printf("%08x%c", ((uint32_t *)out)[i], i % 4 == 3 ? '\n' : ' ');
    if (((uint32_t *)out)[i] != expected[i])
      failed = 1;
  }
  const uint32_t endpoint_expected[4] = {0xbf800000, 0xbf800000, 0xbe800000,
                                         0xbe800000};
  for (unsigned i = 0; i < 4; i++) {
    printf("triangle_endpoint_max=%.1f words=", endpoint_inputs[i]);
    for (unsigned j = 0; j < 4; j++) {
      uint32_t actual = ((uint32_t *)out)[8416 + 4 * i + j];
      printf("%s%08x", j ? "," : "", actual);
      if (actual != endpoint_expected[j])
        failed = 1;
    }
    putchar('\n');
  }
  const uint32_t zero_expected[8][4] = {
      {0, 0xbf800000, 0xbe800000, 0xbe800000},
      {0, 0x3f800000, 0x3e800000, 0x3e800000},
      {0, 0xbf800000, 0, 0xbf000000},
      {0, 0x3f800000, 0, 0x3f000000},
      {0xff800000, 0xbf800000, 0xbf000000, 0},
      {0, 0x3f800000, 0x3f000000, 0},
      {0xff800000, 0xbf800000, 0xbf000000, 0xbf000000},
      {0x7f800000, 0x3f800000, 0x3f000000, 0x3f000000}};
  for (unsigned i = 0; i < 8; i++) {
    printf("triangle_zero_%u words=", i);
    for (unsigned j = 0; j < 4; j++) {
      uint32_t actual = ((uint32_t *)out)[8448 + 4 * i + j];
      printf("%s%08x", j ? "," : "", actual);
      if (actual != zero_expected[i][j])
        failed = 1;
    }
    putchar('\n');
  }
  // Check hit status separately: these cases map the hardware's edge
  // ownership, without assuming the returned unnormalized barycentrics.
  for (unsigned shape = 0; shape < 3; shape++) {
    for (unsigned p = 0; p < 6; p++) {
      for (unsigned facing = 0; facing < 2; facing++) {
        static const unsigned hit_masks[3][2] = {
            {0x01, 0x0b}, {0x0b, 0x01}, {0x25, 0x01}};
        for (unsigned point = 0; point < 6; point++) {
          unsigned base = shape == 0 ? 56 : 368 + (shape - 1) * 288;
          uint32_t t =
              ((uint32_t *)out)[base + 4 * (p * 12 + facing * 6 + point)];
          int hit = (t & 0x7fffffffu) != 0x7f800000u;
          if (hit != ((hit_masks[shape][facing] >> point) & 1))
            failed = 1;
        }
      }
    }
  }
  static const uint32_t remap[24] = {
      0xbf800000, 0xbf800000, 0xbe800000, 0xbe000000, 0xbf800000, 0xbf800000,
      0xbe000000, 0xbf200000, 0xbf800000, 0xbf800000, 0xbf200000, 0xbe800000,
      0xbf800000, 0xbf800000, 0xbec00000, 0xbe800000, 0xbf800000, 0xbf800000,
      0xbe800000, 0xbec00000, 0xbf800000, 0xbf800000, 0xbec00000, 0xbec00000};
  for (unsigned i = 0; i < 24; i++)
    if (((uint32_t *)out)[344 + i] != remap[i])
      failed = 1;
  printf("permuted_edges=216 paired_remaps=6\n");
  unsigned random_hits = 0;
  for (unsigned test = 0; test < 48; test++) {
    for (unsigned lane = 0; lane < 4; lane++) {
      unsigned base = test < 24 ? 944 + test * 4 : 2400 + (test - 24) * 4;
      uint32_t got = ((uint32_t *)out)[base + lane];
      if (got != random_expected[test][lane])
        failed = 1;
      if (got != 0xffffffffu)
        random_hits++;
    }
  }
  printf("random_box_cases=48 hit_lanes=%u\n", random_hits);
  if (random_hits == 0)
    failed = 1;
  static const unsigned oblique_masks[3][6][2] = {
      {{4, 4}, {3, 2}, {1, 3}, {5, 5}, {2, 2}, {3, 1}},
      {{2, 2}, {5, 4}, {3, 1}, {3, 3}, {4, 4}, {1, 3}},
      {{4, 4}, {3, 3}, {5, 1}, {4, 4}, {3, 3}, {1, 5}}};
  for (unsigned shape = 0; shape < 3; shape++)
    for (unsigned d = 0; d < 6; d++)
      for (unsigned facing = 0; facing < 2; facing++)
        for (unsigned point = 0; point < 3; point++) {
          unsigned index = ((shape * 6 + d) * 2 + facing) * 3 + point;
          uint32_t t = ((uint32_t *)out)[1040 + 4 * index];
          int hit = (t & 0x7fffffffu) != 0x7f800000u;
          if (hit != ((oblique_masks[shape][d][facing] >> point) & 1))
            failed = 1;
        }
  printf("oblique_edges=108\n");
  static const unsigned tied_masks[3][6][2] = {
      {{3, 2}, {3, 2}, {3, 2}, {2, 3}, {3, 2}, {2, 3}},
      {{5, 4}, {5, 4}, {5, 4}, {4, 5}, {5, 4}, {4, 5}},
      {{3, 3}, {3, 3}, {3, 3}, {3, 3}, {3, 3}, {3, 3}}};
  for (unsigned shape = 0; shape < 3; shape++)
    for (unsigned d = 0; d < 6; d++)
      for (unsigned facing = 0; facing < 2; facing++)
        for (unsigned point = 0; point < 3; point++) {
          unsigned i = ((shape * 6 + d) * 2 + facing) * 3 + point;
          uint32_t t = ((uint32_t *)out)[1472 + 4 * i];
          int hit = (t & 0x7fffffffu) != 0x7f800000u;
          if (hit != ((tied_masks[shape][d][facing] >> point) & 1))
            failed = 1;
        }
  static const uint32_t special[36] = {
      0x7f800000, 0,          0xbf800000, 0,          0x7f800000, 0,
      0,          0,          0x7f800000, 0,          0xbf800000, 0,
      0x7f800000, 0,          0x3f000000, 0xbe800000, 0x7f800000, 0,
      0,          0,          0x7f800000, 0,          0,          0,
      0x7f800000, 0,          0x3f800000, 0,          0x7f800000, 0,
      0,          0x3f800000, 0x7f800000, 0,          0xbf000000, 0x3e800000};
  for (unsigned i = 0; i < 36; i++)
    if (((uint32_t *)out)[1904 + i] != special[i])
      failed = 1;
  printf("tied_edges=108 parallel_and_degenerate=9\n");
  static const uint32_t special_float[20] = {
      0x7f800000, 0,          0,          0,          0x7f800000, 0,          0,
      0,          0xff800000, 0xffc00000, 0xff800000, 0xff800000, 0x7f800000, 0,
      0,          0,          0x7f800000, 0x3f800000, 0,          0};
  for (unsigned i = 0; i < 20; i++)
    if (((uint32_t *)out)[1940 + i] != special_float[i])
      failed = 1;
  printf("nonfinite_and_subnormal=5\n");
  static const uint32_t slab_expected[10][4] = {
      {9, 18, 0xffffffff, 0xffffffff},
      {9, 36, 18, 0xffffffff},
      {9, 27, 18, 0xffffffff},
      {27, 0xffffffff, 0xffffffff, 0xffffffff},
      {9, 18, 0xffffffff, 0xffffffff},
      {9, 18, 0xffffffff, 0xffffffff},
      {0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff},
      {9, 36, 18, 0xffffffff},
      {9, 27, 18, 0xffffffff},
      {9, 18, 0xffffffff, 0xffffffff}};
  for (unsigned sign = 0; sign < 2; sign++)
    for (unsigned n = 0; n < 10; n++)
      for (unsigned lane = 0; lane < 4; lane++)
        if (((uint32_t *)out)[1960 + 4 * (sign * 10 + n) + lane] !=
            slab_expected[n][lane])
          failed = 1;
  printf("parallel_box_slabs=20\n");
  static const unsigned grows[6] = {0, 1, 2, 3, 4, 8};
  for (unsigned test = 0; test < 15; test++)
    for (unsigned g = 0; g < 6; g++) {
      uint32_t want = (test % 3 <= grows[g]) ? 9u : 0xffffffffu;
      unsigned base = 2040 + 4 * (test * 6 + g);
      if (((uint32_t *)out)[base] != want)
        failed = 1;
      for (unsigned lane = 1; lane < 4; lane++)
        if (((uint32_t *)out)[base + lane] != 0xffffffffu)
          failed = 1;
    }
  printf("box_growth_cases=90\n");
  for (unsigned test = 0; test < 16; test++) {
    uint32_t expected_det = 0xbf800000u + (test & ~1u);
    uint32_t *actual = (uint32_t *)out + 2540 + test * 4;
    if (actual[0] != expected_det || actual[1] != expected_det ||
        actual[2] != 0xbe800000u || actual[3] != 0xbe800000u + test)
      failed = 1;
  }
  static const int precision_offsets[3][16] = {
      {0, 0, 1, 2, 4, 4, 5, 6, 8, 8, 9, 10, 12, 12, 13, 14},
      {0, 0, 1, 2, 4, 4, 5, 6, 8, 8, 9, 10, 12, 12, 13, 14},
      {-3, -1, 1, 1, 2, 3, 5, 5, 6, 7, 9, 9, 10, 11, 13, 13}};
  for (unsigned origin = 1; origin < 4; origin++)
    for (unsigned test = 0; test < 16; test++) {
      uint32_t got = ((uint32_t *)out)[2540 + 4 * (origin * 16 + test) + 1];
      uint32_t want =
          (uint32_t)(0xbf800000ll + precision_offsets[origin - 1][test]);
      if (got != want)
        failed = 1;
    }
  static const uint32_t fractional_expected[4] = {0xbfa37127u, 0xbfa37127u,
                                                  0xbeae075eu, 0xbf104188u};
  for (unsigned i = 0; i < 4; i++)
    if (((uint32_t *)out)[2800 + i] != fractional_expected[i])
      failed = 1;
  printf("triangle_precision=64\n");
  if (failed) {
    fprintf(stderr, "instruction probe failed or timed out\n");
    return 1;
  }
  if (argc == 4) {
    FILE *dump = fopen(argv[3], "wb");
    if (!dump) {
      perror("open output");
      return 1;
    }
    int ok;
    if (strcmp(argv[2], "--dump-edges") == 0) {
      ok = dump_edges(dump, out);
    } else if (strcmp(argv[2], "--dump-degenerate") == 0) {
      ok = dump_degenerate(dump, out);
    } else if (strcmp(argv[2], "--dump-scales") == 0) {
      ok = dump_scales(dump, out);
    } else if (strcmp(argv[2], "--dump-mixed") == 0) {
      ok = dump_mixed(dump, out, 0);
    } else if (strcmp(argv[2], "--dump-underflow") == 0) {
      ok = dump_mixed(dump, out, 1);
    } else {
      int fractional_dump = strcmp(argv[2], "--dump-fractional") == 0;
      int precision = strcmp(argv[2], "--dump-precision") == 0;
      int triangles = precision || fractional_dump ||
                      strcmp(argv[2], "--dump-triangles") == 0;
      const char *magic = fractional_dump ? "TXBVF001"
                          : precision     ? "TXBVP001"
                          : triangles     ? "TXBVT001"
                                          : "TXBVH001";
      ok = fwrite(magic, 1, 8, dump) == 8;
      for (unsigned test = 0; test < (fractional_dump ? 1u
                                      : precision     ? 16u
                                      : triangles     ? 6u
                                                      : 48u) &&
                              ok;
           test++) {
        if (triangles) {
          unsigned side = (precision || fractional_dump) ? 0 : test / 3;
          uint32_t input = fractional_dump ? 7552
                           : precision     ? 7296 + test * 16
                                           : 3424 + test * 16;
          float origin[3] = {fractional_dump ? .2f
                             : side          ? .75f
                                             : .25f,
                             fractional_dump ? .3f
                             : precision     ? .25f
                             : side          ? .625f
                                             : .125f,
                             -1.f};
          uint32_t output = fractional_dump ? 2800
                            : precision     ? 2540 + test * 4
                                            : 344 + test * 4;
          float direction[3] = {fractional_dump ? .1f : 0.f,
                                fractional_dump ? .2f : 0.f, 1.f};
          ok = fwrite((uint32_t *)out + input, sizeof(uint32_t), 16, dump) ==
                   16 &&
               fwrite(origin, sizeof(float), 3, dump) == 3 &&
               fwrite(direction, sizeof(float), 3, dump) == 3 &&
               fwrite(&side, sizeof(uint32_t), 1, dump) == 1 &&
               fwrite((uint32_t *)out + output, sizeof(uint32_t), 4, dump) == 4;
        } else {
          unsigned input =
              test < 24 ? 4032 + test * 48 : 6144 + (test - 24) * 48;
          unsigned output = test < 24 ? 944 + test * 4 : 2400 + (test - 24) * 4;
          ok = fwrite((uint32_t *)out + input, sizeof(uint32_t), 48, dump) ==
                   48 &&
               fwrite((uint32_t *)out + output, sizeof(uint32_t), 4, dump) == 4;
        }
      }
    }
    if (fclose(dump) != 0 || !ok) {
      fprintf(stderr, "failed to write output\n");
      return 1;
    }
  }
  if (argc == 5 && strcmp(argv[2], "--retail") == 0) {
    static const float origins[8][3] = {
        {.25f, .25f, -4}, {0, 0, -4}, {100, 100, -4}, {0, 0, -4},
        {0, 0, 12},       {0, 0, 3},  {-10, 5, -40},  {5, -10, 40}};
    static const float inverses[8][3] = {
        {1, 1, 1},          {.25f, .25f, 1}, {1, 1, 1},    {.25f, .25f, 1},
        {-.25f, -.25f, -1}, {.25f, .25f, 1}, {-2, .5f, 1}, {2, -.5f, -1}};
    FILE *dump = fopen(argv[4], "wb");
    if (!dump) {
      perror("open output");
      return 1;
    }
    int ok = fwrite("TXBVR001", 1, 8, dump) == 8;
    for (unsigned test = 0; test < 8 && ok; test++) {
      uint32_t record[48] = {0};
      memcpy(record, (uint32_t *)out + 7680, 128);
      for (unsigned axis = 0; axis < 3; axis++) {
        record[32 + axis] = bits(origins[test][axis]);
        record[35 + axis] = bits(inverses[test][axis]);
      }
      record[38] = bits(100.f);
      record[39] = 1;
      record[40] = ((uint32_t *)out)[3001];
      ok = fwrite(record, sizeof(uint32_t), 48, dump) == 48 &&
           fwrite((uint32_t *)out + 2500 + test * 4, sizeof(uint32_t), 4,
                  dump) == 4;
    }
    if (fclose(dump) != 0 || !ok) {
      fprintf(stderr, "failed to write output\n");
      return 1;
    }
  }
  if (argc == 5 && (strcmp(argv[2], "--retail-triangles") == 0 ||
                    strcmp(argv[2], "--retail-oblique") == 0 ||
                    strcmp(argv[2], "--retail-tied") == 0 ||
                    strcmp(argv[2], "--retail-miss") == 0 ||
                    strcmp(argv[2], "--retail-edge") == 0 ||
                    strcmp(argv[2], "--retail-nearedge") == 0)) {
    FILE *dump = fopen(argv[4], "wb");
    if (!dump) {
      perror("open output");
      return 1;
    }
    int ok = fwrite("TXBVT002", 1, 8, dump) == 8;
    for (unsigned test = 0; test < 6 && ok; test++) {
      uint32_t *record = (uint32_t *)out + 7680 + test * 32;
      ok = fwrite(record, 4, 23, dump) == 23 &&
           fwrite((uint32_t *)out + 2500 + test * 4, 4, 4, dump) == 4;
    }
    if (fclose(dump) != 0 || !ok) {
      fprintf(stderr, "failed to write output\n");
      return 1;
    }
  }
  CHK(hsa_signal_destroy(signal));
  CHK(hsa_memory_free(args));
  CHK(hsa_memory_free(out));
  CHK(hsa_executable_destroy(exe));
  CHK(hsa_code_object_reader_destroy(reader));
  CHK(hsa_queue_destroy(queue));
  CHK(hsa_shut_down());
  free(data);
  return 0;
}
