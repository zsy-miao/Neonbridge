#include "xmem.h"
#include <stdio.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <psapi.h>
#include <wchar.h>

static FILE *report;
static int failure(int line, const char *check)
{
    fprintf(stderr, "xmem-default failed at line %d: %s\n", line, check);
    if (report) fprintf(report, "xmem-default failed at line %d: %s\n", line, check);
    return 1;
}
#define REQUIRE(check) do { if (!(check)) return failure(__LINE__, #check); } while (0)

static int app_local_provider(void)
{
    wchar_t executable[MAX_PATH], dll[MAX_PATH];
    HMODULE loaded = GetModuleHandleW(L"xmem.dll");
    DWORD n = GetModuleFileNameW(NULL, executable, MAX_PATH);
    if (!loaded || !n || n >= MAX_PATH) return 0;
    n = GetModuleFileNameW(loaded, dll, MAX_PATH);
    if (!n || n >= MAX_PATH) return 0;
    wchar_t *exe_tail = wcsrchr(executable, L'\\');
    wchar_t *dll_tail = wcsrchr(dll, L'\\');
    if (!exe_tail || !dll_tail || _wcsicmp(dll_tail + 1, L"xmem.dll")) return 0;
    *exe_tail = *dll_tail = 0;
    return _wcsicmp(executable, dll) == 0;
}

