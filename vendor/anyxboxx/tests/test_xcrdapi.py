"""XCRDAPI.dll : DStorageGetFactory minimal (note de premier démarrage, W7).

Le titre demande une factory DirectStorage au premier usage d'un fichier ; sans elle, il s'arrête sur un
int3. La factory rend S_OK pour son IID et pour IUnknown, et suit la disposition console (IDStorageFactoryX,
TomBox crates/compat/xcrdapi/src/objects.rs). Le slot 8 (+0x40, SetStagingBufferSize) rend un HRESULT
positif que le titre teste ; aucun tampon de transit n'existe, donc la taille est journalisée et ignorée.
CreateQueue, OpenFile et CreateStatusArray rendent E_NOTIMPL, sortie mise à NULL.
"""
import ctypes
import sys

import pytest

pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="DLL Windows x64")

S_OK, E_NOTIMPL, E_NOINTERFACE, E_POINTER = 0, 0x8000_4001, 0x8000_4002, 0x8000_4003
SLOT_CREATE_QUEUE, SLOT_OPEN_FILE, SLOT_CREATE_STATUS_ARRAY, SLOT_SET_STAGING_BUFFER_SIZE = 3, 4, 5, 8
IID_FACTORY_X = bytes([0xac, 0x54, 0x3b, 0xfb, 0x82, 0xbf, 0x3a, 0x46,
                       0xb7, 0x77, 0x73, 0x19, 0x58, 0x37, 0x86, 0x4d])
IID_IUNKNOWN = bytes([0, 0, 0, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46])
IID_OTHER = bytes(range(16))


def hr(value):
    return value & 0xFFFF_FFFF


@pytest.fixture
def xcrdapi(runtime_dir):
    dll = ctypes.WinDLL(str(runtime_dir / "XCRDAPI.dll"))
    dll.DStorageGetFactory.restype = ctypes.c_uint32
    dll.DStorageGetFactory.argtypes = (ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p))
    return dll


def get_factory(xcrdapi, iid=IID_FACTORY_X):
    riid = ctypes.create_string_buffer(iid, 16)
    out = ctypes.c_void_p(0x1234)
    result = xcrdapi.DStorageGetFactory(ctypes.addressof(riid), ctypes.byref(out))
    return result, out.value


def method(factory, slot, restype, *argtypes):
    """Appelle le slot `slot` de la table virtuelle de l'objet (premier mot = pointeur de table)."""
    vtable = ctypes.cast(factory, ctypes.POINTER(ctypes.c_void_p))[0]
    entry = ctypes.cast(vtable, ctypes.POINTER(ctypes.c_void_p))[slot]
    function = ctypes.CFUNCTYPE(restype, ctypes.c_void_p, *argtypes)(entry)
    return lambda *args: function(factory, *args)


def test_null_output_pointer_is_refused(xcrdapi):
    riid = ctypes.create_string_buffer(IID_FACTORY_X, 16)
    assert hr(xcrdapi.DStorageGetFactory(ctypes.addressof(riid), None)) == E_POINTER


def test_factory_is_returned_for_the_console_iid(xcrdapi):
    result, factory = get_factory(xcrdapi)
    assert hr(result) == S_OK
    assert factory

def test_factory_answers_iunknown(xcrdapi):
    result, factory = get_factory(xcrdapi, IID_IUNKNOWN)
    assert hr(result) == S_OK
    assert factory


def test_unknown_iid_is_refused_and_output_is_null(xcrdapi):
    result, factory = get_factory(xcrdapi, IID_OTHER)
    assert hr(result) == E_NOINTERFACE
    assert factory is None


def test_slot_8_returns_a_non_negative_hresult(xcrdapi):
    _, factory = get_factory(xcrdapi)
    set_staging = method(factory, SLOT_SET_STAGING_BUFFER_SIZE, ctypes.c_int32, ctypes.c_uint32)
    assert set_staging(0x200_0000) >= 0


def test_create_queue_returns_e_notimpl_and_null_output(xcrdapi):
    _, factory = get_factory(xcrdapi)
    out = ctypes.c_void_p(0x1234)
    riid = ctypes.create_string_buffer(IID_FACTORY_X, 16)
    create = method(factory, SLOT_CREATE_QUEUE, ctypes.c_uint32, ctypes.c_void_p, ctypes.c_void_p,
                    ctypes.c_void_p)
    assert hr(create(None, ctypes.addressof(riid), ctypes.byref(out))) == E_NOTIMPL
    assert out.value is None


def test_open_file_returns_e_notimpl_and_null_output(xcrdapi):
    _, factory = get_factory(xcrdapi)
    out = ctypes.c_void_p(0x1234)
    riid = ctypes.create_string_buffer(IID_FACTORY_X, 16)
    open_file = method(factory, SLOT_OPEN_FILE, ctypes.c_uint32, ctypes.c_wchar_p, ctypes.c_void_p,
                       ctypes.c_void_p)
    assert hr(open_file("C:/jeu.bin", ctypes.addressof(riid), ctypes.byref(out))) == E_NOTIMPL
    assert out.value is None


def test_create_status_array_returns_e_notimpl_and_null_output(xcrdapi):
    _, factory = get_factory(xcrdapi)
    out = ctypes.c_void_p(0x1234)
    riid = ctypes.create_string_buffer(IID_FACTORY_X, 16)
    create = method(factory, SLOT_CREATE_STATUS_ARRAY, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_char_p,
                    ctypes.c_void_p, ctypes.c_void_p)
    assert hr(create(1, None, ctypes.addressof(riid), ctypes.byref(out))) == E_NOTIMPL
    assert out.value is None
