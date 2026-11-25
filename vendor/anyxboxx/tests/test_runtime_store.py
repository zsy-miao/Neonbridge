"""Runtime GDK hors ligne : XStore (Microsoft Store), appelé par n° de méthode comme le jeu.

Signatures : XStore.h du GDK public ; codes d'erreur : XGameErr.h (Microsoft Learn).
Hors ligne, la licence du jeu est locale et valide, les requêtes de produits sont vides et
tout ce qui exige la boutique échoue avec E_GAMESTORE_NETWORK_ERROR.
"""
import ctypes

import pytest

from test_runtime_cpp import (
    S_OK, THREAD_POOL, AsyncBlock, GdkInterface, TaskQueueApi, add_local_user, anyxboxx_only,
)
from test_runtime_events import E_INVALIDARG, E_NOTIMPL, EVENT_CALLBACK

pytestmark = anyxboxx_only

PROVIDER = "0DD112AC-7C24-448C-B92B-3960FB5BD30C"
VERSIONS = ("B09D803C-2414-4A05-82C6-66DFDC9E9A44", "60B09F4E-1B85-45B1-826C-169118E230E1",
            "2D42FEA5-E71D-4B76-97CD-C50AFBB3AE5D", "5C48DEDF-0B67-4492-A4B5-6829B8E796E1")
E_GAMESTORE_NETWORK_ERROR = 0x89245301
P = ctypes.c_void_p
PP = ctypes.POINTER(ctypes.c_void_p)
BLOCK = ctypes.POINTER(AsyncBlock)
PRODUCT_KIND_GAME = 0x04

CREATE_CONTEXT, CLOSE_CONTEXT = 3, 4
QUERY_ASSOCIATED, QUERY_ASSOCIATED_RESULT = 5, 6
QUERY_PRODUCTS, QUERY_PRODUCTS_RESULT = 7, 8
QUERY_CURRENT_GAME, QUERY_CURRENT_GAME_RESULT = 11, 12
ENUMERATE_QUERY, HAS_MORE_PAGES, CLOSE_QUERY = 15, 16, 19
ACQUIRE_PACKAGE_LICENSE, ACQUIRE_PACKAGE_LICENSE_RESULT = 20, 21
QUERY_GAME_LICENSE, QUERY_GAME_LICENSE_RESULT = 28, 29
SHOW_PURCHASE_UI, SHOW_PURCHASE_UI_RESULT = 49, 50
SHOW_RATE_AND_REVIEW_UI, SHOW_RATE_AND_REVIEW_UI_RESULT = 51, 52
DOWNLOAD_PACKAGES, DOWNLOAD_PACKAGES_RESULT_COUNT = 62, 63
QUERY_PACKAGE_IDENTIFIER = 65
REGISTER_LICENSE_CHANGED, UNREGISTER_LICENSE_CHANGED = 66, 67


class GameLicense(ctypes.Structure):
    _fields_ = [("sku_store_id", ctypes.c_char * 18), ("is_active", ctypes.c_bool),
                ("is_trial_owned_by_this_user", ctypes.c_bool), ("is_disc_license", ctypes.c_bool),
                ("is_trial", ctypes.c_bool), ("trial_time_remaining", ctypes.c_uint32),
                ("trial_unique_id", ctypes.c_char * 64), ("expiration_date", ctypes.c_int64)]


