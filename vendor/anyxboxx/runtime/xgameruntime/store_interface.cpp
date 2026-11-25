// Interface XStore du runtime GDK servie par AnyXboxX : le Microsoft Store, hors ligne.
//
// Fournisseur {0DD112AC…}, quatre versions d'interface qui prolongent la même table
// (méthodes 3 à 83, gdk_api_table.inc). Signatures et énumérations : en-tête public
// XStore.h et codes d'erreur de XGameErr.h du GDK (Microsoft Learn).
//
// Sans Xbox Live ni boutique, AnyXboxX ne vend rien et ne contacte aucun serveur :
//  - un contexte (XStoreCreateContext) se crée et se ferme normalement ;
//  - la licence du jeu est locale : valide et complète (isActive vrai, ni essai ni disque).
//    C'est un choix assumé : un PC qui lance le jeu avec AnyXboxX n'a pas de licence
//    Store à vérifier, et la refuser empêcherait tout démarrage ;
//  - les requêtes de produits rendent un jeu de résultats vide (aucun produit connu) ;
//  - achat, fenêtres du Store, téléchargements et licences de paquet à acquérir échouent
//    avec E_GAMESTORE_NETWORK_ERROR, l'erreur de la console quand la boutique est injoignable ;
//  - l'abonnement au changement de licence réussit, le rappel ne part jamais ;
//  - tout le reste renvoie E_NOTIMPL et laisse une trace.
#include <windows.h>

#include <cstdint>
#include <cstring>
#include <ctime>
#include <new>
#include <utility>

#include "XAsync.h"
#include "async_call.h"
#include "interface_helpers.h"
#include "interfaces.h"
#include "trace.h"

