// Temporary diagnostic proxy; the supplied reference DLL is never embedded.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdint.h>
#include <stdio.h>
#ifdef TRACE_UNIQUE
#include <string.h>
#endif

typedef struct {
    uint32_t dimension;
    uint64_t alignment, width;
    uint32_t height;
    uint16_t slices, mips;
    uint32_t format, samples, quality, layout, flags;
} Desc;
_Static_assert(sizeof(Desc) == 56, "unexpected x64 descriptor layout");

typedef HRESULT (WINAPI *Create)(const Desc *, void **);
static volatile LONG calls;

#ifdef TRACE_UNIQUE
// Keep the first call number even when repeated allocations fill a long trace.
static SRWLOCK trace_lock = SRWLOCK_INIT;
static uint16_t slots[32768];
static uint64_t keys[16384][12];
static LONG distinct;

static LONG trace_index(const Desc *d, HRESULT hr) {
    if (!d) return 0;
    const uint64_t key[] = {d->dimension, d->alignment, d->width, d->height,
        d->slices, d->mips, d->format, d->samples, d->quality, d->layout, d->flags, (DWORD)hr};
    uint64_t hash = 14695981039346656037ull;
    for (size_t i = 0; i < 12; ++i) hash = (hash ^ key[i]) * 1099511628211ull;
    size_t slot = (hash ^ (hash >> 32)) % 32768;
    LONG index = 0;
    AcquireSRWLockExclusive(&trace_lock);
    while (slots[slot] && memcmp(keys[slots[slot] - 1], key, sizeof(key)))
        slot = (slot + 1) % 32768;
    if (!slots[slot] && distinct <= 16384) {
        index = ++distinct;
        if (index <= 16384) {
            slots[slot] = (uint16_t)index;
            memcpy(keys[index - 1], key, sizeof(key));
        }
    }
    ReleaseSRWLockExclusive(&trace_lock);
    return index;
}
#endif

HRESULT WINAPI XGCreateTextureComputer(const Desc *desc, void **out) {
    DWORD incoming = GetLastError();
    HMODULE module = GetModuleHandleW(L"xg_reference.dll");
    if (!module) module = LoadLibraryW(L"xg_reference.dll");
    Create create = module ? (Create)(uintptr_t)GetProcAddress(module, "XGCreateTextureComputer") : NULL;
    if (!create) { SetLastError(ERROR_PROC_NOT_FOUND); return E_FAIL; }
    SetLastError(incoming);
    HRESULT hr = create(desc, out);
    DWORD last_error = GetLastError();
    LONG n = InterlockedIncrement(&calls);
#ifdef TRACE_UNIQUE
    LONG index = trace_index(desc, hr);
#else
    LONG index = n;
#endif
    if (index > 0 && index <= 16384 && desc) {
        char text[512];
        int count = snprintf(text, sizeof(text),
            "XG_REFERENCE_TRACE call=%ld hr=%08lx dimension=%u alignment=%llu width=%llu height=%u slices=%u mips=%u format=%u samples=%u quality=%u layout=%u flags=%08x\n",
            n, (unsigned long)hr, desc->dimension, (unsigned long long)desc->alignment,
            (unsigned long long)desc->width, desc->height, desc->slices, desc->mips,
            desc->format, desc->samples, desc->quality, desc->layout, desc->flags);
        if (count > 0) OutputDebugStringA(text);
    } else if (index == 16385) {
        OutputDebugStringA("XG_REFERENCE_TRACE limit=16384 reached; further calls omitted\n");
    }
    SetLastError(last_error);
    return hr;
}
