"""Runtime C++ (runtime/) : les DLL qui remplacent xgameruntime, xmem et d3d12_xs."""
import ctypes
import sys

import pytest

pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="DLL Windows x64")

S_OK, E_NOINTERFACE = 0, 0x80004002
XBOX_SCARLETT_ANACONDA = 7
MEM_COMMIT = 0x1000
ROOT_SIGNATURE_VERSION_1_0 = 1


def load(runtime_dir, name):
    return ctypes.WinDLL(str(runtime_dir / name))


def test_runtime_answers_like_a_series_x(runtime_dir):
    runtime = load(runtime_dir, "xgameruntime.dll")
    runtime.QueryApiImpl.restype = ctypes.c_uint32
    assert runtime.QueryApiImpl(None, None, None, None) == E_NOINTERFACE
    assert runtime.XSystemGetDeviceType() == XBOX_SCARLETT_ANACONDA
    assert runtime.XGameRuntimeInitialize() == S_OK


def test_xmem_alloc_is_aligned_and_queryable(runtime_dir):
    xmem = load(runtime_dir, "xmem.dll")
    xmem.XMemAlloc.restype = ctypes.c_void_p
    xmem.XMemAlloc.argtypes = (ctypes.c_size_t, ctypes.c_uint64)
    xmem.XMemFree.argtypes = (ctypes.c_void_p, ctypes.c_uint64)
    small, large = xmem.XMemAlloc(100, 0), xmem.XMemAlloc(8192, 0)
    try:
        assert small and small % 64 == 0
        assert large and large % 4096 == 0
        ctypes.memset(large, 0xAB, 8192)

        class MemoryInfo(ctypes.Structure):
            _fields_ = [("base", ctypes.c_void_p), ("allocation_base", ctypes.c_void_p),
                        ("allocation_protect", ctypes.c_uint32), ("partition", ctypes.c_uint16),
                        ("region_size", ctypes.c_size_t), ("state", ctypes.c_uint32),
                        ("protect", ctypes.c_uint32), ("type", ctypes.c_uint32)]

        buffer = (ctypes.c_ubyte * 64)()     # appelants BVH : processus, adresse, tampon de 64 octets
        xmem.XMemVirtualQuery.restype = ctypes.c_int32
        xmem.XMemVirtualQuery.argtypes = (ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p)
        assert xmem.XMemVirtualQuery(ctypes.c_void_p(-1), large, buffer) == 1
        assert MemoryInfo.from_buffer(buffer).state == MEM_COMMIT
    finally:
        xmem.XMemFree(small, 0)
        xmem.XMemFree(large, 0)


def test_xmem_working_set_statistics_report_unavailable(runtime_dir):
    xmem = load(runtime_dir, "xmem.dll")
    assert xmem.XMemGetWorkingSetStatistics(0, ctypes.create_string_buffer(256)) == 0


def test_d3d12_xs_serializes_root_signature_through_pc_d3d12(runtime_dir):
    d3d12_xs = load(runtime_dir, "d3d12_xs.dll")
    serialize = d3d12_xs.D3D12SerializeVersionedRootSignature
    serialize.restype = ctypes.c_uint32
    serialize.argtypes = (ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p), ctypes.POINTER(ctypes.c_void_p))
    desc = (ctypes.c_uint32 * 16)()          # D3D12_VERSIONED_ROOT_SIGNATURE_DESC vide
    desc[0] = ROOT_SIGNATURE_VERSION_1_0
    blob, error = ctypes.c_void_p(), ctypes.c_void_p()
    assert serialize(desc, ctypes.byref(blob), ctypes.byref(error)) == S_OK
    vtable = ctypes.cast(ctypes.cast(blob, ctypes.POINTER(ctypes.c_void_p))[0], ctypes.POINTER(ctypes.c_void_p))
    size = ctypes.WINFUNCTYPE(ctypes.c_size_t, ctypes.c_void_p)(vtable[4])(blob)   # ID3DBlob::GetBufferSize
    ctypes.WINFUNCTYPE(ctypes.c_uint32, ctypes.c_void_p)(vtable[2])(blob)           # Release
    assert size > 0