namespace {

// --- Types publics de XStore.h, au strict nécessaire ----------------------------------
constexpr size_t kStoreSkuIdSize = 18;     // STORE_SKU_ID_SIZE
constexpr size_t kTrialUniqueIdSize = 64;  // TRIAL_UNIQUE_ID_MAX_SIZE
constexpr HRESULT E_GAMESTORE_NETWORK_ERROR = static_cast<HRESULT>(0x89245301L);

struct XStoreGameLicense {
  char sku_store_id[kStoreSkuIdSize];
  bool is_active;
  bool is_trial_owned_by_this_user;
  bool is_disc_license;
  bool is_trial;
  uint32_t trial_time_remaining_in_seconds;
  char trial_unique_id[kTrialUniqueIdSize];
  time_t expiration_date;
};

constexpr uint32_t kContextMagic = 0x31535841;  // « AXS1 »
constexpr uint32_t kQueryMagic = 0x32535841;    // « AXS2 »

struct Handle {
  uint32_t magic;
};
using XStoreContextHandle = Handle*;
using XStoreProductQueryHandle = Handle*;

// XTaskQueueRegistrationToken : structure d'un seul uint64_t, passée par valeur.
struct RegistrationToken {
  uint64_t token;
};

bool Valid(const Handle* handle, uint32_t magic) { return handle != nullptr && handle->magic == magic; }

Handle* NewHandle(uint32_t magic) { return new (std::nothrow) Handle{magic}; }

void Destroy(Handle* handle, uint32_t magic) {
  if (!Valid(handle, magic)) return;
  handle->magic = 0;
  delete handle;
}

// Identités des résultats : XAsyncGetResult vérifie qu'on lit ce que l'appel a produit.
const char kLicenseIdentity = 0;
const char kQueryIdentity = 0;
const char kOfflineIdentity = 0;

// --- Contexte --------------------------------------------------------------------------
HRESULT XStoreCreateContext(void* user, XStoreContextHandle* context) noexcept {
  uint64_t xuid = 0;
  if (context == nullptr || (user != nullptr && !anyxboxx::LocalUserId(user, &xuid))) return E_INVALIDARG;
  *context = NewHandle(kContextMagic);
  return *context != nullptr ? S_OK : E_OUTOFMEMORY;
}

void XStoreCloseContextHandle(XStoreContextHandle context) noexcept { Destroy(context, kContextMagic); }

// --- Licence du jeu : locale, valide ---------------------------------------------------
HRESULT XStoreQueryGameLicenseAsync(XStoreContextHandle context, XAsyncBlock* async) noexcept {
  if (!Valid(context, kContextMagic)) return E_INVALIDARG;
  anyxboxx::AsyncCall call;
  call.size = sizeof(XStoreGameLicense);
  call.write = [](void* buffer) {
    XStoreGameLicense license{};
    license.is_active = true;  // licence complète, ni essai ni disque, sans expiration
    std::memcpy(buffer, &license, sizeof license);
  };
  return anyxboxx::BeginAsyncCall(async, &kLicenseIdentity, "XStoreQueryGameLicenseAsync", std::move(call));
}

HRESULT XStoreQueryGameLicenseResult(XAsyncBlock* async, XStoreGameLicense* license) noexcept {
  return XAsyncGetResult(async, &kLicenseIdentity, sizeof(XStoreGameLicense), license, nullptr);
}

// --- Requêtes de produits : jeu de résultats vide --------------------------------------
HRESULT BeginEmptyQuery(XStoreContextHandle context, XAsyncBlock* async, const char* name) {
  if (!Valid(context, kContextMagic)) return E_INVALIDARG;
  Handle* query = NewHandle(kQueryMagic);
  if (query == nullptr) return E_OUTOFMEMORY;
  anyxboxx::AsyncCall call;
  call.size = sizeof(void*);
  call.write = [query](void* buffer) { std::memcpy(buffer, &query, sizeof query); };
  call.discard = [query] { Destroy(query, kQueryMagic); };
  return anyxboxx::BeginAsyncCall(async, &kQueryIdentity, name, std::move(call));
}

HRESULT XStoreQueryAssociatedProductsAsync(XStoreContextHandle context, uint32_t, uint32_t,
                                           XAsyncBlock* async) noexcept {
  return BeginEmptyQuery(context, async, "XStoreQueryAssociatedProductsAsync");
}

HRESULT XStoreQueryProductsAsync(XStoreContextHandle context, uint32_t, const char** ids, size_t id_count,
                                 const char**, size_t, XAsyncBlock* async) noexcept {
  if (ids == nullptr && id_count > 0) return E_INVALIDARG;
  return BeginEmptyQuery(context, async, "XStoreQueryProductsAsync");
}

HRESULT XStoreQueryEntitledProductsAsync(XStoreContextHandle context, uint32_t, uint32_t,
                                         XAsyncBlock* async) noexcept {
  return BeginEmptyQuery(context, async, "XStoreQueryEntitledProductsAsync");
}

HRESULT XStoreQueryProductForCurrentGameAsync(XStoreContextHandle context, XAsyncBlock* async) noexcept {
  return BeginEmptyQuery(context, async, "XStoreQueryProductForCurrentGameAsync");
}

HRESULT XStoreQueryProductForPackageAsync(XStoreContextHandle context, uint32_t, const char* package,
                                          XAsyncBlock* async) noexcept {
  if (package == nullptr) return E_INVALIDARG;
  return BeginEmptyQuery(context, async, "XStoreQueryProductForPackageAsync");
}

HRESULT XStoreQueryAssociatedProductsForStoreIdAsync(XStoreContextHandle context, const char* store_id,
                                                     uint32_t, uint32_t, XAsyncBlock* async) noexcept {
  if (store_id == nullptr) return E_INVALIDARG;
  return BeginEmptyQuery(context, async, "XStoreQueryAssociatedProductsForStoreIdAsync");
}

// Résultat commun des six requêtes : un handle de requête vide.
HRESULT XStoreQueryResult(XAsyncBlock* async, XStoreProductQueryHandle* query) noexcept {
  return XAsyncGetResult(async, &kQueryIdentity, sizeof(void*), query, nullptr);
}

HRESULT XStoreEnumerateProductsQuery(XStoreProductQueryHandle query, void*, void* callback) noexcept {
  if (!Valid(query, kQueryMagic) || callback == nullptr) return E_INVALIDARG;
  return S_OK;  // aucun produit : le rappel n'est jamais appelé
}

bool XStoreProductsQueryHasMorePages(XStoreProductQueryHandle) noexcept { return false; }

void XStoreCloseProductsQueryHandle(XStoreProductQueryHandle query) noexcept { Destroy(query, kQueryMagic); }

// --- Appels qui exigent la boutique : échec réseau de la console -----------------------
HRESULT BeginOffline(XStoreContextHandle context, XAsyncBlock* async, const char* name) {
  if (!Valid(context, kContextMagic)) return E_INVALIDARG;
  anyxboxx::AsyncCall call;
  call.result = E_GAMESTORE_NETWORK_ERROR;
  return anyxboxx::BeginAsyncCall(async, &kOfflineIdentity, name, std::move(call));
}

// Résultats sans sortie, ou dont la sortie n'existe que si l'appel a réussi : on rend l'état.
HRESULT XStoreOfflineResult(XAsyncBlock* async) noexcept { return XAsyncGetStatus(async, false); }

HRESULT XStoreLicenseResult(XAsyncBlock* async, void** license) noexcept {
  if (license == nullptr) return E_INVALIDARG;
  *license = nullptr;
  return XAsyncGetStatus(async, false);
}

// XStoreRateAndReviewResult { bool wasUpdated; } : un seul octet à écrire.
HRESULT XStoreRateAndReviewResult(XAsyncBlock* async, bool* was_updated) noexcept {
  if (was_updated == nullptr) return E_INVALIDARG;
  *was_updated = false;
  return XAsyncGetStatus(async, false);
}

HRESULT XStoreInstallCountResult(XAsyncBlock* async, uint32_t* count) noexcept {
  if (count == nullptr) return E_INVALIDARG;
  *count = 0;
  return XAsyncGetStatus(async, false);
}

HRESULT XStoreInstallResult(XAsyncBlock* async, uint32_t, void*) noexcept { return XAsyncGetStatus(async, false); }

HRESULT XStoreAcquireLicenseForPackageAsync(XStoreContextHandle context, const char*, XAsyncBlock* async) noexcept {
  return BeginOffline(context, async, "XStoreAcquireLicenseForPackageAsync");
}

HRESULT XStoreAcquireLicenseForDurablesAsync(XStoreContextHandle context, const char*, XAsyncBlock* async) noexcept {
  return BeginOffline(context, async, "XStoreAcquireLicenseForDurablesAsync");
}

HRESULT XStoreShowProductPageUIAsync(XStoreContextHandle context, const char*, XAsyncBlock* async) noexcept {
  return BeginOffline(context, async, "XStoreShowProductPageUIAsync");
}

HRESULT XStoreShowAssociatedProductsUIAsync(XStoreContextHandle context, const char*, uint32_t, XAsyncBlock* async) noexcept {
  return BeginOffline(context, async, "XStoreShowAssociatedProductsUIAsync");
}

HRESULT XStoreShowPurchaseUIAsync(XStoreContextHandle context, const char*, const char*, const char*, XAsyncBlock* async) noexcept {
  return BeginOffline(context, async, "XStoreShowPurchaseUIAsync");
}

HRESULT XStoreShowGiftingUIAsync(XStoreContextHandle context, const char*, const char*, const char*, XAsyncBlock* async) noexcept {
  return BeginOffline(context, async, "XStoreShowGiftingUIAsync");
}

HRESULT XStoreShowRateAndReviewUIAsync(XStoreContextHandle context, XAsyncBlock* async) noexcept {
  return BeginOffline(context, async, "XStoreShowRateAndReviewUIAsync");
}

HRESULT XStoreShowRedeemTokenUIAsync(XStoreContextHandle context, const char*, const char**, size_t, bool, XAsyncBlock* async) noexcept {
  return BeginOffline(context, async, "XStoreShowRedeemTokenUIAsync");
}

HRESULT XStoreDownloadPackageUpdatesAsync(XStoreContextHandle context, const char**, size_t, XAsyncBlock* async) noexcept {
  return BeginOffline(context, async, "XStoreDownloadPackageUpdatesAsync");
}

HRESULT XStoreDownloadAndInstallPackageUpdatesAsync(XStoreContextHandle context, const char**, size_t, XAsyncBlock* async) noexcept {
  return BeginOffline(context, async, "XStoreDownloadAndInstallPackageUpdatesAsync");
}

HRESULT XStoreDownloadAndInstallPackagesAsync(XStoreContextHandle context, const char**, size_t, XAsyncBlock* async) noexcept {
  return BeginOffline(context, async, "XStoreDownloadAndInstallPackagesAsync");
}

// --- Changement de licence : abonnement qui ne se déclenche jamais ---------------------
HRESULT XStoreRegisterGameLicenseChanged(XStoreContextHandle context, void*, void*, void* callback,
                                         void* token) noexcept {
  if (!Valid(context, kContextMagic)) return E_INVALIDARG;
  return anyxboxx::AddSubscription(callback, token);
}

bool XStoreUnregisterGameLicenseChanged(XStoreContextHandle context, RegistrationToken token, bool) noexcept {
  return Valid(context, kContextMagic) && anyxboxx::RemoveSubscription(token.token);
}

void* const kMethods[] = {
    ANYXBOXX_IUNKNOWN_METHODS,  // 0-2
    ANYXBOXX_METHOD(XStoreCreateContext),  // 3
    ANYXBOXX_METHOD(XStoreCloseContextHandle),  // 4
    ANYXBOXX_METHOD(XStoreQueryAssociatedProductsAsync),  // 5
    ANYXBOXX_METHOD(XStoreQueryResult),  // 6 XStoreQueryAssociatedProductsResult
    ANYXBOXX_METHOD(XStoreQueryProductsAsync),  // 7
    ANYXBOXX_METHOD(XStoreQueryResult),  // 8 XStoreQueryProductsResult
    ANYXBOXX_METHOD(XStoreQueryEntitledProductsAsync),  // 9
    ANYXBOXX_METHOD(XStoreQueryResult),  // 10 XStoreQueryEntitledProductsResult
    ANYXBOXX_METHOD(XStoreQueryProductForCurrentGameAsync),  // 11
    ANYXBOXX_METHOD(XStoreQueryResult),  // 12 XStoreQueryProductForCurrentGameResult
    ANYXBOXX_METHOD(XStoreQueryProductForPackageAsync),  // 13
    ANYXBOXX_METHOD(XStoreQueryResult),  // 14 XStoreQueryProductForPackageResult
    ANYXBOXX_METHOD(XStoreEnumerateProductsQuery),  // 15
    ANYXBOXX_METHOD(XStoreProductsQueryHasMorePages),  // 16
    ANYXBOXX_NOT_IMPLEMENTED,  // 17 XStoreProductsQueryNextPageAsync
    ANYXBOXX_NOT_IMPLEMENTED,  // 18 XStoreProductsQueryNextPageResult
    ANYXBOXX_METHOD(XStoreCloseProductsQueryHandle),  // 19
    ANYXBOXX_METHOD(XStoreAcquireLicenseForPackageAsync),  // 20
    ANYXBOXX_METHOD(XStoreLicenseResult),  // 21 XStoreAcquireLicenseForPackageResult
    ANYXBOXX_NOT_IMPLEMENTED,  // 22 XStoreIsLicenseValid
    ANYXBOXX_NOT_IMPLEMENTED,  // 23 XStoreCloseLicenseHandle
    ANYXBOXX_NOT_IMPLEMENTED,  // 24 XStoreCanAcquireLicenseForStoreIdAsync
    ANYXBOXX_NOT_IMPLEMENTED,  // 25 XStoreCanAcquireLicenseForStoreIdResult
    ANYXBOXX_NOT_IMPLEMENTED,  // 26 XStoreCanAcquireLicenseForPackageAsync
    ANYXBOXX_NOT_IMPLEMENTED,  // 27 XStoreCanAcquireLicenseForPackageResult
    ANYXBOXX_METHOD(XStoreQueryGameLicenseAsync),  // 28
    ANYXBOXX_METHOD(XStoreQueryGameLicenseResult),  // 29
    ANYXBOXX_NOT_IMPLEMENTED,  // 30 XStoreQueryAddOnLicensesAsync
    ANYXBOXX_NOT_IMPLEMENTED,  // 31 XStoreQueryAddOnLicensesResultCount
    ANYXBOXX_NOT_IMPLEMENTED,  // 32 XStoreQueryAddOnLicensesResult
    ANYXBOXX_NOT_IMPLEMENTED,  // 33 XStoreQueryConsumableBalanceRemainingAsync
    ANYXBOXX_NOT_IMPLEMENTED,  // 34 XStoreQueryConsumableBalanceRemainingResult
    ANYXBOXX_NOT_IMPLEMENTED,  // 35 XStoreReportConsumableFulfillmentAsync
    ANYXBOXX_NOT_IMPLEMENTED,  // 36 XStoreReportConsumableFulfillmentResult
    ANYXBOXX_NOT_IMPLEMENTED,  // 37 XStoreGetUserCollectionsIdAsync
    ANYXBOXX_NOT_IMPLEMENTED,  // 38 XStoreGetUserCollectionsIdResultSize
    ANYXBOXX_NOT_IMPLEMENTED,  // 39 XStoreGetUserCollectionsIdResult
    ANYXBOXX_NOT_IMPLEMENTED,  // 40 XStoreGetUserPurchaseIdAsync
    ANYXBOXX_NOT_IMPLEMENTED,  // 41 XStoreGetUserPurchaseIdResultSize
    ANYXBOXX_NOT_IMPLEMENTED,  // 42 XStoreGetUserPurchaseIdResult
    ANYXBOXX_NOT_IMPLEMENTED,  // 43 XStoreQueryLicenseTokenAsync
    ANYXBOXX_NOT_IMPLEMENTED,  // 44 XStoreQueryLicenseTokenResultSize
    ANYXBOXX_NOT_IMPLEMENTED,  // 45 XStoreQueryLicenseTokenResult
    ANYXBOXX_NOT_IMPLEMENTED,  // 46 : aucune fonction publique
    ANYXBOXX_NOT_IMPLEMENTED,  // 47 : aucune fonction publique
    ANYXBOXX_NOT_IMPLEMENTED,  // 48 : aucune fonction publique
    ANYXBOXX_METHOD(XStoreShowPurchaseUIAsync),  // 49
    ANYXBOXX_METHOD(XStoreOfflineResult),  // 50 XStoreShowPurchaseUIResult
    ANYXBOXX_METHOD(XStoreShowRateAndReviewUIAsync),  // 51
    ANYXBOXX_METHOD(XStoreRateAndReviewResult),  // 52 XStoreShowRateAndReviewUIResult
    ANYXBOXX_METHOD(XStoreShowRedeemTokenUIAsync),  // 53
    ANYXBOXX_METHOD(XStoreOfflineResult),  // 54 XStoreShowRedeemTokenUIResult
    ANYXBOXX_NOT_IMPLEMENTED,  // 55 XStoreQueryGameAndDlcPackageUpdatesAsync
    ANYXBOXX_NOT_IMPLEMENTED,  // 56 XStoreQueryGameAndDlcPackageUpdatesResultCount
    ANYXBOXX_NOT_IMPLEMENTED,  // 57 XStoreQueryGameAndDlcPackageUpdatesResult
    ANYXBOXX_METHOD(XStoreDownloadPackageUpdatesAsync),  // 58
    ANYXBOXX_METHOD(XStoreOfflineResult),  // 59 XStoreDownloadPackageUpdatesResult
    ANYXBOXX_METHOD(XStoreDownloadAndInstallPackageUpdatesAsync),  // 60
    ANYXBOXX_METHOD(XStoreOfflineResult),  // 61 XStoreDownloadAndInstallPackageUpdatesResult
    ANYXBOXX_METHOD(XStoreDownloadAndInstallPackagesAsync),  // 62
    ANYXBOXX_METHOD(XStoreInstallCountResult),  // 63 XStoreDownloadAndInstallPackagesResultCount
    ANYXBOXX_METHOD(XStoreInstallResult),  // 64 XStoreDownloadAndInstallPackagesResult
    ANYXBOXX_NOT_IMPLEMENTED,  // 65 XStoreQueryPackageIdentifier
    ANYXBOXX_METHOD(XStoreRegisterGameLicenseChanged),  // 66
    ANYXBOXX_METHOD(XStoreUnregisterGameLicenseChanged),  // 67
    ANYXBOXX_NOT_IMPLEMENTED,  // 68 XStoreRegisterPackageLicenseLost
    ANYXBOXX_NOT_IMPLEMENTED,  // 69 XStoreUnregisterPackageLicenseLost
    ANYXBOXX_NOT_IMPLEMENTED,  // 70 XStoreIsAvailabilityPurchasable
    ANYXBOXX_METHOD(XStoreAcquireLicenseForDurablesAsync),  // 71
    ANYXBOXX_METHOD(XStoreLicenseResult),  // 72 XStoreAcquireLicenseForDurablesResult
    ANYXBOXX_METHOD(XStoreShowAssociatedProductsUIAsync),  // 73
    ANYXBOXX_METHOD(XStoreOfflineResult),  // 74 XStoreShowAssociatedProductsUIResult
    ANYXBOXX_METHOD(XStoreShowProductPageUIAsync),  // 75
    ANYXBOXX_METHOD(XStoreOfflineResult),  // 76 XStoreShowProductPageUIResult
    ANYXBOXX_METHOD(XStoreQueryAssociatedProductsForStoreIdAsync),  // 77
    ANYXBOXX_METHOD(XStoreQueryResult),  // 78 XStoreQueryAssociatedProductsForStoreIdResult
    ANYXBOXX_NOT_IMPLEMENTED,  // 79 XStoreQueryPackageUpdatesAsync
    ANYXBOXX_NOT_IMPLEMENTED,  // 80 XStoreQueryPackageUpdatesResultCount
    ANYXBOXX_NOT_IMPLEMENTED,  // 81 XStoreQueryPackageUpdatesResult
    ANYXBOXX_METHOD(XStoreShowGiftingUIAsync),  // 82
    ANYXBOXX_METHOD(XStoreOfflineResult),  // 83 XStoreShowGiftingUIResult
};

const anyxboxx::StaticInterface kStore{kMethods};

}  // namespace

namespace anyxboxx {

void* StoreInterface() { return const_cast<StaticInterface*>(&kStore); }

}  // namespace anyxboxx
