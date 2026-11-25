"""Sous-ensemble de kernel32 / kernelx / api-ms-win-core-* pour un titre mono-thread."""
import struct
import time

from .core import MASK64, hle

# IsProcessorFeaturePresent : PF_COMPARE_EXCHANGE_DOUBLE 2, PF_MMX 3, PF_XMMI 6,
# PF_3DNOW absent, PF_RDTSC 8, PF_PAE 9, PF_XMMI64 10, PF_NX 12, PF_SSE3 13,
# PF_COMPARE_EXCHANGE128 14, PF_FASTFAIL 23, PF_SSSE3 36, PF_SSE4_1 37, PF_SSE4_2 38.
CPU_FEATURES = frozenset({2, 3, 6, 8, 9, 10, 12, 13, 14, 23, 36, 37, 38})
ARCH_AMD64, PROCESSOR_AMD_X8664, ZEN2_FAMILY = 9, 8664, 0x17
PAGE_SIZE, ALLOCATION_GRANULARITY = 0x1000, 0x10000
MIN_APP_ADDRESS, MAX_APP_ADDRESS = 0x10000, 0x7FFF_FFFE_FFFF
ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, ERROR_ENVVAR_NOT_FOUND = 87, 50, 203
ERROR_INVALID_ADDRESS = 487
FAST_FAIL_CODE = 0xC0000409

TICKS_PER_SECOND = 10_000_000          # QueryPerformanceFrequency
FILETIME_EPOCH = 116_444_736_000_000_000  # 1601 → 1970 en unités de 100 ns
PROCESS_ID, THREAD_ID = 0x4, 0x1000
PSEUDO_PROCESS, PSEUDO_THREAD = MASK64, MASK64 - 1
MEM_RELEASE = 0x8000
TLS_OUT_OF_INDEXES = 0xFFFF_FFFF
MAX_TLS = 1088


def _elapsed(c):
    return time.perf_counter() - c.machine.started


@hle("kernel")
def GetTickCount64(c):
    return int(_elapsed(c) * 1000)


@hle("kernel")
def GetTickCount(c):
    return int(_elapsed(c) * 1000) & 0xFFFF_FFFF


@hle("kernel")
def QueryPerformanceCounter(c):
    c.write_u64(c.arg(0), int(_elapsed(c) * TICKS_PER_SECOND))
    return 1


@hle("kernel")
def QueryPerformanceFrequency(c):
    c.write_u64(c.arg(0), TICKS_PER_SECOND)
    return 1