static int working_set(void)
{
    _Static_assert(sizeof(XMEM_TITLE_STATISTICS_V2) == 120, "v2 statistics size");
    _Static_assert(offsetof(XMEM_TITLE_STATISTICS_V2, pool_budget) == 40, "single pool offset");
    _Static_assert(offsetof(XMEM_TITLE_STATISTICS_V2, peak_committed) == 88, "peak offset");
    struct {
        UINT64 guard_before;
        XMEM_TITLE_STATISTICS_V2 stats;
        UINT64 guard_after;
    } output;
    memset(&output, 0xa5, sizeof output);
    XMEM_TITLE_STATISTICS_V2 untouched = output.stats;
    REQUIRE(!XMemGetWorkingSetStatistics(0x10000001u, &output.stats));
    REQUIRE(GetLastError() == ERROR_NOT_SUPPORTED && !memcmp(&output.stats, &untouched, sizeof untouched));
    REQUIRE(!XMemGetWorkingSetStatistics(XMEM_TITLE_WORKING_SET_V2, NULL));
    REQUIRE(GetLastError() == ERROR_INVALID_PARAMETER);

    // As in the title, read real process-wide values, including memory acquired outside XMem.
    const SIZE_T size = 8u << 20;
    volatile BYTE *pages = VirtualAlloc(NULL, size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    REQUIRE(pages);
    for (SIZE_T i = 0; i < size; i += 4096) pages[i] = 0x35;
    REQUIRE(XMemGetWorkingSetStatistics(XMEM_TITLE_WORKING_SET_V2, &output.stats));
    MEMORYSTATUSEX host = {0};
    host.dwLength = sizeof host;
    PROCESS_MEMORY_COUNTERS_EX process = {0};
    process.cb = sizeof process;
    REQUIRE(GlobalMemoryStatusEx(&host));
    REQUIRE(K32GetProcessMemoryInfo(GetCurrentProcess(), (PROCESS_MEMORY_COUNTERS*)&process, sizeof process));
    REQUIRE(output.guard_before == 0xa5a5a5a5a5a5a5a5ULL && output.guard_after == output.guard_before);
    REQUIRE(output.stats.physical_budget <= host.ullTotalPhys && output.stats.physical_budget >= output.stats.committed);
    REQUIRE(output.stats.pool_budget == output.stats.physical_budget && output.stats.pool_committed == output.stats.committed);
    REQUIRE(output.stats.committed >= size && output.stats.peak_committed >= output.stats.committed);
    // The driver may charge a few pages between the two independent snapshots.
    UINT64 commit = process.PrivateUsage ? process.PrivateUsage : process.PagefileUsage;
    UINT64 expected = commit < host.ullTotalPhys ? commit : host.ullTotalPhys;
    UINT64 drift = expected > output.stats.committed ? expected - output.stats.committed : output.stats.committed - expected;
    REQUIRE(drift < 2u * 1024 * 1024);
    BYTE zero[32] = {0};
    REQUIRE(!memcmp(output.stats.reserved1, zero, sizeof output.stats.reserved1));
    REQUIRE(!memcmp(output.stats.reserved2, zero, sizeof output.stats.reserved2));
    REQUIRE(!memcmp(output.stats.reserved3, zero, sizeof output.stats.reserved3));
    REQUIRE(VirtualFree((void*)pages, 0, MEM_RELEASE));
    return 0;
}

static int virtual_reserve_commit(void)
{
    SYSTEM_INFO info;
    GetSystemInfo(&info);
    SIZE_T page = info.dwPageSize;
    MEMORY_BASIC_INFORMATION area;
    BYTE *base = XMemVirtualAlloc(NULL, 3 * page, MEM_RESERVE | MEM_TOP_DOWN, 1, PAGE_NOACCESS);
    REQUIRE(base && !((UINT_PTR)base % info.dwAllocationGranularity));
    REQUIRE(VirtualQuery(base, &area, sizeof area) == sizeof area);
    REQUIRE(area.State == MEM_RESERVE && area.AllocationBase == base && area.RegionSize >= 3 * page);
    volatile BYTE *middle = XMemVirtualAlloc(base + page, page, MEM_COMMIT, 1, PAGE_READWRITE);
    REQUIRE(middle == base + page);
    REQUIRE(VirtualQuery(base + page, &area, sizeof area) == sizeof area);
    REQUIRE(area.State == MEM_COMMIT && area.Protect == PAGE_READWRITE && area.AllocationBase == base);
    middle[0] = 0x73;
    middle[page - 1] = 0xa7;
    REQUIRE(middle[0] == 0x73 && middle[page - 1] == 0xa7);
    REQUIRE(VirtualQuery(base, &area, sizeof area) == sizeof area && area.State == MEM_RESERVE);
    REQUIRE(VirtualQuery(base + 2 * page, &area, sizeof area) == sizeof area && area.State == MEM_RESERVE);
    REQUIRE(VirtualFree(base, 0, MEM_RELEASE));
    REQUIRE(VirtualQuery(base, &area, sizeof area) == sizeof area && area.State == MEM_FREE);

    BYTE *direct = XMemVirtualAlloc(NULL, page, MEM_COMMIT, 1, PAGE_READWRITE);
    REQUIRE(direct && VirtualQuery(direct, &area, sizeof area) == sizeof area);
    REQUIRE(area.State == MEM_COMMIT && area.AllocationBase == direct && area.Protect == PAGE_READWRITE);
    REQUIRE(VirtualFree(direct, 0, MEM_RELEASE));
    REQUIRE(!XMemVirtualAlloc(NULL, page, MEM_RESERVE, 2, PAGE_NOACCESS) && GetLastError() == ERROR_NOT_SUPPORTED);
    const DWORD protections[] = {0x2404, 0x10404};
    const SIZE_T page64k = 65536;
    for (size_t i = 0; i < sizeof protections / sizeof protections[0]; ++i) {
        BYTE *aperture = XMemVirtualAlloc(NULL, 2 * page64k, MEM_RESERVE | 0x20400000u, 3, protections[i]);
        REQUIRE(aperture && VirtualQuery(aperture, &area, sizeof area) == sizeof area);
        REQUIRE(area.State == MEM_RESERVE && area.RegionSize >= 2 * page64k);
        BYTE *committed = XMemVirtualAlloc(aperture + page64k, page64k, MEM_COMMIT | 0x20400000u, 3, protections[i]);
        REQUIRE(committed == aperture + page64k);
        REQUIRE(VirtualQuery(committed, &area, sizeof area) == sizeof area && area.Protect == PAGE_READWRITE);
        committed[0] = 0x69;
        committed[page64k - 1] = 0x17;
        REQUIRE(committed[0] == 0x69 && committed[page64k - 1] == 0x17);
        REQUIRE(VirtualQuery(aperture, &area, sizeof area) == sizeof area && area.State == MEM_RESERVE);
        REQUIRE(VirtualFree(aperture, 0, MEM_RELEASE));
    }
    // The console interprets this flag pair as a larger page mode.
    REQUIRE(!XMemVirtualAlloc(NULL, page, MEM_COMMIT | 0x20400000u, 1, PAGE_READWRITE) &&
            GetLastError() == ERROR_NOT_SUPPORTED);
    return 0;
}

static int virtual_query(void)
{
    _Static_assert(sizeof(XMEM_VIRTUAL_INFO) == 64, "Xbox virtual query result size");
    _Static_assert(offsetof(XMEM_VIRTUAL_INFO, console_flags) == 48, "Xbox metadata offset");
    struct { UINT64 before; XMEM_VIRTUAL_INFO info; UINT64 after; } out;
    SYSTEM_INFO system;
    GetSystemInfo(&system);
    BYTE *base = VirtualAlloc(NULL, 2 * system.dwPageSize, MEM_RESERVE, PAGE_NOACCESS);
    REQUIRE(base);
    HMODULE module = GetModuleHandleW(L"xmem.dll");
    REQUIRE(GetProcAddress(module, "XMemVirtualQuery") == GetProcAddress(module, MAKEINTRESOURCEA(36)));
    REQUIRE(VirtualAlloc(base + system.dwPageSize, system.dwPageSize, MEM_COMMIT, PAGE_READONLY));
    BYTE sentinel[sizeof out]; memset(sentinel, 0xa5, sizeof sentinel);
    for (int i = 0; i < 2; ++i) {
        memset(&out, 0xa5, sizeof out);
        REQUIRE(!XMemVirtualQuery(GetCurrentProcess(), base + i * system.dwPageSize, &out.info));
        REQUIRE(GetLastError() == ERROR_NOT_SUPPORTED && !memcmp(&out, sentinel, sizeof out));
    }
    REQUIRE(!XMemVirtualQuery(GetCurrentProcess(), base, NULL));
    REQUIRE(GetLastError() == ERROR_INVALID_PARAMETER);
    REQUIRE(VirtualFree(base, 0, MEM_RELEASE));
    memset(&out, 0xa5, sizeof out);
    MEMORY_BASIC_INFORMATION released = {0};
    REQUIRE(VirtualQuery(base, &released, sizeof released) == sizeof released);
    REQUIRE(XMemVirtualQuery(GetCurrentProcess(), base, &out.info));
    REQUIRE(!memcmp(&out.info.memory, &released, sizeof released) && released.State == MEM_FREE);
    REQUIRE(out.info.console_flags == 1 && !out.info.console_tag);
    REQUIRE(out.before == 0xa5a5a5a5a5a5a5a5ull && out.after == out.before);
    base = XMemVirtualAlloc(NULL, 2 * system.dwPageSize, MEM_RESERVE, 1, PAGE_READONLY);
    REQUIRE(base && XMemVirtualAlloc(base + system.dwPageSize, system.dwPageSize, MEM_COMMIT, 1, PAGE_READONLY));
    const void *addresses[] = {base, base + system.dwPageSize, module};
    for (unsigned i = 0; i < 3; ++i) {
        MEMORY_BASIC_INFORMATION expected = {0};
        REQUIRE(VirtualQuery(addresses[i], &expected, sizeof expected) == sizeof expected);
        memset(&out, 0xa5, sizeof out);
        REQUIRE(XMemVirtualQuery(GetCurrentProcess(), addresses[i], &out.info));
        REQUIRE(!memcmp(&out.info.memory, &expected, sizeof expected));
        REQUIRE(out.info.console_flags == 1 && !out.info.console_tag);
        REQUIRE(out.before == 0xa5a5a5a5a5a5a5a5ull && out.after == out.before);
    }
    REQUIRE(VirtualFree(base, 0, MEM_RELEASE));
    for (DWORD watch = 0; watch < 2; ++watch) {
        DWORD kind = MEM_RESERVE | MEM_COMMIT | (watch ? MEM_WRITE_WATCH : 0);
        base = XMemVirtualAlloc(NULL, 65536, kind, 1, PAGE_READWRITE);
        REQUIRE(base);
        base[0] = 0x77;
        REQUIRE(XMemVirtualQuery(GetCurrentProcess(), base, &out.info));
        REQUIRE(out.info.memory.State == (MEM_COMMIT | (watch ? (1u << 21) : 0)));
        REQUIRE(out.info.memory.Protect == PAGE_READWRITE && out.info.console_flags == 1);
        if (watch) {
            void* dirty = NULL; ULONG_PTR count = 1; DWORD granularity = 0;
            REQUIRE(!GetWriteWatch(0,base,4096,&dirty,&count,&granularity) && count == 1 && dirty == base);
        }
        REQUIRE(VirtualFree(base,0,MEM_RELEASE));
    }
    void *placement = XMemAllocDefault(65536, XMEM_PLACEMENT_ATTRIBUTES);
    REQUIRE(placement && !XMemVirtualQuery(GetCurrentProcess(),placement,&out.info));
    REQUIRE(GetLastError() == ERROR_NOT_SUPPORTED);
    XMemFreeDefault(placement,XMEM_PLACEMENT_ATTRIBUTES);
    return 0;
}

static int virtual_cpu_profiles(void)
{
    SYSTEM_INFO system; GetSystemInfo(&system);
    SIZE_T page = system.dwPageSize;
    const DWORD protections[] = { PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE, PAGE_EXECUTE,
                                  PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE };
    MEMORY_BASIC_INFORMATION area;
    for (unsigned i = 0; i < sizeof protections / sizeof protections[0]; ++i) {
        DWORD protection = protections[i];
        BYTE *base = XMemVirtualAlloc(NULL, 2 * page, MEM_RESERVE | MEM_COMMIT, 1, protection);
        REQUIRE(base && !((UINT_PTR)base % system.dwAllocationGranularity));
        REQUIRE(VirtualQuery(base, &area, sizeof area) == sizeof area);
        REQUIRE(area.AllocationBase == base && area.State == MEM_COMMIT && area.Protect == protection);
        if (protection == PAGE_READWRITE || protection == PAGE_EXECUTE_READWRITE) {
            base[0] = 0x39; base[2 * page - 1] = 0xb4;
            REQUIRE(base[0] == 0x39 && base[2 * page - 1] == 0xb4);
        }
        REQUIRE(VirtualFree(base, 0, MEM_RELEASE));
        base = XMemVirtualAlloc(NULL, 2 * page, MEM_RESERVE | MEM_TOP_DOWN, 1, protection);
        REQUIRE(base && VirtualQuery(base, &area, sizeof area) == sizeof area && area.State == MEM_RESERVE);
        REQUIRE(XMemVirtualAlloc(base, 2 * page, MEM_COMMIT, 1, protection) == base);
        REQUIRE(VirtualQuery(base, &area, sizeof area) == sizeof area &&
                area.State == MEM_COMMIT && area.Protect == protection);
        REQUIRE(VirtualFree(base, 0, MEM_RELEASE));
    }
    for (unsigned i = 1; i < sizeof protections / sizeof protections[0]; ++i) {
        DWORD guarded = protections[i] | PAGE_GUARD;
        BYTE *base = XMemVirtualAlloc(NULL, page, MEM_RESERVE | MEM_COMMIT, 1, guarded);
        REQUIRE(base && VirtualQuery(base, &area, sizeof area) == sizeof area && area.Protect == guarded);
        BYTE sample = 0xff; SIZE_T transferred = 0;
        REQUIRE(!ReadProcessMemory(GetCurrentProcess(), base, &sample, 1, &transferred));
        REQUIRE(VirtualQuery(base, &area, sizeof area) == sizeof area && area.Protect == protections[i]);
        // Execute-only pages are not guaranteed to be readable on every host.
        if (protections[i] != PAGE_EXECUTE) {
            REQUIRE(ReadProcessMemory(GetCurrentProcess(), base, &sample, 1, &transferred));
            REQUIRE(transferred == 1 && sample == 0);
        }
        REQUIRE(VirtualFree(base, 0, MEM_RELEASE));
    }
    REQUIRE(!XMemVirtualAlloc(NULL, page, MEM_RESERVE | MEM_COMMIT, 1, PAGE_NOACCESS | PAGE_GUARD));
    REQUIRE(GetLastError() == ERROR_NOT_SUPPORTED);
    return 0;
}

static int write_watch(void)
{
    HMODULE module = GetModuleHandleW(L"xmem.dll");
    REQUIRE(GetProcAddress(module, "XMemGetWriteWatch") == GetProcAddress(module, MAKEINTRESOURCEA(18)));
    SYSTEM_INFO info;
    GetSystemInfo(&info);
    SIZE_T page = info.dwPageSize;
    volatile BYTE *base = XMemVirtualAlloc(NULL, 4 * page,
        MEM_COMMIT | MEM_RESERVE | MEM_WRITE_WATCH | MEM_TOP_DOWN, 1, PAGE_READWRITE);
    REQUIRE(base);
    base[7] = 1;
    base[2 * page + 7] = 2;
    void *expected[4] = {0}; ULONG_PTR expected_count = 4; DWORD expected_granularity = 0;
    REQUIRE(!GetWriteWatch(0, (void *)base, 4 * page, expected, &expected_count, &expected_granularity));
    struct { UINT64 before; void *pages[4]; UINT64 after; } output = {0};
    output.before = output.after = 0xa5a5a5a5a5a5a5a5ULL;
    ULONG_PTR count = 4; DWORD granularity = 0;
    REQUIRE(!XMemGetWriteWatch(0, (void *)base, 4 * page, output.pages, &count, &granularity));
    REQUIRE(count == expected_count && count == 2 && granularity == expected_granularity);
    REQUIRE(!memcmp(output.pages, expected, count * sizeof(void *)));
    REQUIRE(output.before == 0xa5a5a5a5a5a5a5a5ULL && output.after == output.before);
    count = 4;
    REQUIRE(!XMemGetWriteWatch(WRITE_WATCH_FLAG_RESET, (void *)base, 4 * page,
                               output.pages, &count, &granularity) && count == 2);
    base[page + 3] = 3;
    count = 4;
    REQUIRE(!XMemGetWriteWatch(0, (void *)base, 4 * page, output.pages, &count, &granularity));
    REQUIRE(count == 1 && output.pages[0] == (void *)(base + page));
    REQUIRE(output.before == 0xa5a5a5a5a5a5a5a5ULL && output.after == output.before);
    expected_count = count = 4;
    DWORD host_error = GetWriteWatch(0x80000000u, (void *)base, 4 * page,
                                     expected, &expected_count, &expected_granularity);
    DWORD guest_error = XMemGetWriteWatch(0x80000000u, (void *)base, 4 * page,
                                          output.pages, &count, &granularity);
    REQUIRE(host_error != 0 && guest_error == host_error && count == expected_count);
    REQUIRE(GetProcAddress(module, "XMemSetOption") == GetProcAddress(module, MAKEINTRESOURCEA(30)));
    REQUIRE(GetProcAddress(module, "XMemResumeWriteWatch") == GetProcAddress(module, MAKEINTRESOURCEA(27)));
    REQUIRE(XMemResumeWriteWatch());
    XMEM_OPTION setting = {0x10000, 0, 1};
    XMemSetOption(&setting);
    memset(output.pages, 0xa5, sizeof output.pages);
    void *untouched[4]; memset(untouched, 0xa5, sizeof untouched);
    count = 4; granularity = 0xa5a5a5a5u;
    REQUIRE(XMemGetWriteWatch(0, (void *)base, 4 * page, output.pages, &count, &granularity) == 1003);
    REQUIRE(count == 4 && granularity == 0xa5a5a5a5u && !memcmp(output.pages, untouched, sizeof untouched));
    REQUIRE(!XMemResumeWriteWatch());
    setting.value = 0x100000000ULL; // the console uses only the low 32 bits
    XMemSetOption(&setting);
    REQUIRE(XMemGetWriteWatch(0, (void *)base, 4 * page, output.pages, &count, &granularity) == 1003);
    REQUIRE(XMemResumeWriteWatch());
    REQUIRE(!XMemGetWriteWatch(0, (void *)base, 4 * page, output.pages, &count, &granularity));
    REQUIRE(count == 1 && output.pages[0] == (void *)(base + page));
    // New direct CPU allocations capture the active tagging policy.
    for (unsigned enabled = 0; enabled < 2; ++enabled) {
        setting.id = 2; setting.value = enabled;
        XMemSetOption(&setting);
        BYTE *direct = XMemAllocDefault(0x21003, 0x05000000);
        REQUIRE(direct && !((UINT_PTR)direct & 31));
        direct[0x21002] = 42;
        XMEM_VIRTUAL_INFO query = {0};
        REQUIRE(XMemVirtualQuery(GetCurrentProcess(), direct + 4096, &query));
        REQUIRE(query.memory.RegionSize == (enabled ? 0x22000 : 0x21000));
        REQUIRE(query.console_tag == (enabled ? ((UINT64)0x41566d58 << 32) | (XMemMakeTag(0x586d5641) >> 40) : 0));
        XMemFreeDefault(direct, 0x05000000);
    }
    setting.id = 2; setting.value = 0; XMemSetOption(&setting);
    for (unsigned enabled = 0; enabled < 2; ++enabled) {
        setting.id = 0x10001; setting.value = enabled; XMemSetOption(&setting);
        volatile BYTE *aperture = XMemVirtualAlloc(NULL, 0x10000, 0x20402000, 3, 0x2404);
        REQUIRE(aperture && XMemVirtualAlloc((void *)aperture, 0x10000, 0x20401000, 3, 0x2404) == aperture);
        aperture[page] = 7;
        count = 4; granularity = 0;
        DWORD status = XMemGetWriteWatch(0, (void *)aperture, 0x10000, output.pages, &count, &granularity);
        REQUIRE(status != 0);  // desktop retail has no tools partition
        REQUIRE(VirtualFree((void *)aperture, 0, MEM_RELEASE));
    }
    setting.id = 0xfeed;
    SetLastError(0xbee7);
    XMemSetOption(&setting);
    REQUIRE(GetLastError() == 0xbee7);
    XMemSetOption(NULL);
    REQUIRE(GetLastError() == ERROR_INVALID_PARAMETER);
    REQUIRE(VirtualFree((void *)base, 0, MEM_RELEASE));
    base = XMemVirtualAlloc(NULL, 4 * page, MEM_RESERVE | MEM_WRITE_WATCH, 1, PAGE_NOACCESS);
    REQUIRE(base);
    REQUIRE(XMemVirtualAlloc((void *)base, 4 * page, MEM_COMMIT, 1, PAGE_READWRITE) == base);
    base[3 * page] = 1;
    count = 4; granularity = 0;
    REQUIRE(!XMemGetWriteWatch(0, (void *)base, 4 * page, output.pages, &count, &granularity));
    REQUIRE(count == 1 && output.pages[0] == (void *)(base + 3 * page));
    REQUIRE(VirtualFree((void *)base, 0, MEM_RELEASE));
    return 0;
}

static int allocation_tags(void)
{
    HMODULE module = GetModuleHandleW(L"xmem.dll");
    REQUIRE(GetProcAddress(module, "XMemMakeTag") == GetProcAddress(module, MAKEINTRESOURCEA(21)));
    REQUIRE(XMemMakeTag(0) == 0);
    REQUIRE(XMemMakeTag(0x41414141u) == 0x69a69a0000000000ULL);
    UINT64 hash = 14695981039346656037ULL;
    unsigned count = 0;
    for (unsigned shift = 0; shift < 32; shift += 8) {
        for (unsigned c = 0; c < 256; ++c) {
            UINT32 value = (0x41414141u & ~(0xffu << shift)) | (c << shift);
            UINT64 tag = XMemMakeTag(value);
            for (int i = 0; i < 8; ++i) { hash ^= (tag >> (8 * i)) & 255; hash *= 1099511628211ULL; }
            ++count;
        }
    }
    UINT32 seed = 0x6d2b79f5;
    for (unsigned i = 0; i < 4096; ++i) {
        seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5;
        UINT64 tag = XMemMakeTag(seed);
        for (int j = 0; j < 8; ++j) { hash ^= (tag >> (8 * j)) & 255; hash *= 1099511628211ULL; }
        ++count;
    }
    REQUIRE(count == 5120 && hash == 0xd4bf30c57509b176ULL);
    return 0;
}

static unsigned hook_alloc_calls, hook_free_calls;
static SIZE_T hook_bytes;
static UINT64 hook_alloc_attributes, hook_free_attributes;
static void *hook_freed;
static BYTE hook_storage[16];

static void *WINAPI guest_alloc_hook(SIZE_T bytes, UINT64 attributes)
{
    ++hook_alloc_calls;
    hook_bytes = bytes;
    hook_alloc_attributes = attributes;
    return hook_storage;
}

static void WINAPI guest_free_hook(void *memory, UINT64 attributes)
{
    ++hook_free_calls;
    hook_freed = memory;
    hook_free_attributes = attributes;
}

static void *WINAPI reentrant_alloc_hook(SIZE_T bytes, UINT64 attributes)
{
    (void)bytes; (void)attributes;
    XMemSetAllocationHooks(NULL, NULL);
    return hook_storage;
}

static int allocation_hooks(void)
{
    HMODULE module = GetModuleHandleW(L"xmem.dll");
    REQUIRE(GetProcAddress(module, "XMemSetAllocationHooks") == GetProcAddress(module, MAKEINTRESOURCEA(28)));
    REQUIRE(GetProcAddress(module, "XMemAlloc") != GetProcAddress(module, "XMemAllocDefault"));
    REQUIRE(GetProcAddress(module, "XMemFree") != GetProcAddress(module, "XMemFreeDefault"));
    XMemSetAllocationHooks(guest_alloc_hook, guest_free_hook);
    REQUIRE(XMemAlloc(0, 0x1234567887654321ULL) == hook_storage);
    REQUIRE(hook_alloc_calls == 1 && hook_bytes == 0 && hook_alloc_attributes == 0x1234567887654321ULL);
    XMemFree(NULL, 0x1020304050607080ULL);
    REQUIRE(hook_free_calls == 1 && hook_freed == NULL && hook_free_attributes == 0x1020304050607080ULL);
    REQUIRE(XMemAlloc(17, 0x9988776655443322ULL) == hook_storage);
    XMemFree(hook_storage, 0x8877665544332211ULL);
    REQUIRE(hook_alloc_calls == 2 && hook_bytes == 17 && hook_alloc_attributes == 0x9988776655443322ULL);
    REQUIRE(hook_free_calls == 2 && hook_freed == hook_storage && hook_free_attributes == 0x8877665544332211ULL);
    // A null allocator resets the pair even when a release callback is supplied.
    XMemSetAllocationHooks(NULL, guest_free_hook);
    void *ordinary = XMemAlloc(32, XMEM_ORDINARY_ATTRIBUTES);
    REQUIRE(ordinary && HeapValidate(GetProcessHeap(), 0, ordinary));
    XMemFree(ordinary, XMEM_ORDINARY_ATTRIBUTES);
    REQUIRE(hook_alloc_calls == 2 && hook_free_calls == 2);
    XMemSetAllocationHooks(reentrant_alloc_hook, guest_free_hook);
    REQUIRE(XMemAlloc(1, 0) == hook_storage);
    ordinary = XMemAlloc(32, XMEM_ORDINARY_ATTRIBUTES);
    REQUIRE(ordinary && HeapValidate(GetProcessHeap(), 0, ordinary));
    XMemFree(ordinary, XMEM_ORDINARY_ATTRIBUTES);
    XMemSetAllocationHooks(NULL, NULL);
    return 0;
}

typedef void *(WINAPI *Allocation)(SIZE_T, UINT64);
typedef void (WINAPI *Release)(void *, UINT64);

static int heap_profiles(void)
{
    const UINT64 metadata[] = {0, 1, 0x9e, 0x3fff, 0x20000, 0x800000, 0x823fff, 0x200000000ULL};
    for (unsigned type = 0; type < 5; ++type) {
        for (unsigned i = 0; i < sizeof metadata / sizeof metadata[0]; ++i) {
            UINT64 attributes = ((UINT64)type << 24) | metadata[i];
            BYTE *memory = XMemAllocDefault(17, attributes);
            REQUIRE(memory && !((UINT_PTR)memory & 15) && HeapValidate(GetProcessHeap(), 0, memory));
            memset(memory, (int)(i + type), 17);
            REQUIRE(memory[0] == (BYTE)(i + type) && memory[16] == memory[0]);
            XMemFreeDefault(memory, attributes);
            REQUIRE(!HeapValidate(GetProcessHeap(), 0, memory));
            memory = XMemAlloc(17, attributes);
            REQUIRE(memory && HeapValidate(GetProcessHeap(), 0, memory));
            XMemFree(memory, attributes);
        }
    }
    // The supplied default routine calls HeapFree even for a null heap pointer.
    SetLastError(0xabcdu);
    (void)HeapFree(GetProcessHeap(), 0, NULL);
    DWORD expected = GetLastError();
    SetLastError(0xabcdu);
    XMemFreeDefault(NULL, 0);
    REQUIRE(GetLastError() == expected);
    SetLastError(0xabcdu);
    XMemFree(NULL, 0);
    REQUIRE(GetLastError() == expected);
    return 0;
}

static int allocator_pools(void)
{
    const SIZE_T block = 4u << 20;
    XMEM_POOL_STATS stats = {0};
    stats.size = sizeof stats; stats.reserved = 0xa5a5a5a5;
    REQUIRE(XMemGetAllocationHysteresis(16, 0) == (SIZE_T)-1 && GetLastError() == ERROR_INVALID_PARAMETER);
    for (unsigned i = 0; i < 16; ++i) {
        SIZE_T initial = (i >= 1 && i <= 6) || (i >= 10 && i <= 12) ? 16 * block : 2 * block;
        REQUIRE(XMemGetAllocationHysteresis(i, 0) == initial && XMemGetAllocationHysteresis(i, 2) == initial);
    }
    REQUIRE(XMemSetAllocationHysteresis(0, 0, block + 1) && XMemGetAllocationHysteresis(0, 0) == 2 * block);
    REQUIRE(XMemPreallocateFreeSpace(0, 0) && XMemGetAllocationStatistics(0, 0, &stats));
    REQUIRE(stats.cached == 2 * block && stats.extent == stats.cached && stats.commits == 2);
    BYTE *first = XMemAllocDefault(240, 0x08000000), *second = XMemAllocDefault(258, 0x09000000);
    REQUIRE(first && second && !((UINT_PTR)first & 255) && !((UINT_PTR)second & 511));
    memset(first, 0x1b, 240); memset(second, 0xce, 258);
    REQUIRE(first[239] == 0x1b && second[257] == 0xce && XMemGetAllocationStatistics(0, 0, &stats));
    REQUIRE(stats.cached == block && stats.reserved == 0xa5a5a5a5 && stats.commits == 2);
    REQUIRE(XMemSetAllocationHysteresis(0, 0, 0));
    REQUIRE(XMemGetAllocationStatistics(0, 0, &stats) && stats.cached == 0 && stats.decommits == 1);
    XMemFreeDefault(first, 0x08000000);
    REQUIRE(second[257] == 0xce); XMemFreeDefault(second, 0x09000000);
    REQUIRE(XMemGetAllocationStatistics(0, 0, &stats) && stats.decommits == 2 && stats.extent == 2 * block);
    REQUIRE(XMemGetAllocationStatistics(0, 1, &stats) && stats.commits == 0 && stats.decommits == 0);
    REQUIRE(XMemGetAllocationHysteresis(0, 1) == 2 * block);
    stats.size = 31; REQUIRE(!XMemGetAllocationStatistics(0, 0, &stats) && GetLastError() == ERROR_BAD_LENGTH);
    return 0;
}

static void *tracked_allocation, *tracked_release;
static void WINAPI on_heap_alloc(HANDLE heap, DWORD flags, SIZE_T bytes, void *pointer)
{
    if (heap == XMemGetTitleHeap() && !flags && bytes == 73) tracked_allocation = pointer;
}
static void WINAPI on_heap_free(HANDLE heap, DWORD flags, void *pointer)
{
    if (heap == XMemGetTitleHeap() && !flags) tracked_release = pointer;
}
static int heap_diagnostics(void)
{
    SetLastError(0); REQUIRE(!XMemIsToolsMemAvailable() && GetLastError() == ERROR_OBJECT_NOT_FOUND);
    UINT64 page_counts[9] = {0};
    page_counts[0] = page_counts[8] = 0x1122334455667788ULL;
    REQUIRE(XMemGetFreePageStatistics(&page_counts[1]));
    REQUIRE(page_counts[0] == page_counts[8] && page_counts[8] == 0x1122334455667788ULL);
    REQUIRE(!page_counts[1] && page_counts[2] && !page_counts[3] && !page_counts[4]);
    REQUIRE(!memcmp(&page_counts[2], &page_counts[5], 3 * sizeof(UINT64)));
    MEMORYSTATUSEX host = {0};
    host.dwLength = sizeof host;
    REQUIRE(GlobalMemoryStatusEx(&host));
    UINT64 observed = page_counts[2] << 12;
    UINT64 available = min(host.ullAvailPhys, host.ullAvailPageFile);
    UINT64 drift = observed > available ? observed - available : available - observed;
    REQUIRE(drift < 64ull * 1024 * 1024);
    if (available >= (2ull << 30)) {
        // Empty CPU tokens claim physical budget without touching 512 MiB of RAM.
        SIZE_T n = 8192;
        UINT64 *tokens = calloc(n, sizeof *tokens);
        REQUIRE(tokens && XMemAllocatePhysicalPages(0x20400000, &n, tokens) && n == 8192);
        UINT64 charged[7];
        REQUIRE(XMemGetFreePageStatistics(charged));
        REQUIRE(charged[1] + 65536 < page_counts[2]);
        REQUIRE(XMemFreePhysicalPages(n, tokens));
        free(tokens);
    }
    HMODULE module = GetModuleHandleW(L"xmem.dll");
    const UINT64 *volatile *history = (const UINT64 *volatile *)GetProcAddress(module, "XMemHistoryData");
    REQUIRE(history && (FARPROC)history == GetProcAddress(module, MAKEINTRESOURCEA(19)) && *history);
    MEMORY_BASIC_INFORMATION view;
    REQUIRE(VirtualQuery(*history, &view, sizeof view) == sizeof view && view.Protect == PAGE_READONLY);
    for (int retry = 0; retry < 30 && !((const volatile UINT64 *)*history)[15]; ++retry) Sleep(100);
    REQUIRE((*history)[0] >= (*history)[1] && (*history)[1] && (*history)[15] && (*history)[16]);
    XMemSetWin32HeapTrackingHooks(on_heap_alloc, NULL, on_heap_free);
    void *p = XMemAllocDefault(73, 0);
    REQUIRE(p && tracked_allocation == p);
    XMemFreeDefault(p, 0);
    REQUIRE(tracked_release == p);
    XMemSetWin32HeapTrackingHooks(NULL, NULL, NULL);
    tracked_allocation = tracked_release = NULL;
    p = XMemAllocDefault(73, 0); REQUIRE(p); XMemFreeDefault(p, 0);
    REQUIRE(!tracked_allocation && !tracked_release);

    HANDLE heap = XMemCreateWin32Heap(4, NULL);
    SIZE_T before[2], after[2];
    REQUIRE(heap && XMemGetWin32HeapStatistics(heap, before) && before[0] > 0);
    SIZE_T bytes = 8u << 20;
    void *big = HeapAlloc(heap, 0, bytes);
    REQUIRE(big && XMemGetWin32HeapStatistics(heap, after));
    REQUIRE(after[0] + after[1] >= before[0] + before[1] + bytes);
    REQUIRE(HeapFree(heap, 0, big) && HeapDestroy(heap));
    char dump[4096], small[] = "123456789";
    XMemDumpMemoryStats(dump, sizeof dump);
    REQUIRE(strstr(dump, "SYSTEM PERFORMANCE INFO") && strstr(dump, "CommittedPages: "));
    XMemDumpMemoryStats(small + 1, 5);
    REQUIRE(!memcmp(small, "1XXXX\0""789", sizeof small));
    return 0;
}

static int title_and_private_heaps(void)
{
    HMODULE module = GetModuleHandleW(L"xmem.dll");
    REQUIRE(GetProcAddress(module, "XMemGetTitleHeap") == GetProcAddress(module, MAKEINTRESOURCEA(14)));
    REQUIRE(GetProcAddress(module, "XMemCreateWin32Heap") == GetProcAddress(module, MAKEINTRESOURCEA(4)));
    HANDLE title = XMemGetTitleHeap();
    REQUIRE(title && title == GetProcessHeap());
    BYTE *a = HeapAlloc(title, 0, 37);
    REQUIRE(a && HeapValidate(title, 0, a));
    memset(a, 0x56, 37);
    XMemFreeDefault(a, 0);
    BYTE *b = XMemAllocDefault(41, 0);
    REQUIRE(b && HeapValidate(title, 0, b));
    REQUIRE(HeapFree(title, 0, b));

    const UINT64 flags[] = {0, 4, 0x8000, 0x8004};
    for (unsigned i = 0; i < sizeof flags / sizeof flags[0]; ++i) {
        HANDLE heap = XMemCreateWin32Heap(flags[i], (void *)0x10000);
        REQUIRE(heap && heap != title);
        BYTE *block = HeapAlloc(heap, 0, 113);
        REQUIRE(block && HeapValidate(heap, 0, block));
        memset(block, (int)i, 113);
        REQUIRE(block[0] == i && block[112] == i);
        REQUIRE(HeapFree(heap, 0, block));
        REQUIRE(HeapDestroy(heap));
    }
    REQUIRE(!XMemCreateWin32Heap(2, NULL) && GetLastError() == ERROR_INVALID_PARAMETER);
    REQUIRE(!XMemCreateWin32Heap(2, (void *)0x10000) && GetLastError() == ERROR_NOT_SUPPORTED);
    REQUIRE(!XMemCreateWin32Heap(8, NULL) && GetLastError() == ERROR_INVALID_PARAMETER);
    return 0;
}

static int allocation_pair(Allocation allocate, Release release)
{
    const UINT64 heap_attributes[] = {0x04800092, 0x04800040, 0x04800078, 0x04820000};
    for (size_t i = 0; i < sizeof heap_attributes / sizeof heap_attributes[0]; ++i) {
        BYTE *memory = allocate(257, heap_attributes[i]);
        REQUIRE(memory && ((UINT_PTR)memory & 15) == 0);
        REQUIRE(HeapValidate(GetProcessHeap(), 0, memory));
        memset(memory, 0xa5, 257);
        REQUIRE(memory[0] == 0xa5 && memory[256] == 0xa5);
        release(memory, heap_attributes[i]);
        REQUIRE(!HeapValidate(GetProcessHeap(), 0, memory));
    }
    enum { small_size = 12, placed_size = 65536 };
    volatile BYTE *small = allocate(small_size, XMEM_ORDINARY_ATTRIBUTES);
    volatile BYTE *placed = allocate(placed_size, XMEM_PLACEMENT_ATTRIBUTES);
    REQUIRE(small && placed);
    REQUIRE(((UINT_PTR)small & 15) == 0 && ((UINT_PTR)placed & 65535) == 0);
    REQUIRE(HeapValidate(GetProcessHeap(), 0, (void *)small));
    MEMORY_BASIC_INFORMATION area;
    REQUIRE(VirtualQuery((void *)placed, &area, sizeof area) == sizeof area);
    REQUIRE(area.State == MEM_COMMIT && area.AllocationBase == placed && area.RegionSize >= placed_size);

    for (unsigned i = 0; i < small_size; ++i) small[i] = (BYTE)(0xb1u + i);
    for (unsigned i = 0; i < placed_size; ++i) placed[i] = (BYTE)(i ^ (i >> 8));
    for (unsigned i = 0; i < small_size; ++i) REQUIRE(small[i] == (BYTE)(0xb1u + i));
    for (unsigned i = 0; i < placed_size; ++i) REQUIRE(placed[i] == (BYTE)(i ^ (i >> 8)));

    release((void *)small, XMEM_ORDINARY_ATTRIBUTES);
    REQUIRE(!HeapValidate(GetProcessHeap(), 0, (void *)small));
    release((void *)placed, XMEM_PLACEMENT_ATTRIBUTES);
    REQUIRE(VirtualQuery((void *)placed, &area, sizeof area) == sizeof area && area.State == MEM_FREE);
    REQUIRE(!allocate(0, XMEM_ORDINARY_ATTRIBUTES));
    REQUIRE(!allocate(1, 0x1f804000) && GetLastError() == ERROR_NOT_SUPPORTED);
    release(NULL, XMEM_ORDINARY_ATTRIBUTES);

    return 0;
}

static BOOL page_state(BYTE *at, DWORD state)
{
    MEMORY_BASIC_INFORMATION info;
    return VirtualQuery(at, &info, sizeof info) == sizeof info && info.State == state;
}

static int physical_aliases(void)
{
    const SIZE_T page = 65536;
    UINT64 tokens[2]; SIZE_T count = 2;
    REQUIRE(XMemAllocatePhysicalPages(0x20400000, &count, tokens) && count == 2);
    BYTE *a = XMemVirtualAlloc(NULL, page * 4, 0x20402000, 0x23, 0x2404);
    BYTE *b = XMemVirtualAlloc(NULL, page * 2, 0x20402000, 0x23, 0x10404);
    REQUIRE(a && b && page_state(a, MEM_RESERVE) && page_state(b, MEM_RESERVE));
    UINT64 twice[] = {tokens[0], tokens[0]};
    REQUIRE(XMemMapPhysicalPages(a + page, 2, twice) == a + page);
    REQUIRE(XMemMapPhysicalPages(b, 1, tokens) == b);
    for (SIZE_T i = 0; i < page; ++i) {
        a[page + i] = (BYTE)(i ^ (i >> 8));
        REQUIRE(a[2 * page + i] == a[page + i] && b[i] == a[page + i]);
    }
    b[34] = 99;
    REQUIRE(a[page + 34] == 99 && a[2 * page + 34] == 99);
    // A zero entry unmaps only its own address; another entry can replace
    // an occupied mapping in the same call. Other aliases retain their data.
    void *mixed[] = {a + page, b};
    UINT64 replace[] = {0, tokens[1]};
    REQUIRE(XMemMapPhysicalPagesScatter(mixed, 2, replace));
    REQUIRE(page_state(a + page, MEM_RESERVE) && a[2 * page + 34] == 99);
    b[34] = 77;
    REQUIRE(a[2 * page + 34] == 99);
    UINT64 restore[] = {tokens[0], tokens[0]};
    REQUIRE(XMemMapPhysicalPagesScatter(mixed, 2, restore));
    REQUIRE(a[page + 34] == 99 && b[34] == 99);
    // A failing replacement must restore an earlier detached page, including
    // its former token and bytes, before returning to the title.
    BYTE *foreign = VirtualAlloc(NULL, page, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    REQUIRE(foreign);
    void *invalid[] = {b, foreign};
    REQUIRE(!XMemMapPhysicalPagesScatter(invalid, 2, replace) && b[34] == 99);
    REQUIRE(VirtualFree(foreign, 0, MEM_RELEASE));
    REQUIRE(XMemMapPhysicalPages(a + page, 2, NULL) == a + page);
    REQUIRE(page_state(a + page, MEM_RESERVE) && b[34] == 99);
    REQUIRE(XMemMapPhysicalPages(b, 1, NULL) == b);
    void *spread[] = {b + page, a + 3 * page};
    REQUIRE(XMemMapPhysicalPagesScatter(spread, 2, twice));
    REQUIRE(b[page + 34] == 99 && a[3 * page + 34] == 99);
    a[3 * page + 34] = 101;
    REQUIRE(b[page + 34] == 101 && XMemMapPhysicalPagesScatter(spread, 2, NULL));
    MEMORY_BASIC_INFORMATION region;
    REQUIRE(VirtualQuery(a, &region, sizeof region) && region.RegionSize == page * 4);
    REQUIRE(VirtualQuery(b, &region, sizeof region) && region.RegionSize == page * 2);
    REQUIRE(VirtualFree(a, 0, MEM_RELEASE) && VirtualFree(b, 0, MEM_RELEASE));
    BYTE *ordinary = VirtualAlloc(NULL, page, MEM_RESERVE, PAGE_NOACCESS);
    REQUIRE(ordinary && XMemMapPhysicalPages(ordinary, 1, tokens) == ordinary && ordinary[34] == 101);
    ordinary[34] = 102;
    REQUIRE(XMemMapPhysicalPages(ordinary, 1, NULL) == ordinary && VirtualFree(ordinary, 0, MEM_RELEASE));
    a = XMemVirtualAlloc(NULL, page, 0x20402000, 0x23, 0x2404);
    REQUIRE(a && XMemMapPhysicalPages(a, 1, tokens) == a && a[34] == 102);
    b = XMemVirtualAlloc(NULL, page, 0x20402000, 0x23, 0x10404);
    REQUIRE(b && XMemMapPhysicalPages(b, 1, tokens) == b && b[34] == 102);
    REQUIRE(XMemFreePhysicalPages(2, tokens));
    REQUIRE(page_state(a, MEM_RESERVE) && page_state(b, MEM_RESERVE));
    REQUIRE(VirtualFree(a, 0, MEM_RELEASE) && VirtualFree(b, 0, MEM_RELEASE));
    return 0;
}

static int physical_pages(void)
{
    enum { page = 65536 };
    const UINT64 flags = 0x20400000ULL, guard = 0xbfc3711234ULL;
    UINT64 storage[4] = {guard, 0, 0, guard}, *tokens = &storage[1];
    SIZE_T count = 2;
    HMODULE dll = GetModuleHandleW(L"xmem.dll");
    REQUIRE(GetProcAddress(dll, "XMemAllocatePhysicalPages") == GetProcAddress(dll, MAKEINTRESOURCEA(3)));
    REQUIRE(GetProcAddress(dll, "XMemMapPhysicalPages") == GetProcAddress(dll, MAKEINTRESOURCEA(22)));
    REQUIRE(GetProcAddress(dll, "XMemFreePhysicalPages") == GetProcAddress(dll, MAKEINTRESOURCEA(8)));
    if (!XMemAllocatePhysicalPages(flags, &count, tokens)) {
        DWORD error = GetLastError();
        if (report) fprintf(report, "XMemAllocatePhysicalPages failed error=%lu returned_pages=%zu\n", error, count);
        return failure(__LINE__, "allocate two physical pages");
    }
    REQUIRE(count == 2 && tokens[0] != tokens[1] && storage[0] == guard && storage[3] == guard);
    BYTE *base = VirtualAlloc(NULL, page * 4, MEM_RESERVE, PAGE_NOACCESS);
    REQUIRE(base && XMemMapPhysicalPages(base + page, count, tokens) == base + page);
    REQUIRE(page_state(base, MEM_RESERVE) && page_state(base + 3 * page, MEM_RESERVE));
    REQUIRE(!XMemFreePhysicalPages(count, tokens) && GetLastError() == ERROR_BUSY);
    for (UINT i = 0; i < page; ++i) {
        base[page + i] = (BYTE)(i ^ (i >> 8));
        base[2 * page + i] = (BYTE)~(i ^ (i >> 8));
    }
    REQUIRE(!XMemMapPhysicalPages(base, 2, NULL));
    REQUIRE(page_state(base + page, MEM_COMMIT));
    REQUIRE(XMemMapPhysicalPages(base + page, count, NULL) == base + page);
    REQUIRE(page_state(base + page, MEM_RESERVE) && page_state(base + page * 2, MEM_RESERVE));
    UINT64 reversed[] = {tokens[1], tokens[0]}, duplicate[] = {tokens[0], tokens[0]};
    UINT64 invalid[] = {tokens[0], UINT64_MAX};
    REQUIRE(!XMemFreePhysicalPages(2, invalid));
    REQUIRE(!XMemFreePhysicalPages(2, duplicate));
    REQUIRE(!XMemMapPhysicalPages(base, 2, duplicate) && page_state(base, MEM_RESERVE));
    REQUIRE(!XMemMapPhysicalPages(base + 1, 1, tokens));
    REQUIRE(XMemMapPhysicalPages(base, count, reversed) == base);
    for (UINT i = 0; i < page; ++i) {
        REQUIRE(base[i] == (BYTE)~(i ^ (i >> 8)));
        REQUIRE(base[page + i] == (BYTE)(i ^ (i >> 8)));
    }
    REQUIRE(XMemMapPhysicalPages(base, 1, NULL) == base);
    REQUIRE(XMemFreePhysicalPages(1, &reversed[0]));
    REQUIRE(!XMemMapPhysicalPages(base + 2 * page, 1, &reversed[0]));
    REQUIRE(!XMemFreePhysicalPages(1, &reversed[0]));
    REQUIRE(page_state(base + page, MEM_COMMIT));
    REQUIRE(XMemMapPhysicalPages(base + page, 1, NULL) == base + page);
    REQUIRE(XMemFreePhysicalPages(1, &reversed[1]));
    REQUIRE(VirtualAlloc(base, page, MEM_COMMIT, PAGE_READWRITE) == base);
    REQUIRE(!XMemMapPhysicalPages(base, 1, NULL) && page_state(base, MEM_COMMIT));
    REQUIRE(VirtualFree(base, 0, MEM_RELEASE) && page_state(base, MEM_FREE));
    REQUIRE(storage[0] == guard && storage[3] == guard);
    REQUIRE(!XMemAllocatePhysicalPages(0x20000000, &count, tokens) && !count && GetLastError() == ERROR_NOT_SUPPORTED);
    return 0;
}

static int scattered_pages(void)
{
    const SIZE_T page = 65536;
    HMODULE module = GetModuleHandleW(L"xmem.dll");
    REQUIRE(GetProcAddress(module, "XMemMapPhysicalPagesScatter") == GetProcAddress(module, MAKEINTRESOURCEA(23)));
    BYTE *base = VirtualAlloc(NULL, 8 * page, MEM_RESERVE, PAGE_NOACCESS);
    REQUIRE(base);
    SIZE_T count = 3;
    UINT64 tokens[3];
    REQUIRE(XMemAllocatePhysicalPages(0x20400000, &count, tokens) && count == 3);
    void *mapped[] = {base, base + 3 * page, base + 6 * page};
    REQUIRE(XMemMapPhysicalPagesScatter(mapped, 0, tokens));
    REQUIRE(!XMemMapPhysicalPagesScatter(NULL, 0, tokens));
    REQUIRE(XMemMapPhysicalPagesScatter(mapped, 3, tokens));
    for (unsigned i = 0; i < 3; ++i) {
        REQUIRE(page_state(mapped[i], MEM_COMMIT));
        memset(mapped[i], (int)(0x31 + i), page);
    }
    REQUIRE(page_state(base + page, MEM_RESERVE) && page_state(base + 2 * page, MEM_RESERVE));
    void *duplicate[] = {mapped[0], mapped[0]};
    REQUIRE(!XMemMapPhysicalPagesScatter(duplicate, 2, NULL));
    REQUIRE(page_state(mapped[0], MEM_COMMIT));
    REQUIRE(XMemMapPhysicalPagesScatter(mapped, 3, NULL));
    for (unsigned i = 0; i < 3; ++i) REQUIRE(page_state(mapped[i], MEM_RESERVE));
    void *reverse[] = {base + page, base + 4 * page, base + 7 * page};
    UINT64 reversed[] = {tokens[2], tokens[1], tokens[0]};
    UINT64 duplicates[] = {tokens[0], tokens[0], tokens[2]};
    REQUIRE(!XMemMapPhysicalPagesScatter(reverse, 3, duplicates));
    // A late failure must restore earlier mappings and their page contents.
    BYTE *foreign = VirtualAlloc(base + 2 * page, page, MEM_COMMIT, PAGE_READWRITE);
    REQUIRE(foreign == base + 2 * page);
    foreign[0] = 0x5a;
    void *mixed[] = {reverse[0], reverse[1], foreign};
    REQUIRE(!XMemMapPhysicalPagesScatter(mixed, 3, reversed));
    REQUIRE(page_state(reverse[0], MEM_RESERVE) && page_state(reverse[1], MEM_RESERVE) && foreign[0] == 0x5a);
    REQUIRE(VirtualFree(foreign, page, MEM_DECOMMIT));
    REQUIRE(XMemMapPhysicalPagesScatter(reverse, 3, reversed));
    for (unsigned i = 0; i < 3; ++i) {
        BYTE *p = reverse[i];
        for (SIZE_T n = 0; n < page; ++n) REQUIRE(p[n] == 0x33 - i);
    }
    REQUIRE(XMemMapPhysicalPagesScatter(reverse, 3, NULL));
    REQUIRE(XMemFreePhysicalPages(3, tokens));
    REQUIRE(VirtualFree(base, 0, MEM_RELEASE));
    return 0;
}

int wmain(int argc, wchar_t **argv)
{
    BOOL pages_only = argc >= 2 && !wcscmp(argv[1], L"--pages");
    BOOL query_only = argc >= 2 && !wcscmp(argv[1], L"--query");
    BOOL tag_only = argc >= 2 && !wcscmp(argv[1], L"--tags");
    BOOL hooks_only = argc >= 2 && !wcscmp(argv[1], L"--hooks");
    BOOL watch_only = argc >= 2 && !wcscmp(argv[1], L"--watch");
    BOOL scatter_only = argc >= 2 && !wcscmp(argv[1], L"--scatter");
    BOOL cpu_profiles_only = argc >= 2 && !wcscmp(argv[1], L"--virtual-cpu");
    BOOL heap_only = argc >= 2 && !wcscmp(argv[1], L"--heap-profiles");
    BOOL heaps_only = argc >= 2 && !wcscmp(argv[1], L"--heaps");
    if (pages_only || query_only || tag_only || hooks_only || watch_only || scatter_only || cpu_profiles_only || heap_only || heaps_only) { --argc; ++argv; }
    if (!(argc == 1 || (argc == 3 && wcscmp(argv[1], L"--report") == 0))) {
        fputs("usage: xmem-smoke.exe [--pages|--query|--tags|--hooks|--watch|--scatter|--virtual-cpu|--heap-profiles|--heaps] [--report FILE]\n", stderr);
        return 2;
    }
    if (argc == 3 && _wfopen_s(&report, argv[2], L"wb") != 0) return 2;
    REQUIRE(app_local_provider());
    if (heaps_only) {
        REQUIRE(!title_and_private_heaps());
        REQUIRE(!allocator_pools());
        REQUIRE(!heap_diagnostics());
        const char result[] = "xmem-heaps PASS title=1 private=4 invalid=2 pool-commits=2 pool-decommits=2 heap-callbacks=2 stats=1 dump=1 history=2 page-classes=3\n";
        if (report && (fputs(result, report) == EOF || fclose(report))) return 2;
        fputs(result, stdout);
        return 0;
    }
    if (heap_only) {
        REQUIRE(!heap_profiles());
        const char result[] = "xmem-heap-profiles PASS attributes=40 alloc=80 free=80 null-free=2\n";
        if (report && (fputs(result, report) == EOF || fclose(report))) return 2;
        fputs(result, stdout);
        return 0;
    }
    if (cpu_profiles_only) {
        REQUIRE(!virtual_cpu_profiles());
        const char result[] = "xmem-virtual-cpu PASS protections=6 allocations=12 guards=5 invalid=1\n";
        if (report && (fputs(result, report) == EOF || fclose(report))) return 2;
        fputs(result, stdout);
        return 0;
    }
    if (scatter_only) {
        REQUIRE(!scattered_pages());
        const char result[] = "xmem-scatter PASS pages=3 remapped-bytes=196608 duplicates=2 guards=2 rollback=1\n";
        if (report && (fputs(result, report) == EOF || fclose(report))) return 2;
        fputs(result, stdout);
        return 0;
    }
    if (watch_only) {
        REQUIRE(!write_watch());
        const char result[] = "xmem-watch PASS initial=2 reset=2 next=1 guards=2 invalid=1 pause=2 resume=3 allocations=2 direct-tags=2 retail-apertures=2\n";
        if (report && (fputs(result, report) == EOF || fclose(report))) return 2;
        fputs(result, stdout);
        return 0;
    }
    if (hooks_only) {
        REQUIRE(!allocation_hooks());
        const char result[] = "xmem-hooks PASS calls=2/2 zero-size=1 null-free=1 restore=1 reentrant=1\n";
        if (report && (fputs(result, report) == EOF || fclose(report))) return 2;
        fputs(result, stdout);
        return 0;
    }
    if (tag_only) {
        REQUIRE(!allocation_tags());
        const char result[] = "xmem-tags PASS count=5120 hash=d4bf30c57509b176\n";
        if (report && (fputs(result, report) == EOF || fclose(report))) return 2;
        fputs(result, stdout);
        return 0;
    }
    if (query_only) {
        REQUIRE(!virtual_query());
        const char result[] = "xmem-query PASS cpu=5 released=1 unsupported=3 guards=2 null=1\n";
        if (report && (fputs(result, report) == EOF || fclose(report))) return 2;
        fputs(result, stdout);
        return 0;
    }
    if (pages_only) {
        REQUIRE(!physical_pages());
        REQUIRE(!physical_aliases());
        const char result[] = "xmem-pages PASS cpu-pages=2 remapped-bytes=131072 stale-and-foreign-rejected=1 shared-aliases=3 scatter-aliases=2 rollback=1 free-unmaps=2\n";
        if (report && (fputs(result, report) == EOF || fclose(report))) return 2;
        fputs(result, stdout);
        return 0;
    }
    REQUIRE(!working_set());
    REQUIRE(!virtual_reserve_commit());
    HMODULE module = GetModuleHandleW(L"xmem.dll");
    REQUIRE(GetProcAddress(module, "XMemAlloc") == GetProcAddress(module, MAKEINTRESOURCEA(1)));
    REQUIRE(GetProcAddress(module, "XMemFree") == GetProcAddress(module, MAKEINTRESOURCEA(6)));
    REQUIRE(GetProcAddress(module, "XMemAllocDefault") == GetProcAddress(module, MAKEINTRESOURCEA(2)));
    REQUIRE(GetProcAddress(module, "XMemFreeDefault") == GetProcAddress(module, MAKEINTRESOURCEA(7)));
    REQUIRE(!allocation_pair(XMemAlloc, XMemFree));
    REQUIRE(!allocation_pair(XMemAllocDefault, XMemFreeDefault));

    const char result[] = "xmem-default: PASS (allocator pairs 1/6 and 2/7, ordinary and 64 KiB allocations, host working set v2, virtual reserve/commit)\n";
    if (report) {
        int wrote = fputs(result, report);
        if (fclose(report) || wrote == EOF) return 2;
    }
    fputs(result, stdout);
    return 0;
}