def test_xmem_frees_each_block_with_the_allocator_that_made_it(runtime_dir):
    """Un bloc alloué avant XMemSetAllocationHooks ne doit jamais partir dans le crochet du jeu."""
    xmem = load(runtime_dir, "xmem.dll")
    alloc_type = ctypes.WINFUNCTYPE(ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint64)
    free_type = ctypes.WINFUNCTYPE(None, ctypes.c_void_p, ctypes.c_uint64)
    pool = ctypes.create_string_buffer(256)
    freed = []
    hook_alloc = alloc_type(lambda size, attributes: ctypes.addressof(pool))
    hook_free = free_type(lambda address, attributes: freed.append(address))
    xmem.XMemAlloc.restype = ctypes.c_void_p
    xmem.XMemAlloc.argtypes = (ctypes.c_size_t, ctypes.c_uint64)
    xmem.XMemFree.argtypes = (ctypes.c_void_p, ctypes.c_uint64)
    xmem.XMemSetAllocationHooks.argtypes = (ctypes.c_void_p, ctypes.c_void_p)
    before = xmem.XMemAlloc(64, 0)
    xmem.XMemSetAllocationHooks(ctypes.cast(hook_alloc, ctypes.c_void_p), ctypes.cast(hook_free, ctypes.c_void_p))
    try:
        hooked = xmem.XMemAlloc(32, 0)
        assert hooked == ctypes.addressof(pool)
        xmem.XMemFree(before, 0)            # bloc par défaut : libéré par AnyXboxX
        assert freed == []
        xmem.XMemFree(hooked, 0)            # bloc du jeu : rendu à son crochet
        assert freed == [hooked]
    finally:
        xmem.XMemSetAllocationHooks(None, None)


SYSTEM_RUNTIME = __import__("pathlib").Path(__import__("os").environ.get("SystemRoot", r"C:\Windows")) / "System32" / "xgameruntime.dll"


def _runtime_in_subprocess(runtime_dir, system_runtime):
    """QueryApiImpl et InitializeApiImplEx2 dans un processus neuf (l'option est lue une fois)."""
    import os
    import subprocess

    script = (
        "import ctypes, sys; r = ctypes.WinDLL(sys.argv[1]); r.QueryApiImpl.restype = ctypes.c_uint32;"
        "r.InitializeApiImplEx2.restype = ctypes.c_uint32; g = (ctypes.c_ubyte * 16)(*range(16));"
        "o = ctypes.c_void_p(); print(r.QueryApiImpl(g, g, ctypes.byref(o), None), o.value)")
    env = {**os.environ, "ANYXBOXX_SYSTEM_RUNTIME": system_runtime}
    out = subprocess.run([sys.executable, "-c", script, str(runtime_dir / "xgameruntime.dll")],
                         capture_output=True, text=True, env=env, check=True).stdout.split()
    return int(out[0]), out[1]


def test_without_windows_runtime_unknown_interfaces_are_refused(runtime_dir):
    assert _runtime_in_subprocess(runtime_dir, "0") == (E_NOINTERFACE, "None")


@pytest.mark.skipif(not SYSTEM_RUNTIME.is_file(), reason="runtime GDK de Windows absent")
def test_unknown_interface_delegated_to_windows_fails_cleanly(runtime_dir):
    code, pointer = _runtime_in_subprocess(runtime_dir, "1")
    assert code & 0x8000_0000 and pointer == "None"


# Ces tests vérifient l'implémentation d'AnyXboxX : si le runtime GDK de Windows est
# présent et autorisé (ANYXBOXX_SYSTEM_RUNTIME=1), c'est lui qui sert toutes les interfaces.
anyxboxx_only = pytest.mark.skipif(
    __import__("os").environ.get("ANYXBOXX_SYSTEM_RUNTIME") != "0" and SYSTEM_RUNTIME.is_file(),
    reason="le runtime GDK de Windows sert les interfaces dans ce mode")


