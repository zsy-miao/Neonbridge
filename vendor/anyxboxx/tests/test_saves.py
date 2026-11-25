"""Sauvegardes (XGameSave) et stockage local persistant, appelés par n° de méthode comme le jeu."""
import ctypes
import os
import sys

import pytest

from test_runtime_cpp import (
    S_OK, THREAD_POOL, AsyncBlock, GdkInterface, TaskQueueApi, add_local_user, anyxboxx_only,
)

GAME_SAVE = ("704C3F58-E629-4CC2-B197-30511B996FE2", "1BFFF3AF-F14A-40A3-8E35-9ADA906593F9")
LOCAL_STORAGE = ("F4FAF4D4-2D04-4FCE-B3E0-474A713A3E84", "41A4E10C-5A7E-41D9-8C37-37BDE62A07D6")
E_GS_INVALID_CONTAINER_NAME = 0x80830001
E_GS_PROVIDED_BUFFER_TOO_SMALL = 0x80830007
QUOTA = 256 * 1024 * 1024
P = ctypes.c_void_p
PP = ctypes.POINTER(ctypes.c_void_p)
U32P = ctypes.POINTER(ctypes.c_uint32)

pytestmark = anyxboxx_only


class SaveBlob(ctypes.Structure):
    _fields_ = [("name", ctypes.c_char_p), ("size", ctypes.c_uint32), ("data", ctypes.POINTER(ctypes.c_ubyte))]


class ContainerInfo(ctypes.Structure):
    _fields_ = [("name", ctypes.c_char_p), ("display_name", ctypes.c_char_p), ("blob_count", ctypes.c_uint32),
                ("total_size", ctypes.c_uint64), ("last_modified", ctypes.c_int64), ("needs_sync", ctypes.c_bool)]


CONTAINER_CALLBACK = (ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.POINTER(ContainerInfo), ctypes.c_void_p)
                    if sys.platform == "win32" else None)


def open_provider(saves, user):
    provider = P()
    initialize = saves.method(3, ctypes.c_uint32, P, ctypes.c_char_p, ctypes.c_bool, PP)
    assert initialize(user, b"config-test", False, ctypes.byref(provider)) == S_OK
    return provider


@pytest.fixture
def save_game(runtime_dir, tmp_path, monkeypatch):
    """Profil local, fournisseur ouvert, dossiers du jeu et des sauvegardes temporaires."""
    game = tmp_path / "jeu"
    game.mkdir()
    (game / "MicrosoftGame.config").write_text(
        "<Game><TitleId>1234ABCD</TitleId><PersistentLocalStorage><SizeMB>64</SizeMB>"
        "</PersistentLocalStorage></Game>", encoding="utf-8")
    monkeypatch.setenv("ANYXBOXX_GAME_DIR", str(game))
    monkeypatch.setenv("ANYXBOXX_SAVE_DIR", str(tmp_path / "saves"))
    _, _, _, user = add_local_user(runtime_dir)
    saves = GdkInterface(runtime_dir, *GAME_SAVE)
    provider = open_provider(saves, user)
    yield saves, provider, tmp_path / "saves", user
    saves.method(6, None, P)(provider)


def write_blobs(saves, provider, container_name, blobs, display=b"Partie"):
    container, update = P(), P()
    assert saves.method(16, ctypes.c_uint32, P, ctypes.c_char_p, PP)(provider, container_name, ctypes.byref(container)) == S_OK
    assert saves.method(23, ctypes.c_uint32, P, ctypes.c_char_p, PP)(container, display, ctypes.byref(update)) == S_OK
    write = saves.method(25, ctypes.c_uint32, P, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_size_t)
    for name, data in blobs.items():
        assert write(update, name, data, len(data)) == S_OK
    assert saves.method(27, ctypes.c_uint32, P)(update) == S_OK
    saves.method(24, None, P)(update)
    return container


def read_all(saves, container, size=4096):
    count, buffer = ctypes.c_uint32(0), ctypes.create_string_buffer(size)
    read = saves.method(20, ctypes.c_uint32, P, P, U32P, ctypes.c_size_t, P)
    result = read(container, None, ctypes.byref(count), size, buffer)
    if result != S_OK:
        return result
    blobs = ctypes.cast(buffer, ctypes.POINTER(SaveBlob))
    return {blobs[i].name: bytes(blobs[i].data[:blobs[i].size]) for i in range(count.value)}


def close(saves, container):
    saves.method(17, None, P)(container)


def test_saves_are_written_to_disk_and_read_back(save_game):
    saves, provider, folder, _ = save_game
    container = write_blobs(saves, provider, b"emplacement 1", {b"progression": b"niveau=7", b"options": b"son=on"})
    assert read_all(saves, container) == {b"progression": b"niveau=7", b"options": b"son=on"}
    assert any(path.suffix == ".blob" for path in folder.rglob("*"))
    assert "1234ABCD" in {path.name for path in folder.iterdir()}       # rangées par Title ID
    close(saves, container)