class Store:
    """Interface XStore plus une file XTaskQueue pour attendre les appels asynchrones."""

    def __init__(self, runtime_dir, user=None):
        self.api = GdkInterface(runtime_dir, PROVIDER, VERSIONS[0])
        self.tasks = TaskQueueApi(runtime_dir)
        self.queue = P()
        self.tasks.method(12, ctypes.c_uint32, ctypes.c_int, ctypes.c_int, PP)(
            THREAD_POOL, THREAD_POOL, ctypes.byref(self.queue))
        self.context = P()
        assert self.api.method(CREATE_CONTEXT, ctypes.c_uint32, P, PP)(user, ctypes.byref(self.context)) == S_OK

    def close(self):
        self.api.method(CLOSE_CONTEXT, None, P)(self.context)
        self.tasks.method(17, None, P)(self.queue)

    def call(self, begin_index, *argtypes_and_values, context=None):
        """Lance l'appel asynchrone n° begin_index et rend (retour du lancement, état final, bloc)."""
        argtypes = [t for t, _ in argtypes_and_values]
        values = [v for _, v in argtypes_and_values]
        block = AsyncBlock(queue=self.queue)
        begin = self.api.method(begin_index, ctypes.c_uint32, P, *argtypes, BLOCK)
        returned = begin(self.context if context is None else context, *values, ctypes.byref(block))
        if returned != S_OK:
            return returned, None, block
        status = self.tasks.method(3, ctypes.c_uint32, BLOCK, ctypes.c_bool)(ctypes.byref(block), True)
        return returned, status, block

    def result(self, index, block, *argtypes_and_values):
        function = self.api.method(index, ctypes.c_uint32, BLOCK, *[t for t, _ in argtypes_and_values])
        return function(ctypes.byref(block), *[v for _, v in argtypes_and_values])


@pytest.fixture
def store(runtime_dir):
    _, _, _, user = add_local_user(runtime_dir)
    opened = Store(runtime_dir, user)
    yield opened
    opened.close()


@pytest.mark.parametrize("version", VERSIONS)
def test_every_store_interface_version_is_served(runtime_dir, version):
    assert GdkInterface(runtime_dir, PROVIDER, version).object.value


def test_context_opens_for_the_local_user_or_for_nobody(runtime_dir):
    _, _, _, user = add_local_user(runtime_dir)
    api = GdkInterface(runtime_dir, PROVIDER, VERSIONS[0])
    create = api.method(CREATE_CONTEXT, ctypes.c_uint32, P, PP)
    for owner in (user, None):                        # utilisateur facultatif
        context = P()
        assert create(owner, ctypes.byref(context)) == S_OK and context.value
        api.method(CLOSE_CONTEXT, None, P)(context)
    assert create(user, None) == E_INVALIDARG
    unknown = ctypes.create_string_buffer(16)         # pas un handle XUser d'AnyXboxX
    assert create(ctypes.addressof(unknown), ctypes.byref(P())) == E_INVALIDARG


def test_game_license_is_local_valid_and_full(store):
    begun, status, block = store.call(QUERY_GAME_LICENSE)
    license = GameLicense(is_trial=True)
    assert (begun, status) == (S_OK, S_OK)
    assert store.result(QUERY_GAME_LICENSE_RESULT, block, (ctypes.POINTER(GameLicense), ctypes.byref(license))) == S_OK
    assert license.is_active and not license.is_trial and not license.is_disc_license
    assert not license.is_trial_owned_by_this_user and license.expiration_date == 0


def test_calls_with_a_closed_or_missing_context_are_refused(store):
    unknown = ctypes.create_string_buffer(16)
    assert store.call(QUERY_GAME_LICENSE, context=ctypes.addressof(unknown))[0] == E_INVALIDARG
    assert store.call(QUERY_GAME_LICENSE, context=0)[0] == E_INVALIDARG
    assert store.call(SHOW_PURCHASE_UI, (ctypes.c_char_p, b"9NABCDEFGHIJ"), (P, None), (P, None),
                      context=ctypes.addressof(unknown))[0] == E_INVALIDARG


@pytest.mark.parametrize("begin, finish, arguments", [
    (QUERY_CURRENT_GAME, QUERY_CURRENT_GAME_RESULT, []),
    (QUERY_ASSOCIATED, QUERY_ASSOCIATED_RESULT, [(ctypes.c_uint32, PRODUCT_KIND_GAME), (ctypes.c_uint32, 10)]),
    (QUERY_PRODUCTS, QUERY_PRODUCTS_RESULT, [(ctypes.c_uint32, PRODUCT_KIND_GAME), (P, None), (ctypes.c_size_t, 0),
                                              (P, None), (ctypes.c_size_t, 0)]),
])
def test_product_queries_return_an_empty_result_set(store, begin, finish, arguments):
    begun, status, block = store.call(begin, *arguments)
    assert (begun, status) == (S_OK, S_OK)
    query = P()
    assert store.result(finish, block, (PP, ctypes.byref(query))) == S_OK and query.value
    seen = []
    callback = ctypes.WINFUNCTYPE(ctypes.c_bool, P, P)(lambda product, context: seen.append(product) or True)
    enumerate_query = store.api.method(ENUMERATE_QUERY, ctypes.c_uint32, P, P, P)
    assert enumerate_query(query, None, ctypes.cast(callback, P)) == S_OK and seen == []
    assert store.api.method(HAS_MORE_PAGES, ctypes.c_bool, P)(query) is False
    assert enumerate_query(query, None, None) == E_INVALIDARG       # rappel obligatoire
    store.api.method(CLOSE_QUERY, None, P)(query)
    assert enumerate_query(query, None, ctypes.cast(callback, P)) == E_INVALIDARG   # handle fermé


