"""Runtime GDK hors ligne : abonnements aux invitations, activations et protocoles, et XError.

Signatures : XGameInvite.h, XGameActivation.h, XGameProtocol.h et XError.h du GDK (Microsoft Learn).
Hors ligne, aucune invitation ni activation n'arrive : l'abonnement réussit, le rappel ne part jamais.
"""
import ctypes
import sys

import pytest

from test_runtime_cpp import S_OK, GdkInterface

E_NOTIMPL, E_INVALIDARG = 0x80004001, 0x80070057
INVITE = ("0651AAE2-4012-4077-BF84-8B9097090E2C",) * 2
PENDING_INVITE = ("0651AAE2-4012-4077-BF84-8B9097090E2C", "014D1CC3-BCFE-41FF-B2F0-E1EF07155828")
ACTIVATION = ("7F0FE8B8-E075-49AB-9AA7-A1E065489A9E", "2E4F76FE-0FC7-461E-AB4D-A4499434C3CF")
PROTOCOL = ("95FD18D2-74DD-4D7C-AA1B-0B51827665D6", "026B010C-06C3-4CDD-BBCB-43F229DB1CFF")
ERROR = ("8CA467F7-22E8-4096-8456-BB8AA13F79D8",) * 2
EVENT_CALLBACK = (ctypes.WINFUNCTYPE(None, ctypes.c_void_p, ctypes.c_char_p)
                if sys.platform == "win32" else None)
ERROR_CALLBACK = (ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_int32, ctypes.c_char_p, ctypes.c_void_p)
                 if sys.platform == "win32" else None)


def subscription(runtime_dir, interface, register, unregister):
    api = GdkInterface(runtime_dir, *interface)
    return (api.method(register, ctypes.c_uint32, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p,
                       ctypes.POINTER(ctypes.c_uint64)),
            api.method(unregister, ctypes.c_bool, ctypes.c_uint64, ctypes.c_bool))


@pytest.mark.parametrize("interface, register, unregister", [
    (INVITE, 3, 4), (PENDING_INVITE, 5, 6), (ACTIVATION, 3, 4), (PROTOCOL, 3, 4)])
def test_offline_subscriptions_register_once(runtime_dir, interface, register, unregister):
    subscribe, unsubscribe = subscription(runtime_dir, interface, register, unregister)
    callback = EVENT_CALLBACK(lambda context, uri: None)
    first, second = ctypes.c_uint64(), ctypes.c_uint64()
    assert subscribe(None, None, ctypes.cast(callback, ctypes.c_void_p), ctypes.byref(first)) == S_OK
    assert subscribe(None, None, ctypes.cast(callback, ctypes.c_void_p), ctypes.byref(second)) == S_OK
    assert first.value != 0 and second.value != first.value
    assert unsubscribe(first.value, True) is True
    assert unsubscribe(first.value, True) is False                  # déjà désabonné
    assert unsubscribe(second.value, False) is True


@pytest.mark.parametrize("interface, register, unregister", [(INVITE, 3, 4), (PROTOCOL, 3, 4)])
def test_subscription_without_callback_or_token_is_refused(runtime_dir, interface, register, unregister):
    subscribe, _ = subscription(runtime_dir, interface, register, unregister)
    callback = EVENT_CALLBACK(lambda context, uri: None)
    token = ctypes.c_uint64()
    assert subscribe(None, None, None, ctypes.byref(token)) == E_INVALIDARG
    assert subscribe(None, None, ctypes.cast(callback, ctypes.c_void_p), None) == E_INVALIDARG


@pytest.mark.parametrize("interface, index", [(PENDING_INVITE, 7), (ACTIVATION, 5)])
def test_accepting_an_invite_is_not_implemented(runtime_dir, interface, index):
    accept = GdkInterface(runtime_dir, *interface).method(index, ctypes.c_uint32, ctypes.c_void_p)
    assert accept(None) == E_NOTIMPL


def test_xerror_report_reaches_the_title_callback(runtime_dir):
    api = GdkInterface(runtime_dir, *ERROR)
    report = api.method(3, ctypes.c_uint32, ctypes.c_int32, ctypes.c_char_p)   # HRESULT (xgameruntime-docs)
    set_callback = api.method(4, None, ctypes.c_void_p, ctypes.c_void_p)
    seen = []
    callback = ERROR_CALLBACK(lambda hr, message, context: seen.append((hr, message, context)) or True)
    set_callback(ctypes.cast(callback, ctypes.c_void_p), ctypes.c_void_p(0x1234))
    assert report(ctypes.c_int32(E_INVALIDARG - (1 << 32)).value, b"parametre invalide") == S_OK
    set_callback(None, None)
    report(0, b"sans rappel")
    assert seen == [(E_INVALIDARG - (1 << 32), b"parametre invalide", 0x1234)]