TASK_QUEUE_GUID = bytes.fromhex("cb7d3b07cf1f304094bee3c9eb623428")   # {073B7DCB-…} en mémoire
MANUAL, THREAD_POOL = 0, 1
WORK_PORT = 0
CALLBACK = (ctypes.WINFUNCTYPE(None, ctypes.c_void_p, ctypes.c_bool)
            if sys.platform == "win32" else None)
ASYNC_WORK = (ctypes.WINFUNCTYPE(ctypes.c_uint32, ctypes.c_void_p)
             if sys.platform == "win32" else None)


class AsyncBlock(ctypes.Structure):
    _fields_ = [("queue", ctypes.c_void_p), ("context", ctypes.c_void_p),
                ("callback", ctypes.c_void_p), ("internal", ctypes.c_ubyte * 32)]


class TaskQueueApi:
    """L'interface vue comme le jeu la voit : méthode n° k de l'objet rendu par QueryApiImpl."""

    def __init__(self, runtime_dir):
        runtime = load(runtime_dir, "xgameruntime.dll")
        runtime.QueryApiImpl.restype = ctypes.c_uint32
        guid = (ctypes.c_ubyte * 16).from_buffer_copy(TASK_QUEUE_GUID)
        self.object = ctypes.c_void_p()
        assert runtime.QueryApiImpl(guid, guid, ctypes.byref(self.object), None) == S_OK
        self.vtable = ctypes.cast(ctypes.cast(self.object, ctypes.POINTER(ctypes.c_void_p))[0],
                                  ctypes.POINTER(ctypes.c_void_p))

    def method(self, index, restype, *argtypes):
        function = ctypes.WINFUNCTYPE(restype, ctypes.c_void_p, *argtypes)(self.vtable[index])
        return lambda *args: function(self.object, *args)


@anyxboxx_only
def test_task_queue_is_served_by_anyxboxx_and_dispatches_callbacks(runtime_dir):
    api = TaskQueueApi(runtime_dir)
    create = api.method(12, ctypes.c_uint32, ctypes.c_int, ctypes.c_int, ctypes.POINTER(ctypes.c_void_p))
    submit = api.method(18, ctypes.c_uint32, ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p, CALLBACK)
    dispatch = api.method(16, ctypes.c_bool, ctypes.c_void_p, ctypes.c_int, ctypes.c_uint32)
    close = api.method(17, None, ctypes.c_void_p)
    queue = ctypes.c_void_p()
    assert create(MANUAL, MANUAL, ctypes.byref(queue)) == S_OK
    calls = []
    callback = CALLBACK(lambda context, canceled: calls.append((context, canceled)))
    assert submit(queue, WORK_PORT, 42, callback) == S_OK
    assert calls == []                              # file manuelle : rien avant Dispatch
    assert dispatch(queue, WORK_PORT, 0)
    assert calls == [(42, False)]
    close(queue)


@anyxboxx_only
def test_async_run_completes_on_thread_pool_queue(runtime_dir):
    api = TaskQueueApi(runtime_dir)
    create = api.method(12, ctypes.c_uint32, ctypes.c_int, ctypes.c_int, ctypes.POINTER(ctypes.c_void_p))
    run = api.method(6, ctypes.c_uint32, ctypes.POINTER(AsyncBlock), ASYNC_WORK)
    status = api.method(3, ctypes.c_uint32, ctypes.POINTER(AsyncBlock), ctypes.c_bool)
    close = api.method(17, None, ctypes.c_void_p)
    queue = ctypes.c_void_p()
    assert create(THREAD_POOL, THREAD_POOL, ctypes.byref(queue)) == S_OK
    block = AsyncBlock(queue=queue)
    work = ASYNC_WORK(lambda pointer: 0x1234)
    assert run(ctypes.byref(block), work) == S_OK
    assert status(ctypes.byref(block), True) == 0x1234   # attend la fin du travail
    close(queue)


@anyxboxx_only
def test_thread_time_sensitivity_is_per_thread(runtime_dir):
    api = TaskQueueApi(runtime_dir)
    set_sensitive = api.method(27, ctypes.c_uint32, ctypes.c_bool)
    is_sensitive = api.method(30, ctypes.c_bool)
    assert set_sensitive(True) == S_OK and is_sensitive()
    assert set_sensitive(False) == S_OK and not is_sensitive()


