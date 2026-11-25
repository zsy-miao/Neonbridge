#include <hsa/hsa.h>
#include <hsa/hsa_ext_amd.h>
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
int main(int argc, char **argv) {
  if (argc != 3) {
    fprintf(stderr, "usage: %s native.hsaco output.bin\n", argv[0]);
    return 2;
  }
  CHK(hsa_init());
  CHK(hsa_iterate_agents(each_agent, 0));
  if (!gpu.handle || !cpu.handle) {
    fprintf(stderr, "missing HSA agents\n");
    return 1;
  }
  char agent_name[64] = {0};
  CHK(hsa_agent_get_info(gpu, HSA_AGENT_INFO_NAME, agent_name));
  if (strcmp(agent_name, "gfx1036")) {
    fprintf(stderr, "expected gfx1036, got %s\n", agent_name);
    return 1;
  }
  CHK(hsa_agent_iterate_regions(cpu, each_region, 0));
  if (!region.handle) {
    fprintf(stderr, "agent/region absent\n");
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
  CHK(hsa_memory_allocate(region, 2097152, &out));
  CHK(hsa_amd_agents_allow_access(1, &gpu, NULL, out));
  CHK(hsa_memory_allocate(region, arg_size, &args));
  CHK(hsa_amd_agents_allow_access(1, &gpu, NULL, args));
  *(void **)args = out;
  *(uint32_t *)out = 0;
  if ((uintptr_t)out % 256 != 0)
    return 1;
  uint32_t *tri = (uint32_t *)out + 262144;
  memset(tri, 0, 64);
  float v[12] = {0, 0, 0, 1.127f, 0, 0, 0, 1.133f, 0, 1, 1, 0};
  memcpy(tri, v, sizeof(v));
  tri[15] = 9;
  hsa_signal_t signal;
  CHK(hsa_signal_create(1, 0, NULL, &signal));
  uint64_t index = hsa_queue_load_write_index_relaxed(queue);
  hsa_kernel_dispatch_packet_t *packet =
      (hsa_kernel_dispatch_packet_t *)queue->base_address +
      (index & (queue->size - 1));
  memset(packet, 0, sizeof(*packet));
  packet->setup = 1 << HSA_KERNEL_DISPATCH_PACKET_SETUP_DIMENSIONS;
  packet->workgroup_size_x = 64;
  packet->workgroup_size_y = 1;
  packet->workgroup_size_z = 1;
  packet->grid_size_x = 65536;
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
  if (result != 0)
    return 1;
  FILE *file = fopen(argv[2], "wb");
  if (!file)
    return 2;
  size_t written = fwrite(out, 16, 65536, file);
  if (fclose(file) != 0)
    return 1;
  printf("agent=%s signal=%ld records=%zu\n", agent_name, (long)result,
         written);
  if (result != 0 || written != 65536)
    return 1;
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
