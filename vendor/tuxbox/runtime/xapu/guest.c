#include <windows.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

__declspec(dllimport) HRESULT WINAPI XApuConnect(const DWORD *, void **, uintptr_t *);
__declspec(dllimport) HRESULT WINAPI XApuDisconnect(uintptr_t);
__declspec(dllimport) HRESULT WINAPI XApuEnqueueCommand(uintptr_t, const void *, DWORD *);
__declspec(dllimport) HRESULT WINAPI XApuDequeueResult(uintptr_t, void *);
__declspec(dllimport) DWORD WINAPI XApuGetResultCount(uintptr_t, DWORD *);

#define CHECK(condition) do { if (!(condition)) { printf("xapu failed at line %d\n", __LINE__); return 1; } } while (0)
#pragma pack(push, 1)
typedef struct {
    DWORD kind, stream;
    DWORD context[4];
    DWORD option;
    float rate;
    DWORD skip, frames, packet_bytes;
    void *packet;
    DWORD out_bytes;
    float *output;
} Process;
#pragma pack(pop)
_Static_assert(sizeof(Process) == 64, "Series process command layout");

// One 20 ms mono tone packet encoded with stock libopus 1.6.1.
// The reference floats below were decoded independently with libopus.
static const BYTE packet[] = {
    0x48,0x84,0x9d,0x47,0x4b,0x46,0xd4,0xdc,0xaf,0x06,0xa1,0x3f,0x08,0xc8,
    0x55,0x68,0x86,0xf6,0x97,0xc5,0xc9,0xe2,0xad,0x58,0xc2,0x9c,0xcd,0x00,
    0x99,0x8c,0x78,0x75,0x69,0xc6,0x94,0xd7,0x52,0xbe,0x0b,0xed,0x7b,0x0c,
    0xe0,0x47,0xba,0x53,0xb1,0x0d,0x26,0xa1,0x85,0x8c,0x10,
};

int main(int argc, char **argv) {
    DWORD config[6] = {1, 4, 2, 65536, 0, 9}, sequence = 0, received = 0;
    uintptr_t handle = 0;
    void *memory[2] = {0};
    DWORD control[14] = {1, 1, 0xabcdef12, 0, 0, 0, 1};
    BYTE result[80];
    CHECK(XApuConnect(config, memory, &handle) == S_OK && handle && memory[0] && memory[1]);
    CHECK(XApuDequeueResult(handle, result) == (HRESULT)0x8ac81001);
    CHECK(XApuEnqueueCommand(handle, control, &sequence) == S_OK && sequence == 1);
    CHECK(WaitForSingleObject(memory[1], 0) == WAIT_OBJECT_0);
    CHECK(XApuGetResultCount(handle, &received) == 1 && received == 0);
    memset(result, 0x5a, sizeof(result));
    CHECK(XApuDequeueResult(handle, result) == S_OK && memcmp(result, control, 24) == 0);
    CHECK(result[24] == 0x5a && result[48] == 0x5a);
    float *output = (float *)((BYTE *)memory[0] + 4096);
    output[648] = 123.0f;
    Process process = {3, 1, {0}, 0, 48000, 312, 960, sizeof(packet), memory[0], 648 * 4, output};
    memcpy(memory[0], packet, sizeof(packet));
    CHECK(XApuEnqueueCommand(handle, &process, &sequence) == S_OK && sequence == 2);
    CHECK(XApuDequeueResult(handle, result) == S_OK);
    DWORD bytes; memcpy(&bytes, result + 52, sizeof(bytes));
    CHECK(bytes == 648 * 4 && output[648] == 123.0f);
    CHECK(fabsf(output[0] - 0.14450073f) < 1e-5f && fabsf(output[88] + 0.38259888f) < 1e-5f);
    CHECK(fabsf(output[647] + 0.11242676f) < 1e-5f);
    control[0] = 5; control[3] = 4;
    CHECK(XApuEnqueueCommand(handle, control, NULL) == S_OK);
    control[0] = 2;
    CHECK(XApuEnqueueCommand(handle, control, NULL) == S_OK);
    CHECK(XApuEnqueueCommand(handle, control, NULL) == (HRESULT)0x8ac81000);
    CHECK(XApuDequeueResult(handle, result) == S_OK);
    CHECK(XApuDequeueResult(handle, result) == S_OK);
    CHECK(XApuEnqueueCommand(handle, &process, NULL) == (HRESULT)0x8ac81003);
    CHECK(XApuDisconnect(handle) == S_OK);
    const char *message = "xapu PASS Opus=960 pre-skip=312 output=648 queue=ordered";
    if (argc == 2) {
        FILE *receipt = NULL;
        CHECK(fopen_s(&receipt, argv[1], "w") == 0);
        CHECK(fputs(message, receipt) >= 0 && fclose(receipt) == 0);
    }
    puts(message);
    return 0;
}