def test_query_products_checks_its_identifier_array(store):
    refused = store.call(QUERY_PRODUCTS, (ctypes.c_uint32, PRODUCT_KIND_GAME), (P, None), (ctypes.c_size_t, 2),
                         (P, None), (ctypes.c_size_t, 0))
    assert refused[0] == E_INVALIDARG


def test_purchase_and_downloads_fail_like_an_unreachable_store(store):
    begun, status, block = store.call(SHOW_PURCHASE_UI, (ctypes.c_char_p, b"9NABCDEFGHIJ"), (P, None), (P, None))
    assert begun == S_OK and status == E_GAMESTORE_NETWORK_ERROR
    assert store.result(SHOW_PURCHASE_UI_RESULT, block) == E_GAMESTORE_NETWORK_ERROR

    begun, status, block = store.call(ACQUIRE_PACKAGE_LICENSE, (ctypes.c_char_p, b"Package_1.0_x64__abc"))
    license = P(1)
    assert begun == S_OK and status == E_GAMESTORE_NETWORK_ERROR
    assert store.result(ACQUIRE_PACKAGE_LICENSE_RESULT, block, (PP, ctypes.byref(license))) == E_GAMESTORE_NETWORK_ERROR
    assert license.value is None                                    # aucun handle de licence

    begun, status, block = store.call(DOWNLOAD_PACKAGES, (P, None), (ctypes.c_size_t, 0))
    count = ctypes.c_uint32(7)
    assert begun == S_OK and status == E_GAMESTORE_NETWORK_ERROR
    assert store.result(DOWNLOAD_PACKAGES_RESULT_COUNT, block, (ctypes.POINTER(ctypes.c_uint32), ctypes.byref(count))) \
        == E_GAMESTORE_NETWORK_ERROR and count.value == 0


def test_rate_and_review_result_writes_only_its_one_byte_bool(store):
    begun, status, block = store.call(SHOW_RATE_AND_REVIEW_UI)
    assert begun == S_OK and status == E_GAMESTORE_NETWORK_ERROR
    canary = (ctypes.c_uint8 * 4)(0xAA, 0xAA, 0xAA, 0xAA)    # XStoreRateAndReviewResult { bool wasUpdated; }
    assert store.result(SHOW_RATE_AND_REVIEW_UI_RESULT, block, (P, ctypes.addressof(canary))) \
        == E_GAMESTORE_NETWORK_ERROR
    assert list(canary) == [0, 0xAA, 0xAA, 0xAA]

def test_license_change_subscription_never_fires(store):
    register = store.api.method(REGISTER_LICENSE_CHANGED, ctypes.c_uint32, P, P, P, P, ctypes.POINTER(ctypes.c_uint64))
    unregister = store.api.method(UNREGISTER_LICENSE_CHANGED, ctypes.c_bool, P, ctypes.c_uint64, ctypes.c_bool)
    callback, token = EVENT_CALLBACK(lambda context, text: None), ctypes.c_uint64()
    assert register(store.context, None, None, ctypes.cast(callback, P), ctypes.byref(token)) == S_OK and token.value
    assert register(store.context, None, None, None, ctypes.byref(token)) == E_INVALIDARG
    assert unregister(store.context, token.value, True) is True
    assert unregister(store.context, token.value, True) is False


def test_other_store_methods_are_not_implemented(store):
    identifier = store.api.method(QUERY_PACKAGE_IDENTIFIER, ctypes.c_uint32, P, ctypes.c_size_t, P)
    assert identifier(None, 0, None) == E_NOTIMPL
