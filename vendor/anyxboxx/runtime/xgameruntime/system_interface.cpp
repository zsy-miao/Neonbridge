// Identité Xbox Series X servie par AnyXboxX : XSystem, analytique, Title ID, lancement,
// fonctionnalités du runtime et report d'extinction de l'écran.
//
// Méthodes et interfaces : gdk_api_table.inc. Signatures : en-têtes publics XSystem.h,
// XGame.h, XLauncher.h, XGameRuntimeFeature.h et XDisplay.h du GDK.
#include <windows.h>

#include <cstdint>
#include <cstring>

#include "game_config.h"
#include "interface_helpers.h"
#include "interfaces.h"
#include "trace.h"

namespace {

constexpr HRESULT E_NOT_SUFFICIENT_BUFFER_HR = HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER);

// Identifiants fixes de la console émulée (tailles : XSystemConsoleIdBytes = 39,
// XSystemAppSpecificDeviceIdBytes = 45, XSystemXboxLiveSandboxIdMaxBytes = 16).
constexpr char kConsoleId[] = "00000000.00000000.00000000.00000000.00";
constexpr char kSandboxId[] = "RETAIL";
constexpr char kAppDeviceId[] = "QW55WGJveFgtZGV2aWNlLTAwMDAwMDAwMDAwMDAx";

HRESULT CopyText(const char* text, size_t size, char* buffer, size_t* used) {
  const size_t needed = std::strlen(text) + 1;
  if (used != nullptr) *used = needed;
  if (buffer == nullptr || size < needed) return E_NOT_SUFFICIENT_BUFFER_HR;
  std::memcpy(buffer, text, needed);
  return S_OK;
}

// --- XSystem -----------------------------------------------------------------------
HRESULT XSystemGetConsoleId(size_t size, char* buffer, size_t* used) noexcept {
  return CopyText(kConsoleId, size, buffer, used);
}
HRESULT XSystemGetXboxLiveSandboxId(size_t size, char* buffer, size_t* used) noexcept {
  return CopyText(kSandboxId, size, buffer, used);
}
HRESULT XSystemGetAppSpecificDeviceId(size_t size, char* buffer, size_t* used) noexcept {
  return CopyText(kAppDeviceId, size, buffer, used);
}
HRESULT XSystemHandleTrack(void*, void*) noexcept { return S_OK; }  // pas de suivi des handles
bool XSystemIsHandleValid(void*) noexcept { return true; }
void XSystemAllowFullDownloadBandwidth(bool) noexcept {}

// --- Analytique : structure renvoyée par valeur (pointeur de retour après this) -----
struct XVersion {
  uint16_t major;
  uint16_t minor;
  uint16_t build;
  uint16_t revision;
};

struct XSystemAnalyticsInfo {
  XVersion os_version;
  XVersion hosting_os_version;
  char family[64];
  char form[64];
};
static_assert(sizeof(XSystemAnalyticsInfo) == 0x90, "taille relevée dans xgameruntime.lib");

XSystemAnalyticsInfo* WINAPI GetAnalyticsInfo(void*, XSystemAnalyticsInfo* out) noexcept {
  constexpr XVersion kXboxOs = {10, 0, 25398, 0};
  *out = XSystemAnalyticsInfo{kXboxOs, kXboxOs, "Windows.Xbox", "Xbox Series X"};
  return out;
}

// --- Jeu et lancement ----------------------------------------------------------------
HRESULT XGameGetXboxTitleId(uint32_t* title_id) noexcept {
  if (title_id == nullptr) return E_INVALIDARG;
  *title_id = anyxboxx::GameTitleId();
  return *title_id != 0 ? S_OK : E_FAIL;
}

void XLaunchNewGame(const char* exe, const char*, void*) noexcept {
  anyxboxx::Log("[runtime] XLaunchNewGame(%s) : changement de jeu pas encore pris en charge",
                exe != nullptr ? exe : "");
}

HRESULT XLaunchRestartOnCrash(const char*, uint32_t) noexcept { return S_OK; }

// Ouvrir des liens au nom du jeu exposerait le PC : refusé.
HRESULT XLaunchUri(void*, const char* uri) noexcept {
  ANYXBOXX_TRACE("[runtime] XLaunchUri(%s) refusé", uri != nullptr ? uri : "");
  return E_NOTIMPL;
}

bool XGameRuntimeIsFeatureAvailable(uint32_t) noexcept { return true; }

// --- Report de l'extinction de l'écran : rien à reporter sur PC ----------------------
HRESULT XDisplayAcquireTimeoutDeferral(void** handle) noexcept {
  if (handle == nullptr) return E_INVALIDARG;
  *handle = reinterpret_cast<void*>(static_cast<uintptr_t>(1));
  return S_OK;
}
void XDisplayCloseTimeoutDeferralHandle(void*) noexcept {}

void* const kSystemMethods[] = {
    ANYXBOXX_IUNKNOWN_METHODS,                          // 0-2
    ANYXBOXX_METHOD(XSystemGetConsoleId),               // 3
    ANYXBOXX_METHOD(XSystemGetXboxLiveSandboxId),       // 4
    ANYXBOXX_METHOD(XSystemGetAppSpecificDeviceId),     // 5
    ANYXBOXX_METHOD(XSystemHandleTrack),                // 6
    ANYXBOXX_METHOD(XSystemIsHandleValid),              // 7
    ANYXBOXX_METHOD(XSystemAllowFullDownloadBandwidth), // 8
};
void* const kAnalyticsMethods[] = {ANYXBOXX_IUNKNOWN_METHODS, reinterpret_cast<void*>(&GetAnalyticsInfo)};
void* const kGameMethods[] = {
    ANYXBOXX_IUNKNOWN_METHODS,                     // 0-2
    ANYXBOXX_METHOD(XGameGetXboxTitleId),          // 3
    ANYXBOXX_METHOD(XLaunchNewGame),               // 4
    ANYXBOXX_METHOD(XLaunchRestartOnCrash),        // 5
};
void* const kFeatureMethods[] = {ANYXBOXX_IUNKNOWN_METHODS, ANYXBOXX_METHOD(XGameRuntimeIsFeatureAvailable)};
void* const kDisplayMethods[] = {
    ANYXBOXX_IUNKNOWN_METHODS,                              // 0-2
    ANYXBOXX_METHOD(XLaunchUri),                            // 3
    ANYXBOXX_METHOD(XDisplayAcquireTimeoutDeferral),        // 4
    ANYXBOXX_METHOD(XDisplayCloseTimeoutDeferralHandle),    // 5
};

const anyxboxx::StaticInterface kSystem{kSystemMethods};
const anyxboxx::StaticInterface kAnalytics{kAnalyticsMethods};
const anyxboxx::StaticInterface kGame{kGameMethods};
const anyxboxx::StaticInterface kFeature{kFeatureMethods};
const anyxboxx::StaticInterface kDisplay{kDisplayMethods};

}  // namespace

namespace anyxboxx {

void* SystemInterface() { return const_cast<StaticInterface*>(&kSystem); }
void* AnalyticsInterface() { return const_cast<StaticInterface*>(&kAnalytics); }
void* GameInterface() { return const_cast<StaticInterface*>(&kGame); }
void* FeatureInterface() { return const_cast<StaticInterface*>(&kFeature); }
void* DisplayInterface() { return const_cast<StaticInterface*>(&kDisplay); }

}  // namespace anyxboxx
