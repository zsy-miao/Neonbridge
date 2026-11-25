#ifndef PROJECT_TUXBOX_XMEM_H
#define PROJECT_TUXBOX_XMEM_H

#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#define XMEM_ORDINARY_ATTRIBUTES 0x0480009eULL
#define XMEM_PLACEMENT_ATTRIBUTES 0x30804000ULL

__declspec(dllimport) void *WINAPI XMemAlloc(SIZE_T bytes, UINT64 attributes);
__declspec(dllimport) void WINAPI XMemFree(void *memory, UINT64 attributes);
__declspec(dllimport) void *WINAPI XMemAllocDefault(SIZE_T bytes, UINT64 attributes);
__declspec(dllimport) void WINAPI XMemFreeDefault(void *memory, UINT64 attributes);
__declspec(dllimport) HANDLE WINAPI XMemGetTitleHeap(void);
__declspec(dllimport) HANDLE WINAPI XMemCreateWin32Heap(UINT64 flags, void *base);
typedef struct XMEM_POOL_STATS {
    DWORD size, reserved;
    UINT64 extent, cached;
    DWORD commits, decommits;
} XMEM_POOL_STATS;
__declspec(dllimport) SIZE_T WINAPI XMemGetAllocationHysteresis(UINT kind, BOOL large);
__declspec(dllimport) BOOL WINAPI XMemSetAllocationHysteresis(UINT kind, BOOL large, SIZE_T bytes);
__declspec(dllimport) BOOL WINAPI XMemPreallocateFreeSpace(UINT kind, BOOL large);
__declspec(dllimport) BOOL WINAPI XMemGetAllocationStatistics(UINT kind, BOOL large, XMEM_POOL_STATS *out);
typedef void (WINAPI *XMEM_HEAP_ALLOCATED)(HANDLE, DWORD, SIZE_T, void *);
typedef void (WINAPI *XMEM_HEAP_REALLOCATED)(HANDLE, DWORD, void *, void *, SIZE_T);
typedef void (WINAPI *XMEM_HEAP_FREED)(HANDLE, DWORD, void *);
__declspec(dllimport) void WINAPI XMemSetWin32HeapTrackingHooks(XMEM_HEAP_ALLOCATED, XMEM_HEAP_REALLOCATED, XMEM_HEAP_FREED);
__declspec(dllimport) BOOL WINAPI XMemGetWin32HeapStatistics(HANDLE heap, SIZE_T stats[2]);
__declspec(dllimport) BOOL WINAPI XMemGetFreePageStatistics(UINT64 counts[7]);
__declspec(dllimport) BOOL WINAPI XMemIsToolsMemAvailable(void);
__declspec(dllimport) void WINAPI XMemDumpMemoryStats(char *buffer, SIZE_T capacity);
typedef void *(WINAPI *XMEM_ALLOC_HOOK)(SIZE_T bytes, UINT64 attributes);
typedef void (WINAPI *XMEM_FREE_HOOK)(void *memory, UINT64 attributes);
__declspec(dllimport) void WINAPI XMemSetAllocationHooks(XMEM_ALLOC_HOOK allocate, XMEM_FREE_HOOK release);
__declspec(dllimport) UINT WINAPI XMemGetWriteWatch(DWORD flags, const void *base, SIZE_T size,
                                                   void **addresses, ULONG_PTR *count, DWORD *granularity);
typedef struct XMEM_OPTION { UINT32 id; UINT32 reserved; UINT64 value; } XMEM_OPTION;
__declspec(dllimport) void WINAPI XMemSetOption(const XMEM_OPTION *option);
__declspec(dllimport) BOOL WINAPI XMemResumeWriteWatch(void);

// Selector used by the Frosty retail executable. The v2 result spans 120 bytes.
#define XMEM_TITLE_WORKING_SET_V2 0x20000001u
typedef struct XMEM_TITLE_STATISTICS_V2 {
    UINT64 physical_budget;
    UINT64 committed;
    BYTE reserved1[24];
    UINT64 pool_budget;
    UINT64 pool_committed;
    BYTE reserved2[32];
    UINT64 peak_committed;
    BYTE reserved3[24];
} XMEM_TITLE_STATISTICS_V2;
__declspec(dllimport) BOOL WINAPI XMemGetWorkingSetStatistics(UINT flags, XMEM_TITLE_STATISTICS_V2 *stats);

__declspec(dllimport) void *WINAPI XMemVirtualAlloc(void *address, SIZE_T bytes, DWORD allocation_type,
                                                  UINT64 profile, DWORD protection);

typedef struct XMEM_VIRTUAL_INFO {
    MEMORY_BASIC_INFORMATION memory;
    UINT64 console_flags;
    UINT64 console_tag;
} XMEM_VIRTUAL_INFO;
__declspec(dllimport) BOOL WINAPI XMemVirtualQuery(HANDLE process, const void *address,
                                                   XMEM_VIRTUAL_INFO *information);
__declspec(dllimport) UINT64 WINAPI XMemMakeTag(UINT32 fourcc);

__declspec(dllimport) BOOL WINAPI XMemAllocatePhysicalPages(UINT64 flags, SIZE_T *count, UINT64 *tokens);
__declspec(dllimport) void *WINAPI XMemMapPhysicalPages(void *base, SIZE_T count, const UINT64 *tokens);
__declspec(dllimport) BOOL WINAPI XMemMapPhysicalPagesScatter(void *const *addresses, SIZE_T count,
                                                              const UINT64 *tokens);
__declspec(dllimport) BOOL WINAPI XMemFreePhysicalPages(SIZE_T count, const UINT64 *tokens);

#endif
