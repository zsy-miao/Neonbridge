"""API Windows de gestion des modules : GetModuleHandle, GetProcAddress, LoadLibrary…

Une DLL système n'existe pas en mémoire : elle reçoit un pseudo-handle, et
GetProcAddress renvoie l'entrée HLE de la fonction si elle est émulée.
"""
from .core import hle

ERROR_MOD_NOT_FOUND = 126
ERROR_PROC_NOT_FOUND = 127
GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS = 0x4
MAX_ORDINAL = 0xFFFF
MAX_PATH_CHARS = 260
TITLE_ROOT = "G:\\"   # racine du titre sur la console


def _modules(c):
    return c.machine.modules


def _system_handle(c, name):
    from ..modules import dll_name

    name = dll_name(name)
    for handle, known in c.machine.system_modules.items():
        if known == name:
            return handle
    handle = c.machine.new_handle(f"module:{name}")
    c.machine.system_modules[handle] = name
    return handle


def _handle_for(c, name):
    """Handle d'un module par nom : base d'un module chargé ou pseudo-handle système."""
    from ..modules import is_system

    module = _modules(c).get(name)
    if module is not None:
        return module.image.base
    if is_system(name):
        return _system_handle(c, name)
    c.machine.last_error = ERROR_MOD_NOT_FOUND
    return 0


def _name(c, address, wide):
    return c.wstr(address) if wide else c.cstr(address)


def _get_module_handle(c, wide):
    name_ptr = c.arg(0)
    if name_ptr == 0:
        return c.machine.image_base
    return _handle_for(c, _name(c, name_ptr, wide))


@hle("kernel")
def GetModuleHandleW(c):
    return _get_module_handle(c, wide=True)


@hle("kernel")
def GetModuleHandleA(c):
    return _get_module_handle(c, wide=False)


def _get_module_handle_ex(c, wide):
    flags, name_ptr, out = c.arg(0) & 0xFFFF_FFFF, c.arg(1), c.arg(2)
    if flags & GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS:
        module = _modules(c).module_at(name_ptr)
        handle = module.image.base if module else 0
    elif name_ptr == 0:
        handle = c.machine.image_base
    else:
        handle = _handle_for(c, _name(c, name_ptr, wide))
    if out:
        c.write_u64(out, handle)
    if not handle:
        c.machine.last_error = ERROR_MOD_NOT_FOUND
    return int(handle != 0)


@hle("kernel")
def GetModuleHandleExW(c):
    return _get_module_handle_ex(c, wide=True)


@hle("kernel")
def GetModuleHandleExA(c):
    return _get_module_handle_ex(c, wide=False)


def _load_library(c, wide):
    name = _name(c, c.arg(0), wide)
    handle = _handle_for(c, name)
    if not handle and _modules(c).find_file(name) is not None:
        # Charger une DLL pendant un appel imposerait de relancer Unicorn pour son
        # DllMain : le prototype refuse proprement et le jeu voit un échec normal.
        c.machine.log(f"LoadLibrary({name}) : chargement dynamique non émulé.")
    return handle


@hle("kernel", "LoadLibraryW", "LoadLibraryExW", "LoadPackagedLibrary")
def load_library_w(c):
    return _load_library(c, wide=True)


@hle("kernel", "LoadLibraryA", "LoadLibraryExA")
def load_library_a(c):
    return _load_library(c, wide=False)


@hle("kernel")
def FreeLibrary(c):
    return 1


@hle("kernel")
def GetProcAddress(c):
    from . import resolve

    handle, name_ptr = c.arg(0), c.arg(1)
    name = f"#{name_ptr}" if name_ptr <= MAX_ORDINAL else c.cstr(name_ptr)
    modules = _modules(c)
    module = modules.by_base(handle)
    address = 0
    if module is not None:
        address = modules.export(module, name) or 0
    elif handle in c.machine.system_modules:
        dll = c.machine.system_modules[handle]
        if resolve(dll, name)[0] != "absent":
            address = c.machine.thunk_for(dll, name)
    if not address:
        c.machine.last_error = ERROR_PROC_NOT_FOUND
    return address


@hle("kernel")
def GetModuleFileNameW(c):
    handle, buffer, size = c.arg(0), c.arg(1), c.arg(2) & 0xFFFF_FFFF
    module = c.machine.main if handle == 0 else _modules(c).by_base(handle)
    if module is None or not buffer or size == 0:
        c.machine.last_error = ERROR_MOD_NOT_FOUND
        return 0
    path = (TITLE_ROOT + module.path.name)[:min(size, MAX_PATH_CHARS) - 1]
    c.write(buffer, path.encode("utf-16-le") + b"\0\0")
    return len(path)


@hle("kernel")
def RtlPcToFileHeader(c):
    module = _modules(c).module_at(c.arg(0))
    base = module.image.base if module else 0
    if c.arg(1):
        c.write_u64(c.arg(1), base)
    return base