def test_containers_are_enumerated_then_deleted(save_game):
    saves, provider, _, _ = save_game
    close(saves, write_blobs(saves, provider, b"auto", {b"a": b"12345"}, display=b"Sauvegarde auto"))
    seen = []

    def remember(info, context):
        seen.append((info.contents.name, info.contents.display_name, info.contents.blob_count,
                     info.contents.total_size))
        return True

    callback = CONTAINER_CALLBACK(remember)
    enumerate_all = saves.method(14, ctypes.c_uint32, P, P, CONTAINER_CALLBACK)
    assert enumerate_all(provider, None, callback) == S_OK
    assert seen == [(b"auto", b"Sauvegarde auto", 1, 5)]
    remaining = ctypes.c_int64()
    assert saves.method(7, ctypes.c_uint32, P, ctypes.POINTER(ctypes.c_int64))(provider, ctypes.byref(remaining)) == S_OK
    assert 0 < remaining.value < QUOTA
    assert saves.method(10, ctypes.c_uint32, P, ctypes.c_char_p)(provider, b"auto") == S_OK
    seen.clear()
    enumerate_all(provider, None, callback)
    assert seen == []


def test_async_read_returns_blobs_and_their_count(save_game, runtime_dir):
    saves, provider, _, _ = save_game
    container = write_blobs(saves, provider, b"async", {b"x": b"abc"})
    tasks = TaskQueueApi(runtime_dir)
    queue = P()
    create = tasks.method(12, ctypes.c_uint32, ctypes.c_int, ctypes.c_int, PP)
    assert create(THREAD_POOL, THREAD_POOL, ctypes.byref(queue)) == S_OK
    block = AsyncBlock(queue=queue)
    begin = saves.method(21, ctypes.c_uint32, P, P, ctypes.c_uint32, ctypes.POINTER(AsyncBlock))
    assert begin(container, None, 0, ctypes.byref(block)) == S_OK
    assert tasks.method(3, ctypes.c_uint32, ctypes.POINTER(AsyncBlock), ctypes.c_bool)(ctypes.byref(block), True) == S_OK
    size = ctypes.c_size_t()
    result_size = tasks.method(4, ctypes.c_uint32, ctypes.POINTER(AsyncBlock), ctypes.POINTER(ctypes.c_size_t))
    assert result_size(ctypes.byref(block), ctypes.byref(size)) == S_OK
    buffer, count = ctypes.create_string_buffer(size.value), ctypes.c_uint32()
    result = saves.method(22, ctypes.c_uint32, ctypes.POINTER(AsyncBlock), ctypes.c_size_t, P, U32P)
    assert result(ctypes.byref(block), size.value, buffer, ctypes.byref(count)) == S_OK
    blob = ctypes.cast(buffer, ctypes.POINTER(SaveBlob))[0]
    assert (count.value, blob.name, bytes(blob.data[:blob.size])) == (1, b"x", b"abc")
    tasks.method(17, None, P)(queue)
    close(saves, container)


def test_saves_survive_reopening_the_provider(save_game):
    saves, provider, _, user = save_game
    close(saves, write_blobs(saves, provider, b"durable", {b"or": b"999"}))
    again, container = open_provider(saves, user), P()
    assert saves.method(16, ctypes.c_uint32, P, ctypes.c_char_p, PP)(again, b"durable", ctypes.byref(container)) == S_OK
    assert read_all(saves, container) == {b"or": b"999"}
    close(saves, container)
    saves.method(6, None, P)(again)


def test_invalid_name_and_small_buffer_are_refused(save_game):
    saves, provider, _, _ = save_game
    container = P()
    create = saves.method(16, ctypes.c_uint32, P, ctypes.c_char_p, PP)
    assert create(provider, b"", ctypes.byref(container)) == E_GS_INVALID_CONTAINER_NAME
    container = write_blobs(saves, provider, b"petit", {b"gros": b"x" * 100})
    assert read_all(saves, container, size=16) == E_GS_PROVIDED_BUFFER_TOO_SMALL
    close(saves, container)


def test_persistent_local_storage_uses_the_declared_size(save_game, runtime_dir):
    storage = GdkInterface(runtime_dir, *LOCAL_STORAGE)
    path, used = ctypes.create_string_buffer(512), ctypes.c_size_t()
    get_path = storage.method(4, ctypes.c_uint32, ctypes.c_size_t, ctypes.c_char_p, ctypes.POINTER(ctypes.c_size_t))
    assert get_path(512, path, ctypes.byref(used)) == S_OK
    folder = path.value.decode()
    assert folder.endswith("\\") and os.path.isdir(folder) and "1234ABCD" in folder
    info = (ctypes.c_uint64 * 4)()
    assert storage.method(5, ctypes.c_uint32, P)(info) == S_OK
    assert info[3] == 64 * 1024 * 1024                              # <SizeMB>64</SizeMB>
