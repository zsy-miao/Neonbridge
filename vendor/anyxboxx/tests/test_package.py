"""XPackage : le jeu est un paquet installé en local, appelé par n° de méthode comme le jeu."""
import ctypes
import os
import sys

import pytest

from test_runtime_cpp import S_OK, THREAD_POOL, AsyncBlock, GdkInterface, TaskQueueApi, anyxboxx_only

PACKAGE = ("AF406016-E850-4AA8-A88D-2F3DCB9DAC7E", "0B9D556E-E9C4-4F3B-A5D6-5A2CBAB7BB2A")
P = ctypes.c_void_p
PP = ctypes.POINTER(ctypes.c_void_p)

pytestmark = anyxboxx_only


class Details(ctypes.Structure):
    _fields_ = [("identifier", ctypes.c_char_p), ("version", ctypes.c_uint16 * 4), ("kind", ctypes.c_uint32),
                ("display_name", ctypes.c_char_p), ("description", ctypes.c_char_p), ("publisher", ctypes.c_char_p),
                ("store_id", ctypes.c_char_p), ("installing", ctypes.c_bool), ("index", ctypes.c_uint32),
                ("count", ctypes.c_uint32), ("age_restricted", ctypes.c_bool), ("title_id", ctypes.c_char_p)]


ENUMERATION = (ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.POINTER(Details))
              if sys.platform == "win32" else None)


@pytest.fixture
def package(runtime_dir, tmp_path, monkeypatch):
    (tmp_path / "MicrosoftGame.config").write_text(
        '<Game><TitleId>0000ABCD</TitleId><Identity Name="Studio.JeuDeTest" Publisher="CN=Studio" '
        'Version="1.2.3.4" /><ShellVisuals DefaultDisplayName="Jeu de test" /><StoreId>9ABCDEF</StoreId></Game>',
        encoding="utf-8")
    monkeypatch.setenv("ANYXBOXX_GAME_DIR", str(tmp_path))
    real = PACKAGE_GUIDS(runtime_dir)
    return GdkInterface(runtime_dir, *real), tmp_path


def PACKAGE_GUIDS(runtime_dir):
    """Couple (fournisseur, interface) d'XPackage relu dans la table générée."""
    table = (runtime_dir.parents[1] / "xgameruntime" / "gdk_api_table.inc").read_text(encoding="utf-8")
    line = next(line for line in table.splitlines() if "(XPackageIsPackagedProcess," in line)
    return line.split('"')[1], line.split('"')[3]


def test_identity_comes_from_the_game_config(package):
    api, _ = package
    buffer = ctypes.create_string_buffer(33)
    assert api.method(3, ctypes.c_uint32, ctypes.c_size_t, ctypes.c_char_p)(33, buffer) == S_OK
    assert buffer.value == b"Studio.JeuDeTest"
    assert api.method(4, ctypes.c_bool)()                                   # XPackageIsPackagedProcess
    locale = ctypes.create_string_buffer(85)
    assert api.method(11, ctypes.c_uint32, ctypes.c_size_t, ctypes.c_char_p)(85, locale) == S_OK
    assert b"-" in locale.value                                             # ex. fr-FR


def test_every_chunk_is_ready_and_installed(package):
    api, _ = package
    availability = ctypes.c_uint32(9)
    find = api.method(12, ctypes.c_uint32, ctypes.c_char_p, ctypes.c_uint32, P, ctypes.POINTER(ctypes.c_uint32))
    assert find(b"Studio.JeuDeTest", 0, None, ctypes.byref(availability)) == S_OK and availability.value == 0
    monitor = P()
    create = api.method(5, ctypes.c_uint32, ctypes.c_char_p, ctypes.c_uint32, P, ctypes.c_uint32, P, PP)
    assert create(b"Studio.JeuDeTest", 0, None, 0, None, ctypes.byref(monitor)) == S_OK
    progress = (ctypes.c_uint64 * 4)()
    api.method(7, None, P, P)(monitor, progress)
    completed = ctypes.cast(ctypes.addressof(progress) + 25, ctypes.POINTER(ctypes.c_bool))[0]
    assert completed                                                         # installation terminée
    api.method(6, None, P)(monitor)


def test_game_package_is_enumerated_with_its_details(package):
    api, _ = package
    seen = []
    callback = ENUMERATION(lambda context, details: seen.append(
        (details.contents.identifier, details.contents.display_name, list(details.contents.version),
         details.contents.store_id, details.contents.title_id)) or True)
    assert api.method(39, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_uint32, P, ENUMERATION)(0, 0, None, callback) == S_OK
    assert seen == [(b"Studio.JeuDeTest", b"Jeu de test", [1, 2, 3, 4], b"9ABCDEF", b"0000ABCD")]


def test_mounting_a_package_gives_the_game_folder(package, runtime_dir):
    api, folder = package
    tasks = TaskQueueApi(runtime_dir)
    queue = P()
    assert tasks.method(12, ctypes.c_uint32, ctypes.c_int, ctypes.c_int, PP)(THREAD_POOL, THREAD_POOL, ctypes.byref(queue)) == S_OK
    block = AsyncBlock(queue=queue)
    assert api.method(37, ctypes.c_uint32, ctypes.c_char_p, ctypes.POINTER(AsyncBlock))(b"Studio.JeuDeTest", ctypes.byref(block)) == S_OK
    assert tasks.method(3, ctypes.c_uint32, ctypes.POINTER(AsyncBlock), ctypes.c_bool)(ctypes.byref(block), True) == S_OK
    mount = P()
    assert api.method(38, ctypes.c_uint32, ctypes.POINTER(AsyncBlock), PP)(ctypes.byref(block), ctypes.byref(mount)) == S_OK
    size = ctypes.c_size_t()
    assert api.method(24, ctypes.c_uint32, P, ctypes.POINTER(ctypes.c_size_t))(mount, ctypes.byref(size)) == S_OK
    path = ctypes.create_string_buffer(size.value)
    assert api.method(25, ctypes.c_uint32, P, ctypes.c_size_t, ctypes.c_char_p)(mount, size.value, path) == S_OK
    assert os.path.samefile(path.value.decode(), folder)
    api.method(26, None, P)(mount)
    tasks.method(17, None, P)(queue)