@hle("kernel", "GetSystemTimeAsFileTime", "GetSystemTimePreciseAsFileTime")
def system_time(c):
    c.write_u64(c.arg(0), FILETIME_EPOCH + time.time_ns() // 100)


@hle("kernel", "Sleep", "SwitchToThread")
def sleep(c):
    return 1  # pas de temps réel dans l'émulation : on rend la main tout de suite


@hle("kernel")
def GetCurrentProcessId(c):
    return PROCESS_ID


@hle("kernel")
def GetCurrentThreadId(c):
    return THREAD_ID


@hle("kernel")
def GetCurrentProcess(c):
    return PSEUDO_PROCESS


@hle("kernel")
def GetCurrentThread(c):
    return PSEUDO_THREAD


@hle("kernel")
def GetLastError(c):
    return c.machine.last_error


@hle("kernel")
def SetLastError(c):
    c.machine.last_error = c.arg(0) & 0xFFFF_FFFF


@hle("kernel")
def RegisterTraceGuidsW(c):
    """Traces ETW : accepte l'inscription (8e argument = handle) sans rien journaliser."""
    if c.arg(7):
        c.write_u64(c.arg(7), c.machine.new_handle("ETW"))
    return 0  # ERROR_SUCCESS


@hle("kernel")
def UnregisterTraceGuids(c):
    c.machine.close_handle(c.arg(0))
    return 0


@hle("kernel")
def IsDebuggerPresent(c):
    return 0


@hle("kernel")
def IsProcessorFeaturePresent(c):
    # Le Zen 2 de la Series X gère AVX2, mais le processeur émulé par Unicorn
    # s'arrête à SSE4.2 : on annonce ce qu'il exécute vraiment.
    return int((c.arg(0) & 0xFFFF_FFFF) in CPU_FEATURES)


@hle("kernel", "GetSystemInfo", "GetNativeSystemInfo")
def system_info(c):
    """Remplit SYSTEM_INFO (48 octets) : x86-64, un seul processeur émulé."""
    c.write(c.arg(0), struct.pack(
        "<HHIQQQIIIHH", ARCH_AMD64, 0, PAGE_SIZE, MIN_APP_ADDRESS, MAX_APP_ADDRESS,
        1, 1, PROCESSOR_AMD_X8664, ALLOCATION_GRANULARITY, ZEN2_FAMILY, 0))


@hle("kernel", "GetEnvironmentVariableW", "GetEnvironmentVariableA")
def get_environment_variable(c):
    c.machine.last_error = ERROR_ENVVAR_NOT_FOUND  # la console n'a pas de variables d'environnement
    return 0


@hle("kernel")
def DisableThreadLibraryCalls(c):
    return 1


@hle("kernel", "EncodePointer", "DecodePointer")
def encode_pointer(c):
    return c.arg(0)  # l'obfuscation de pointeurs n'apporte rien dans la machine émulée


@hle("kernel")
def InitializeSListHead(c):
    c.write(c.arg(0), bytes(16))


@hle("kernel", "InitializeConditionVariable", "WakeConditionVariable",
     "WakeAllConditionVariable")
def condition_variable(c):
    return None  # un seul thread : personne n'attend


@hle("kernel", "GetLogicalProcessorInformationEx", "GetLogicalProcessorInformation")
def logical_processor_information(c):
    c.machine.last_error = ERROR_NOT_SUPPORTED
    return 0


@hle("kernel")
def OutputDebugStringA(c):
    c.machine.log(f"[débogage] {c.cstr(c.arg(0))}")


@hle("kernel")
def OutputDebugStringW(c):
    c.machine.log(f"[débogage] {c.wstr(c.arg(0))}")


@hle("kernel")
def ExitProcess(c):
    c.machine.request_exit(c.arg(0) & 0xFFFF_FFFF)


@hle("kernel", "RaiseException", "RaiseFailFastException")
def raise_exception(c):
    # Les exceptions SEH ne sont pas encore émulées : on arrête proprement.
    c.machine.log(f"Exception {c.arg(0) & 0xFFFF_FFFF:#x} levée : arrêt du programme.")
    c.machine.request_exit(c.arg(0) & 0xFFFF_FFFF)


@hle("kernel")
def TerminateProcess(c):
    if c.arg(0) != PSEUDO_PROCESS:
        return 0  # un seul processus existe dans la machine émulée
    c.machine.request_exit(c.arg(1) & 0xFFFF_FFFF)
    return 1


# --- Mémoire ---------------------------------------------------------------

@hle("kernel")
def GetProcessHeap(c):
    return c.machine.process_heap


@hle("kernel")
def HeapAlloc(c):
    return c.heap.alloc(c.arg(2))


@hle("kernel")
def HeapReAlloc(c):
    return c.heap.realloc(c.arg(2), c.arg(3))


@hle("kernel")
def HeapFree(c):
    c.heap.free(c.arg(2))
    return 1


@hle("kernel")
def HeapSize(c):
    size = c.heap.size_of(c.arg(2))
    return MASK64 if size is None else size


@hle("kernel")
def VirtualAlloc(c):
    address, size = c.arg(0), c.arg(1)
    if size == 0:
        c.machine.last_error = ERROR_INVALID_PARAMETER
        return 0
    if address:
        # MEM_COMMIT dans une zone déjà réservée : la mémoire existe déjà.
        if c.heap.contains(address, size):
            return address
        c.machine.last_error = ERROR_INVALID_ADDRESS
        return 0
    # Réserver et valider reviennent au même : le tas ne distingue pas les deux.
    return c.heap.alloc(size, alignment=ALLOCATION_GRANULARITY)


@hle("kernel")
def VirtualFree(c):
    if c.arg(2) & MEM_RELEASE:
        c.heap.free(c.arg(0))
    return 1


# --- Synchronisation : un seul thread émulé, les verrous sont sans effet. ---

@hle("kernel", "InitializeCriticalSection", "EnterCriticalSection",
     "LeaveCriticalSection", "DeleteCriticalSection", "InitializeSRWLock",
     "AcquireSRWLockExclusive", "ReleaseSRWLockExclusive",
     "AcquireSRWLockShared", "ReleaseSRWLockShared")
def no_lock(c):
    return None


@hle("kernel", "InitializeCriticalSectionAndSpinCount", "InitializeCriticalSectionEx",
     "TryEnterCriticalSection")
def lock_success(c):
    return 1


# --- Stockage local de thread ---------------------------------------------

@hle("kernel", "TlsAlloc", "FlsAlloc")
def tls_alloc(c):
    slots = c.machine.tls_slots
    index = next((i for i in range(MAX_TLS) if i not in slots), None)
    if index is None:
        return TLS_OUT_OF_INDEXES
    slots[index] = 0
    return index


@hle("kernel", "TlsGetValue", "FlsGetValue")
def tls_get(c):
    return c.machine.tls_slots.get(c.arg(0) & 0xFFFF_FFFF, 0)


@hle("kernel", "TlsSetValue", "FlsSetValue")
def tls_set(c):
    index = c.arg(0) & 0xFFFF_FFFF
    if index not in c.machine.tls_slots:
        return 0
    c.machine.tls_slots[index] = c.arg(1)
    return 1


@hle("kernel", "TlsFree", "FlsFree")
def tls_free(c):
    return int(c.machine.tls_slots.pop(c.arg(0) & 0xFFFF_FFFF, None) is not None)
