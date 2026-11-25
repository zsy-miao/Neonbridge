"""Liaisons ctypes vers kernel32 et ntdll pour le chargeur natif."""
import ctypes
from ctypes import wintypes as wt

MEM_COMMIT_RESERVE = 0x3000
MEM_RELEASE = 0x8000
PAGE_READONLY, PAGE_READWRITE = 0x02, 0x04
PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE = 0x20, 0x40
LOAD_WITH_ALTERED_SEARCH_PATH = 0x8
LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32 = 0x100, 0x800
ERROR_PROC_NOT_FOUND = 127

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)


def _bind(name, restype, *argtypes):
    function = getattr(kernel32, name)
    function.restype = restype
    function.argtypes = argtypes
    return function


VirtualAlloc = _bind("VirtualAlloc", ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t, wt.DWORD, wt.DWORD)
VirtualFree = _bind("VirtualFree", wt.BOOL, ctypes.c_void_p, ctypes.c_size_t, wt.DWORD)
VirtualProtect = _bind("VirtualProtect", wt.BOOL, ctypes.c_void_p, ctypes.c_size_t, wt.DWORD,
                       ctypes.POINTER(wt.DWORD))
FlushInstructionCache = _bind("FlushInstructionCache", wt.BOOL, wt.HANDLE, ctypes.c_void_p, ctypes.c_size_t)
GetCurrentProcess = _bind("GetCurrentProcess", wt.HANDLE)
LoadLibraryExW = _bind("LoadLibraryExW", ctypes.c_void_p, ctypes.c_void_p, wt.HANDLE, wt.DWORD)
LoadLibraryExA = _bind("LoadLibraryExA", ctypes.c_void_p, ctypes.c_void_p, wt.HANDLE, wt.DWORD)
FreeLibrary = _bind("FreeLibrary", wt.BOOL, ctypes.c_void_p)
GetProcAddress = _bind("GetProcAddress", ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p)
GetModuleHandleW = _bind("GetModuleHandleW", ctypes.c_void_p, ctypes.c_void_p)
GetModuleHandleA = _bind("GetModuleHandleA", ctypes.c_void_p, ctypes.c_void_p)
GetModuleFileNameW = _bind("GetModuleFileNameW", wt.DWORD, ctypes.c_void_p, ctypes.c_void_p, wt.DWORD)
GetModuleFileNameA = _bind("GetModuleFileNameA", wt.DWORD, ctypes.c_void_p, ctypes.c_void_p, wt.DWORD)
SetLastError = _bind("SetLastError", None, wt.DWORD)
RtlAddFunctionTable = _bind("RtlAddFunctionTable", wt.BOOLEAN, ctypes.c_void_p, wt.DWORD, ctypes.c_uint64)
RtlDeleteFunctionTable = _bind("RtlDeleteFunctionTable", wt.BOOLEAN, ctypes.c_void_p)


def write_u64(address, value):
    ctypes.c_uint64.from_address(address).value = value & 0xFFFF_FFFF_FFFF_FFFF


def callback_address(callback):
    return ctypes.cast(callback, ctypes.c_void_p).value


GetCommandLineW = _bind("GetCommandLineW", ctypes.c_void_p)
GetCommandLineA = _bind("GetCommandLineA", ctypes.c_void_p)
