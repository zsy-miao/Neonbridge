// Interface XPackage servie par AnyXboxX : le jeu est un paquet entièrement installé en local.
//
// Fournisseur {AF406016…}, interface {0B9D556E…}, méthodes 3 à 41 (gdk_api_table.inc).
// Signatures : en-tête public XPackage.h. Identité lue dans MicrosoftGame.config ; tous
// les morceaux (chunks) sont prêts, toute installation est déjà terminée, et le montage
// d'un paquet donne le dossier du jeu.
#include <windows.h>

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <new>
#include <string>
#include <utility>

#include "XAsync.h"
#include "XTaskQueue.h"
#include "async_call.h"
#include "game_config.h"
#include "interface_helpers.h"
#include "interfaces.h"
#include "save_store.h"
#include "trace.h"

namespace {

constexpr HRESULT E_NOT_SUFFICIENT_BUFFER_HR = HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER);
constexpr size_t kMaxIdentifier = 32;  // XPACKAGE_IDENTIFIER_MAX_LENGTH (33) sans le zéro final
constexpr uint32_t kMonitorMagic = 0x4D505841;  // « AXPM »
constexpr uint32_t kMountMagic = 0x4E505841;    // « AXPN »

enum class XPackageChunkAvailability : uint32_t { Ready = 0 };
enum class XPackageKind : uint32_t { Game = 0 };

struct XVersion {
  uint16_t major, minor, build, revision;
};
struct XPackageInstallationProgress {
  uint64_t total_bytes, installed_bytes, launch_bytes;
  bool launchable, completed;
};
struct XPackageDetails {
  const char* package_identifier;
  XVersion version;
  XPackageKind kind;
  const char* display_name;
  const char* description;
  const char* publisher;
  const char* store_id;
  bool installing;
  uint32_t index;
  uint32_t count;
  bool age_restricted;
  const char* title_id;
};
struct XPackageWriteStats {
  uint64_t interval, budget, elapsed, bytes_written;
};
using EnumerationCallback = bool(CALLBACK*)(void*, const XPackageDetails*);

struct Monitor {
  uint32_t magic;
};
struct Mount {
  uint32_t magic;
  std::string path;
};

std::string Identifier() {
  std::string name = anyxboxx::ReadGameIdentity().name;
  if (name.empty()) name = "AnyXboxX.Game";
  return name.substr(0, kMaxIdentifier);
}

HRESULT CopyText(const std::string& text, size_t size, char* buffer) {
  if (buffer == nullptr || size < text.size() + 1) return E_NOT_SUFFICIENT_BUFFER_HR;
  std::memcpy(buffer, text.c_str(), text.size() + 1);
  return S_OK;
}

XVersion ParseVersion(const std::string& text) {
  unsigned int parts[4] = {};
  sscanf_s(text.c_str(), "%u.%u.%u.%u", &parts[0], &parts[1], &parts[2], &parts[3]);
  return XVersion{static_cast<uint16_t>(parts[0]), static_cast<uint16_t>(parts[1]),
                  static_cast<uint16_t>(parts[2]), static_cast<uint16_t>(parts[3])};
}

Monitor* NewMonitor() { return new (std::nothrow) Monitor{kMonitorMagic}; }

// --- Identité --------------------------------------------------------------------------
HRESULT XPackageGetCurrentProcessPackageIdentifier(size_t size, char* buffer) noexcept {
  return CopyText(Identifier(), size, buffer);
}

bool XPackageIsPackagedProcess() noexcept { return true; }

HRESULT XPackageGetUserLocale(size_t size, char* locale) noexcept {
  wchar_t wide[LOCALE_NAME_MAX_LENGTH] = {};
  if (GetUserDefaultLocaleName(wide, LOCALE_NAME_MAX_LENGTH) == 0) return HRESULT_FROM_WIN32(GetLastError());
  return CopyText(anyxboxx::saves::Utf8(wide), size, locale);
}

HRESULT XPackageGetPackageKind(const char*, XPackageKind* kind) noexcept {
  if (kind == nullptr) return E_INVALIDARG;
  *kind = XPackageKind::Game;
  return S_OK;
}

HRESULT XPackageEnumeratePackages(uint32_t kind, uint32_t, void* context, EnumerationCallback callback) noexcept {
  if (callback == nullptr) return E_INVALIDARG;
  if (kind != static_cast<uint32_t>(XPackageKind::Game)) return S_OK;  // pas de contenu additionnel
  const anyxboxx::GameIdentity identity = anyxboxx::ReadGameIdentity();
  const std::string identifier = Identifier();
  char title_id[16];
  sprintf_s(title_id, "%08X", anyxboxx::GameTitleId());
  const XPackageDetails details{identifier.c_str(), ParseVersion(identity.version), XPackageKind::Game,
                                identity.display_name.c_str(), "", identity.publisher.c_str(),
                                identity.store_id.c_str(), false, 0, 1, false, title_id};
  callback(context, &details);
  return S_OK;
}

HRESULT XPackageEnumerateFeatures(const char*, void*, void*) noexcept { return S_OK; }  // aucune option