def guid_bytes(text):
    import uuid
    return uuid.UUID(text).bytes_le


USER_PROVIDER = "01ACD177-91F9-4763-A38E-CCBB55CE32E0"
GAMERTAG_IID = "CEF4FAC0-7676-4A94-A119-4C43F9EB5B74"
SIGN_OUT_IID = "5131D685-4394-4EE6-8C18-BFB5D4AEF1FF"
LOCAL_XUID = 0x0009000000A11B0B
E_GAMEUSER_USER_NOT_FOUND = 0x89245104


class GdkInterface(TaskQueueApi):
    """N'importe quelle interface (fournisseur, interface) servie par le runtime."""

    def __init__(self, runtime_dir, provider, iid):
        runtime = load(runtime_dir, "xgameruntime.dll")
        runtime.QueryApiImpl.restype = ctypes.c_uint32
        provider_guid = (ctypes.c_ubyte * 16).from_buffer_copy(guid_bytes(provider))
        iid_guid = (ctypes.c_ubyte * 16).from_buffer_copy(guid_bytes(iid))
        self.object = ctypes.c_void_p()
        assert runtime.QueryApiImpl(provider_guid, iid_guid, ctypes.byref(self.object), None) == S_OK
        self.vtable = ctypes.cast(ctypes.cast(self.object, ctypes.POINTER(ctypes.c_void_p))[0],
                                  ctypes.POINTER(ctypes.c_void_p))


def add_local_user(runtime_dir, method=7, result_method=8, argument=0):
    """XUserAddAsync (ou AddByIdWithUi) sur une file du pool de threads, attendue jusqu'au bout."""
    tasks, users = TaskQueueApi(runtime_dir), GdkInterface(runtime_dir, USER_PROVIDER, USER_PROVIDER)
    queue = ctypes.c_void_p()
    create = tasks.method(12, ctypes.c_uint32, ctypes.c_int, ctypes.c_int, ctypes.POINTER(ctypes.c_void_p))
    assert create(THREAD_POOL, THREAD_POOL, ctypes.byref(queue)) == S_OK
    block = AsyncBlock(queue=queue)
    begin = users.method(method, ctypes.c_uint32, ctypes.c_uint64, ctypes.POINTER(AsyncBlock))
    assert begin(argument, ctypes.byref(block)) == S_OK
    status = tasks.method(3, ctypes.c_uint32, ctypes.POINTER(AsyncBlock), ctypes.c_bool)(ctypes.byref(block), True)
    user = ctypes.c_void_p()
    result = users.method(result_method, ctypes.c_uint32, ctypes.POINTER(AsyncBlock), ctypes.POINTER(ctypes.c_void_p))
    outcome = result(ctypes.byref(block), ctypes.byref(user))
    tasks.method(17, None, ctypes.c_void_p)(queue)
    return users, status, outcome, user


@anyxboxx_only
def test_offline_user_signs_in_without_xbox_live(runtime_dir):
    users, status, outcome, user = add_local_user(runtime_dir)
    assert status == S_OK and outcome == S_OK and user.value
    xuid, state, local = ctypes.c_uint64(), ctypes.c_uint32(9), ctypes.c_uint64()
    assert users.method(11, ctypes.c_uint32, ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint64))(user, ctypes.byref(xuid)) == S_OK
    assert users.method(14, ctypes.c_uint32, ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint32))(user, ctypes.byref(state)) == S_OK
    assert users.method(9, ctypes.c_uint32, ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint64))(user, ctypes.byref(local)) == S_OK
    assert (xuid.value, state.value, local.value) == (LOCAL_XUID, 0, 1)   # 0 = XUserState::SignedIn
    allowed, reason = ctypes.c_bool(), ctypes.c_uint32(7)
    check = users.method(20, ctypes.c_uint32, ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32,
                         ctypes.POINTER(ctypes.c_bool), ctypes.POINTER(ctypes.c_uint32))
    assert check(user, 0, 254, ctypes.byref(allowed), ctypes.byref(reason)) == S_OK and allowed.value and reason.value == 0
    users.method(4, None, ctypes.c_void_p)(user)


