"""Photo de profil du profil local hors ligne : un PNG généré à la taille demandée (XUser.h du GDK).

L'exemple Live/Fundamentals_Desktop de Microsoft s'arrêtait sur une exception quand
XUserGetGamerPictureAsync répondait E_NOTIMPL (Kits/LiveTK/LiveInfoHUD.cpp : DX::ThrowIfFailed).
"""
import ctypes
import struct
import zlib

import pytest

from test_runtime_cpp import (S_OK, THREAD_POOL, AsyncBlock, TaskQueueApi, add_local_user,
                              anyxboxx_only)

PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
E_INVALIDARG = 0x80070057
PICTURE_ASYNC, PICTURE_SIZE, PICTURE_RESULT = 16, 17, 18


def picture(runtime_dir, size_code):
    users, _, _, user = add_local_user(runtime_dir)
    tasks = TaskQueueApi(runtime_dir)
    queue = ctypes.c_void_p()
    tasks.method(12, ctypes.c_uint32, ctypes.c_int, ctypes.c_int, ctypes.POINTER(ctypes.c_void_p))(
        THREAD_POOL, THREAD_POOL, ctypes.byref(queue))
    block = AsyncBlock(queue=queue)
    begin = users.method(PICTURE_ASYNC, ctypes.c_uint32, ctypes.c_void_p, ctypes.c_uint32, ctypes.POINTER(AsyncBlock))
    begun = begin(user, size_code, ctypes.byref(block))
    data = None
    if begun == S_OK:
        assert tasks.method(3, ctypes.c_uint32, ctypes.POINTER(AsyncBlock), ctypes.c_bool)(ctypes.byref(block), True) == S_OK
        size = ctypes.c_size_t()
        assert users.method(PICTURE_SIZE, ctypes.c_uint32, ctypes.POINTER(AsyncBlock), ctypes.POINTER(ctypes.c_size_t))(
            ctypes.byref(block), ctypes.byref(size)) == S_OK
        buffer, used = ctypes.create_string_buffer(size.value), ctypes.c_size_t()
        result = users.method(PICTURE_RESULT, ctypes.c_uint32, ctypes.POINTER(AsyncBlock), ctypes.c_size_t,
                              ctypes.c_void_p, ctypes.POINTER(ctypes.c_size_t))
        assert result(ctypes.byref(block), size.value, buffer, ctypes.byref(used)) == S_OK
        data = buffer.raw[:used.value]
    tasks.method(17, None, ctypes.c_void_p)(queue)
    users.method(4, None, ctypes.c_void_p)(user)
    return begun, data


def png_size_and_pixels(data):
    assert data[:8] == PNG_SIGNATURE
    width, height, depth, colour = struct.unpack(">IIBB", data[16:26])
    idat_length = struct.unpack(">I", data[33:37])[0]
    assert data[37:41] == b"IDAT"
    pixels = zlib.decompress(data[41:41 + idat_length])          # vérifie aussi Adler-32
    return width, height, depth, colour, pixels


@anyxboxx_only
@pytest.mark.parametrize("size_code, side", [(0, 64), (1, 208)])
def test_local_profile_has_a_png_gamer_picture(runtime_dir, size_code, side):
    begun, data = picture(runtime_dir, size_code)
    assert begun == S_OK
    width, height, depth, colour, pixels = png_size_and_pixels(data)
    assert (width, height, depth, colour) == (side, side, 8, 2)     # RGB 8 bits
    assert len(pixels) == side * (1 + 3 * side)


@anyxboxx_only
def test_unknown_picture_size_is_refused(runtime_dir):
    begun, _ = picture(runtime_dir, 9)
    assert begun == E_INVALIDARG