// --- Morceaux du paquet : tous installés --------------------------------------------------
HRESULT XPackageFindChunkAvailability(const char*, uint32_t, void*, XPackageChunkAvailability* availability) noexcept {
  if (availability == nullptr) return E_INVALIDARG;
  *availability = XPackageChunkAvailability::Ready;
  return S_OK;
}

HRESULT XPackageEnumerateChunkAvailability(const char*, uint32_t, void*, void*) noexcept { return S_OK; }
HRESULT XPackageChangeChunkInstallOrder(const char*, uint32_t, void*) noexcept { return S_OK; }
HRESULT XPackageUninstallChunks(const char*, uint32_t, void*) noexcept { return S_OK; }

HRESULT XPackageEstimateDownloadSize(const char*, uint32_t, void*, uint64_t* size, bool* confirm) noexcept {
  if (size != nullptr) *size = 0;
  if (confirm != nullptr) *confirm = false;
  return S_OK;
}

HRESULT XPackageCreateInstallationMonitor(const char*, uint32_t, void*, uint32_t, XTaskQueueHandle,
                                          void** monitor) noexcept {
  if (monitor == nullptr) return E_INVALIDARG;
  *monitor = NewMonitor();
  return *monitor != nullptr ? S_OK : E_OUTOFMEMORY;
}

HRESULT XPackageInstallChunks(const char* id, uint32_t count, void* selectors, uint32_t interval, bool,
                              XTaskQueueHandle queue, void** monitor) noexcept {
  return XPackageCreateInstallationMonitor(id, count, selectors, interval, queue, monitor);
}

const char kInstallIdentity = 0;

HRESULT XPackageInstallChunksAsync(const char*, uint32_t, void*, uint32_t, bool, XAsyncBlock* async) noexcept {
  Monitor* monitor = NewMonitor();
  if (monitor == nullptr) return E_OUTOFMEMORY;
  anyxboxx::AsyncCall call;
  call.size = sizeof(void*);
  call.write = [monitor](void* buffer) { std::memcpy(buffer, &monitor, sizeof(void*)); };
  call.discard = [monitor] { delete monitor; };
  return anyxboxx::BeginAsyncCall(async, &kInstallIdentity, "XPackageInstallChunksAsync", std::move(call));
}

HRESULT XPackageInstallChunksResult(XAsyncBlock* async, void** monitor) noexcept {
  return XAsyncGetResult(async, &kInstallIdentity, sizeof(void*), monitor, nullptr);
}

void XPackageCloseInstallationMonitorHandle(void* handle) noexcept {
  auto* monitor = static_cast<Monitor*>(handle);
  if (monitor != nullptr && monitor->magic == kMonitorMagic) {
    monitor->magic = 0;
    delete monitor;
  }
}

void XPackageGetInstallationProgress(void*, XPackageInstallationProgress* progress) noexcept {
  if (progress != nullptr) *progress = XPackageInstallationProgress{1, 1, 1, true, true};
}

bool XPackageUpdateInstallationMonitor(void*) noexcept { return false; }  // rien ne change : déjà terminé

HRESULT NewToken(XTaskQueueRegistrationToken* token) {
  static volatile LONG64 next_token = 0;
  if (token == nullptr) return E_INVALIDARG;
  token->token = static_cast<uint64_t>(InterlockedIncrement64(&next_token));
  return S_OK;
}

HRESULT XPackageRegisterInstallationProgressChanged(void*, void*, void*, XTaskQueueRegistrationToken* token) noexcept {
  return NewToken(token);
}
bool XPackageUnregisterInstallationProgressChanged(void*, XTaskQueueRegistrationToken, bool) noexcept { return true; }
HRESULT XPackageRegisterPackageInstalled(XTaskQueueHandle, void*, void*, XTaskQueueRegistrationToken* token) noexcept {
  return NewToken(token);
}
bool XPackageUnregisterPackageInstalled(XTaskQueueRegistrationToken, bool) noexcept { return true; }

// --- Montage : le dossier du jeu ------------------------------------------------------------
const char kMountIdentity = 0;

HRESULT XPackageMountWithUiAsync(const char*, XAsyncBlock* async) noexcept {
  const std::wstring folder = anyxboxx::ReadGameIdentity().folder;
  auto* mount = new (std::nothrow) Mount{kMountMagic, anyxboxx::saves::Utf8(folder)};
  if (mount == nullptr) return E_OUTOFMEMORY;
  anyxboxx::AsyncCall call;
  call.result = folder.empty() ? HRESULT_FROM_WIN32(ERROR_PATH_NOT_FOUND) : S_OK;
  call.size = sizeof(void*);
  call.write = [mount](void* buffer) { std::memcpy(buffer, &mount, sizeof(void*)); };
  call.discard = [mount] { delete mount; };
  return anyxboxx::BeginAsyncCall(async, &kMountIdentity, "XPackageMountWithUiAsync", std::move(call));
}

HRESULT XPackageMountWithUiResult(XAsyncBlock* async, void** mount) noexcept {
  return XAsyncGetResult(async, &kMountIdentity, sizeof(void*), mount, nullptr);
}