@anyxboxx_only
def test_gamertag_has_its_own_interface(runtime_dir):
    _, _, _, user = add_local_user(runtime_dir)
    gamertag = GdkInterface(runtime_dir, USER_PROVIDER, GAMERTAG_IID)
    get = gamertag.method(3, ctypes.c_uint32, ctypes.c_void_p, ctypes.c_uint32, ctypes.c_size_t,
                          ctypes.c_char_p, ctypes.POINTER(ctypes.c_size_t))
    buffer, used = ctypes.create_string_buffer(16), ctypes.c_size_t()
    assert get(user, 0, 16, buffer, ctypes.byref(used)) == S_OK
    assert buffer.value == b"AnyXboxX" and used.value == 9
    assert get(user, 0, 4, ctypes.create_string_buffer(4), ctypes.byref(used)) == 0x8007007A   # tampon trop petit


@anyxboxx_only
def test_sign_out_interface_shares_the_user_table(runtime_dir):
    sign_out = GdkInterface(runtime_dir, USER_PROVIDER, SIGN_OUT_IID)
    assert not sign_out.method(47, ctypes.c_bool)()          # XUserIsSignOutPresent


@anyxboxx_only
def test_unknown_user_id_is_not_found(runtime_dir):
    _, status, outcome, user = add_local_user(runtime_dir, method=37, result_method=38, argument=1234)
    assert status == E_GAMEUSER_USER_NOT_FOUND and outcome == E_GAMEUSER_USER_NOT_FOUND and not user.value


def test_registry_presents_a_series_x(runtime_dir):
    runtime = load(runtime_dir, "xgameruntime.dll")
    key, value, size = ctypes.c_void_p(), ctypes.c_uint32(), ctypes.c_uint32(4)
    hklm = ctypes.c_void_p(0xFFFFFFFF80000002)
    path = r"Software\Microsoft\Windows NT\CurrentVersion\OEM"
    runtime.AnyXboxX_RegOpenKeyExW.argtypes = (ctypes.c_void_p, ctypes.c_wchar_p, ctypes.c_uint32,
                                               ctypes.c_uint32, ctypes.POINTER(ctypes.c_void_p))
    runtime.AnyXboxX_RegQueryValueExW.argtypes = (ctypes.c_void_p, ctypes.c_wchar_p, ctypes.c_void_p,
                                                  ctypes.c_void_p, ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint32))
    runtime.AnyXboxX_RegCloseKey.argtypes = (ctypes.c_void_p,)
    assert runtime.AnyXboxX_RegOpenKeyExW(hklm, path, 0, 0x20019, ctypes.byref(key)) == 0
    assert runtime.AnyXboxX_RegQueryValueExW(key, "ConsoleMode", None, None, ctypes.byref(value), ctypes.byref(size)) == 0
    assert value.value == 0x64                       # → XSystemDeviceType::XboxScarlettAnaconda
    assert runtime.AnyXboxX_RegQueryValueExW(key, "Autre", None, None, None, ctypes.byref(size)) == 2
    assert runtime.AnyXboxX_RegCloseKey(key) == 0
    real = ctypes.c_void_p()                         # les autres clés vont au vrai registre
    assert runtime.AnyXboxX_RegOpenKeyExW(hklm, "Software", 0, 0x20019, ctypes.byref(real)) == 0
    assert runtime.AnyXboxX_RegCloseKey(real) == 0


