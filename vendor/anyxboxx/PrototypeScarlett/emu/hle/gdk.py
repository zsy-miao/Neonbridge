"""Fonctions du runtime GDK (xgameruntime.dll) émulées pour une Xbox Series X.

Signatures : documentation publique Microsoft GDK (learn.microsoft.com/gaming/gdk).
"""
import struct

from .core import S_OK, hle

# XSystemDeviceType : Unknown 0, Pc 1, XboxOne 2, XboxOneS 3, XboxOneX 4,
# XboxOneXDevkit 5, XboxScarlettLockhart 6 (Series S), XboxScarlettAnaconda 7 (Series X).
XBOX_SERIES_X = 7
CONSOLE_ID = "00000000.00000000.00000000.00000000.00"
DEVICE_ID = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
SANDBOX = "RETAIL"
OS_VERSION = (10, 0, 25398, 4908)
FAMILY_FIELD = 64
E_NOINTERFACE = 0x80004002


@hle("gdk")
def XSystemGetDeviceType(c):
    return XBOX_SERIES_X


@hle("gdk", "XGameRuntimeInitialize", "XGameRuntimeInitializeWithOptions")
def runtime_initialize(c):
    c.machine.runtime_ready = True
    return S_OK


@hle("gdk")
def XGameRuntimeUninitialize(c):
    c.machine.runtime_ready = False


# --- Interface interne de xgameruntime.dll utilisée par les titres GDK PC ---------
# Sur PC, xgameruntime.lib charge la DLL puis passe par ces fonctions : chaque API
# X* demande une interface à QueryApiImpl. Observé sur l'échantillon MIT
# SimpleTriangleDesktop ; les interfaces elles-mêmes ne sont pas encore émulées.

@hle("gdk", "InitializeApiImpl", "InitializeApiImplEx", "InitializeApiImplEx2")
def initialize_api(c):
    c.machine.runtime_ready = True
    return S_OK


@hle("gdk")
def UninitializeApiImpl(c):
    c.machine.runtime_ready = False


@hle("gdk")
def QueryApiImpl(c):
    """QueryApiImpl(fournisseur, interface, void** sortie) : aucune interface fournie."""
    if c.arg(2):
        c.write_u64(c.arg(2), 0)
    return E_NOINTERFACE


@hle("gdk")
def XErrorReport(c):
    c.machine.log(f"[XErrorReport] code {c.arg(0) & 0xFFFF_FFFF:#x}")


@hle("gdk")
def XGameGetXboxTitleId(c):
    c.write_u32(c.arg(0), c.machine.title_id)
    return S_OK


@hle("gdk")
def XSystemGetConsoleId(c):
    return c.write_text(c.arg(1), c.arg(0), CONSOLE_ID, c.arg(2))


@hle("gdk")
def XSystemGetAppSpecificDeviceId(c):
    return c.write_text(c.arg(1), c.arg(0), DEVICE_ID, c.arg(2))


@hle("gdk")
def XSystemGetXboxLiveSandboxId(c):
    return c.write_text(c.arg(1), c.arg(0), SANDBOX, c.arg(2))


@hle("gdk")
def XSystemGetAnalyticsInfo(c):
    """Structure renvoyée par valeur : l'appelant fournit son adresse dans RCX."""
    out = c.arg(0)
    version = struct.pack("<4H", *OS_VERSION)
    family = b"Xbox".ljust(FAMILY_FIELD, b"\0")
    form = b"Xbox Series X".ljust(FAMILY_FIELD, b"\0")
    c.write(out, version + version + family + form)
    return out


@hle("gdk")
def XMemAlloc(c):
    return c.heap.alloc(c.arg(0), alignment=64)


@hle("gdk")
def XMemFree(c):
    c.heap.free(c.arg(0))


@hle("gdk")
def XTaskQueueCreate(c):
    c.write_u64(c.arg(2), c.machine.new_handle("XTaskQueue"))
    return S_OK


@hle("gdk")
def XTaskQueueCloseHandle(c):
    c.machine.close_handle(c.arg(0))


@hle("gdk")
def XTaskQueueDispatch(c):
    return 0  # aucune tâche asynchrone en attente


@hle("gdk")
def XThreadSetTimeSensitive(c):
    return S_OK


@hle("gdk")
def XThreadIsTimeSensitive(c):
    return 0