Mount* CheckedMount(void* handle) {
  auto* mount = static_cast<Mount*>(handle);
  return mount != nullptr && mount->magic == kMountMagic ? mount : nullptr;
}

HRESULT XPackageGetMountPathSize(void* handle, size_t* size) noexcept {
  Mount* mount = CheckedMount(handle);
  if (mount == nullptr || size == nullptr) return E_INVALIDARG;
  *size = mount->path.size() + 1;
  return S_OK;
}

HRESULT XPackageGetMountPath(void* handle, size_t size, char* path) noexcept {
  Mount* mount = CheckedMount(handle);
  if (mount == nullptr) return E_INVALIDARG;
  return CopyText(mount->path, size, path);
}

void XPackageCloseMountHandle(void* handle) noexcept {
  if (Mount* mount = CheckedMount(handle)) {
    mount->magic = 0;
    delete mount;
  }
}

HRESULT XPackageGetWriteStats(XPackageWriteStats* stats) noexcept {
  if (stats == nullptr) return E_INVALIDARG;
  *stats = XPackageWriteStats{0, UINT64_MAX, 0, 0};  // pas de budget d'écriture sur PC
  return S_OK;
}

// Désinstaller des applications ou des paquets depuis le jeu : refusé.
HRESULT XPackageUninstallUWPInstance(const char*) noexcept { return E_NOTIMPL; }
bool XPackageUninstallPackage(const char*) noexcept { return false; }

void* const kMethods[] = {
    ANYXBOXX_IUNKNOWN_METHODS,                                      // 0-2
    ANYXBOXX_METHOD(XPackageGetCurrentProcessPackageIdentifier),    // 3
    ANYXBOXX_METHOD(XPackageIsPackagedProcess),                     // 4
    ANYXBOXX_METHOD(XPackageCreateInstallationMonitor),             // 5
    ANYXBOXX_METHOD(XPackageCloseInstallationMonitorHandle),        // 6
    ANYXBOXX_METHOD(XPackageGetInstallationProgress),               // 7
    ANYXBOXX_METHOD(XPackageUpdateInstallationMonitor),             // 8
    ANYXBOXX_METHOD(XPackageRegisterInstallationProgressChanged),   // 9
    ANYXBOXX_METHOD(XPackageUnregisterInstallationProgressChanged), // 10
    ANYXBOXX_METHOD(XPackageGetUserLocale),                         // 11
    ANYXBOXX_METHOD(XPackageFindChunkAvailability),                 // 12
    ANYXBOXX_METHOD(XPackageEnumerateChunkAvailability),            // 13
    ANYXBOXX_METHOD(XPackageChangeChunkInstallOrder),               // 14
    ANYXBOXX_METHOD(XPackageInstallChunks),                         // 15
    ANYXBOXX_METHOD(XPackageInstallChunksAsync),                    // 16
    ANYXBOXX_METHOD(XPackageInstallChunksResult),                   // 17
    ANYXBOXX_METHOD(XPackageEstimateDownloadSize),                  // 18
    ANYXBOXX_METHOD(XPackageUninstallChunks),                       // 19
    ANYXBOXX_NOT_IMPLEMENTED,                                       // 20 : aucune fonction publique
    ANYXBOXX_NOT_IMPLEMENTED,                                       // 21
    ANYXBOXX_METHOD(XPackageUnregisterPackageInstalled),            // 22
    ANYXBOXX_NOT_IMPLEMENTED,                                       // 23
    ANYXBOXX_METHOD(XPackageGetMountPathSize),                      // 24
    ANYXBOXX_METHOD(XPackageGetMountPath),                          // 25
    ANYXBOXX_METHOD(XPackageCloseMountHandle),                      // 26
    ANYXBOXX_NOT_IMPLEMENTED,                                       // 27 XPackageGetIdentifier : absent de XPackage.h
    ANYXBOXX_NOT_IMPLEMENTED,                                       // 28
    ANYXBOXX_NOT_IMPLEMENTED,                                       // 29
    ANYXBOXX_METHOD(XPackageGetWriteStats),                         // 30
    ANYXBOXX_NOT_IMPLEMENTED,                                       // 31
    ANYXBOXX_METHOD(XPackageUninstallUWPInstance),                  // 32
    ANYXBOXX_METHOD(XPackageEnumerateFeatures),                     // 33
    ANYXBOXX_METHOD(XPackageUninstallPackage),                      // 34
    ANYXBOXX_NOT_IMPLEMENTED,                                       // 35
    ANYXBOXX_NOT_IMPLEMENTED,                                       // 36
    ANYXBOXX_METHOD(XPackageMountWithUiAsync),                      // 37
    ANYXBOXX_METHOD(XPackageMountWithUiResult),                     // 38
    ANYXBOXX_METHOD(XPackageEnumeratePackages),                     // 39
    ANYXBOXX_METHOD(XPackageRegisterPackageInstalled),              // 40
    ANYXBOXX_METHOD(XPackageGetPackageKind),                        // 41
};

const anyxboxx::StaticInterface kPackage{kMethods};

}  // namespace

namespace anyxboxx {

void* PackageInterface() { return const_cast<StaticInterface*>(&kPackage); }

}  // namespace anyxboxx