def test_registry_reg_open_key_variant_pins_console_mode_0x64(runtime_dir):
    """RegOpenKeyW (la variante du titre, étape 9) : même clé simulée, ConsoleMode figé à 0x64."""
    runtime = load(runtime_dir, "xgameruntime.dll")
    hklm = ctypes.c_void_p(0xFFFFFFFF80000002)
    runtime.AnyXboxX_RegOpenKeyW.argtypes = (ctypes.c_void_p, ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_void_p))
    runtime.AnyXboxX_RegQueryValueExW.argtypes = (ctypes.c_void_p, ctypes.c_wchar_p, ctypes.c_void_p,
                                                  ctypes.POINTER(ctypes.c_uint32), ctypes.c_void_p,
                                                  ctypes.POINTER(ctypes.c_uint32))
    runtime.AnyXboxX_RegCloseKey.argtypes = (ctypes.c_void_p,)
    for path in (r"Software\Microsoft\Windows NT\CurrentVersion\OEM", r"\SOFTWARE\Microsoft\Windows NT\CurrentVersion\oem"):
        key, kind, value, size = ctypes.c_void_p(), ctypes.c_uint32(), ctypes.c_uint32(), ctypes.c_uint32(4)
        assert runtime.AnyXboxX_RegOpenKeyW(hklm, path, ctypes.byref(key)) == 0
        assert runtime.AnyXboxX_RegQueryValueExW(key, "ConsoleMode", None, ctypes.byref(kind), ctypes.byref(value),
                                                 ctypes.byref(size)) == 0
        assert (kind.value, value.value, size.value) == (4, 0x64, 4)        # REG_DWORD 0x64
        small = ctypes.c_uint32(2)
        assert runtime.AnyXboxX_RegQueryValueExW(key, "ConsoleMode", None, None, ctypes.byref(value),
                                                 ctypes.byref(small)) == 234   # ERROR_MORE_DATA
        assert small.value == 4
        assert runtime.AnyXboxX_RegCloseKey(key) == 0
    assert runtime.XSystemGetDeviceType() == XBOX_SCARLETT_ANACONDA        # 0x64 -> 7 dans xgameruntime.lib


SYSTEM = ("E349BD1A-FC20-4E40-B99C-4178CC6B409F", "1861CF2E-E18B-4834-A9F5-B4A4E6EFB4CF")
ANALYTICS = ("B884675D-B738-4A9C-815D-9A9A1E0C6C9B",) * 2
GAME = ("973A344E-24BF-4D0F-8457-56C534892B29", "2549F142-6419-4A06-97B5-931AAB7C2F34")


@anyxboxx_only
def test_system_identity_is_a_series_x(runtime_dir):
    system = GdkInterface(runtime_dir, *SYSTEM)
    buffer, used = ctypes.create_string_buffer(64), ctypes.c_size_t()
    sandbox = system.method(4, ctypes.c_uint32, ctypes.c_size_t, ctypes.c_char_p, ctypes.POINTER(ctypes.c_size_t))
    assert sandbox(16, buffer, ctypes.byref(used)) == S_OK and buffer.value == b"RETAIL"
    console = system.method(3, ctypes.c_uint32, ctypes.c_size_t, ctypes.c_char_p, ctypes.POINTER(ctypes.c_size_t))
    assert console(39, buffer, ctypes.byref(used)) == S_OK
    assert buffer.value == b"00000000.00000000.00000000.00000000.00"
    assert used.value == 39

    class Analytics(ctypes.Structure):
        _fields_ = [("os", ctypes.c_uint16 * 4), ("hosting", ctypes.c_uint16 * 4),
                    ("family", ctypes.c_char * 64), ("form", ctypes.c_char * 64)]

    info = Analytics()
    analytics = GdkInterface(runtime_dir, *ANALYTICS)
    returned = analytics.method(3, ctypes.c_void_p, ctypes.POINTER(Analytics))(ctypes.byref(info))
    assert returned == ctypes.addressof(info)
    assert (info.family, info.form, info.os[0]) == (b"Windows.Xbox", b"Xbox Series X", 10)


@anyxboxx_only
def test_title_id_comes_from_the_game_config(runtime_dir, tmp_path, monkeypatch):
    (tmp_path / "MicrosoftGame.config").write_text(
        '<?xml version="1.0"?><Game><TitleId>7952987A</TitleId></Game>', encoding="utf-8")
    monkeypatch.setenv("ANYXBOXX_GAME_DIR", str(tmp_path))
    title = ctypes.c_uint32()
    get = GdkInterface(runtime_dir, *GAME).method(3, ctypes.c_uint32, ctypes.POINTER(ctypes.c_uint32))
    assert get(ctypes.byref(title)) == S_OK and title.value == 0x7952987A
