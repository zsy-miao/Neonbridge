"""kernel32 : handles, événements, attentes, conversions de texte, fichiers (stubs)."""
import os
import struct

from .core import S_OK, hle

WAIT_OBJECT_0 = 0
STARTUPINFO_SIZE = 104
INVALID_FILE_ATTRIBUTES = 0xFFFF_FFFF
ERROR_FILE_NOT_FOUND = 2
CP_UTF8 = 65001
GUID_SIZE = 16
IDOK = 1


@hle("kernel")
def CloseHandle(c):
    return int(c.machine.close_handle(c.arg(0)) or c.arg(0) != 0)


@hle("kernel", "CreateEventExW", "CreateEventW", "CreateEventA", "CreateEventExA")
def create_event(c):
    return c.machine.new_handle("event")


@hle("kernel", "SetEvent", "ResetEvent")
def event_signal(c):
    return 1


@hle("kernel", "WaitForSingleObjectEx", "WaitForSingleObject",
     "WaitForMultipleObjectsEx", "WaitForMultipleObjects")
def wait(c):
    # Un seul thread : personne d'autre ne peut signaler l'objet, on ne bloque pas.
    return WAIT_OBJECT_0


@hle("kernel")
def LocalFree(c):
    c.heap.free(c.arg(0))
    return 0


@hle("kernel")
def GetStartupInfoW(c):
    c.write(c.arg(0), struct.pack("<I", STARTUPINFO_SIZE) + bytes(STARTUPINFO_SIZE - 4))


@hle("kernel")
def SetUnhandledExceptionFilter(c):
    previous, c.machine.exception_filter = c.machine.exception_filter, c.arg(0)
    return previous


@hle("kernel")
def WideCharToMultiByte(c):
    code_page, source, count = c.arg(0) & 0xFFFF_FFFF, c.arg(2), c.arg(3) & 0xFFFF_FFFF
    out, out_size = c.arg(4), c.arg(5) & 0xFFFF_FFFF
    if count == 0xFFFF_FFFF:           # -1 : chaîne terminée par zéro, zéro inclus
        text = c.wstr(source) + "\0"
    else:
        text = c.read(source, 2 * count).decode("utf-16-le", errors="replace")
    data = text.encode("utf-8" if code_page == CP_UTF8 else "cp1252", errors="replace")
    if out_size == 0:
        return len(data)
    if len(data) > out_size:
        return 0
    c.write(out, data)
    return len(data)


@hle("kernel")
def GetModuleFileNameA(c):
    from .library import MAX_PATH_CHARS, TITLE_ROOT

    module = c.machine.main if c.arg(0) == 0 else c.machine.modules.by_base(c.arg(0))
    size = c.arg(2) & 0xFFFF_FFFF
    if module is None or size == 0:
        return 0
    path = (TITLE_ROOT + module.path.name)[:min(size, MAX_PATH_CHARS) - 1]
    c.write(c.arg(1), path.encode() + b"\0")
    return len(path)


@hle("kernel", "GetFileAttributesW", "GetFileAttributesA")
def file_attributes(c):
    # Pas encore de système de fichiers émulé : tout fichier est absent.
    c.machine.last_error = ERROR_FILE_NOT_FOUND
    return INVALID_FILE_ATTRIBUTES


@hle("kernel", "CreateDirectoryW", "CreateDirectoryA", "SetCurrentDirectoryA",
     "SetCurrentDirectoryW")
def directory_success(c):
    return 1


@hle("kernel", "FormatMessageA", "FormatMessageW", "GetLocaleInfoEx")
def no_text(c):
    return 0


@hle("kernel", "RoInitialize", "CoInitializeEx")
def ro_initialize(c):
    return S_OK


@hle("kernel", "RoUninitialize", "CoUninitialize")
def ro_uninitialize(c):
    return None


@hle("user32", "MessageBoxW", "MessageBoxA")
def message_box(c):
    """Pas d'écran pour l'instant : le message part dans le journal, réponse « OK »."""
    read = c.wstr if c.arg(1) and c.read(c.arg(1) + 1, 1) == b"\0" else c.cstr
    caption = read(c.arg(2)) if c.arg(2) else ""
    c.machine.log(f"[MessageBox] {caption} : {read(c.arg(1)) if c.arg(1) else ''}")
    return IDOK


@hle("ole32", "CoCreateGuid")
def co_create_guid(c):
    c.write(c.arg(0), os.urandom(GUID_SIZE))
    return S_OK
