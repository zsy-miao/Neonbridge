// Interface XPersistentLocalStorage servie par AnyXboxX : un dossier du PC par jeu.
//
// Fournisseur {F4FAF4D4…}, interface {41A4E10C…}, méthodes 3 à 8 (gdk_api_table.inc).
// La taille annoncée est celle du <PersistentLocalStorage><SizeMB> du MicrosoftGame.config.
#include <windows.h>

#include <cstdint>
#include <cstring>
#include <string>

#include "XAsync.h"
#include "async_call.h"
#include "game_config.h"
#include "interface_helpers.h"
#include "interfaces.h"
#include "save_store.h"

namespace {

constexpr HRESULT E_NOT_SUFFICIENT_BUFFER_HR = HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER);
constexpr uint64_t kDefaultBytes = 512ull * 1024 * 1024;  // si la config ne déclare rien

struct XPersistentLocalStorageSpaceInfo {
  uint64_t available_free_bytes;
  uint64_t total_free_bytes;
  uint64_t used_bytes;
  uint64_t total_bytes;
};

std::string Folder() {
  const std::wstring folder = anyxboxx::saves::LocalStorageFolder();
  return folder.empty() ? std::string() : anyxboxx::saves::Utf8(folder) + "\\";
}

HRESULT XPersistentLocalStorageGetPathSize(size_t* size) noexcept {
  if (size == nullptr) return E_INVALIDARG;
  const std::string folder = Folder();
  if (folder.empty()) return HRESULT_FROM_WIN32(ERROR_PATH_NOT_FOUND);
  *size = folder.size() + 1;
  return S_OK;
}

HRESULT XPersistentLocalStorageGetPath(size_t size, char* path, size_t* used) noexcept {
  const std::string folder = Folder();
  if (folder.empty()) return HRESULT_FROM_WIN32(ERROR_PATH_NOT_FOUND);
  if (used != nullptr) *used = folder.size() + 1;
  if (path == nullptr || size < folder.size() + 1) return E_NOT_SUFFICIENT_BUFFER_HR;
  std::memcpy(path, folder.c_str(), folder.size() + 1);
  return S_OK;
}

HRESULT XPersistentLocalStorageGetSpaceInfo(XPersistentLocalStorageSpaceInfo* info) noexcept {
  if (info == nullptr) return E_INVALIDARG;
  const uint64_t declared = anyxboxx::GameLocalStorageBytes();
  const uint64_t total = declared != 0 ? declared : kDefaultBytes;
  const auto used = static_cast<uint64_t>(anyxboxx::saves::UsedBytes(anyxboxx::saves::LocalStorageFolder()));
  const uint64_t free = used < total ? total - used : 0;
  *info = XPersistentLocalStorageSpaceInfo{free, free, used, total};
  return S_OK;
}

const char kPromptIdentity = 0;

// Pas de fenêtre à afficher : la place demandée est considérée comme accordée.
HRESULT XPersistentLocalStoragePromptUserForSpaceAsync(uint64_t, XAsyncBlock* async) noexcept {
  return anyxboxx::BeginAsyncCall(async, &kPromptIdentity, "XPersistentLocalStoragePromptUserForSpaceAsync",
                                  anyxboxx::AsyncCall{});
}

HRESULT XPersistentLocalStoragePromptUserForSpaceResult(XAsyncBlock* async) noexcept {
  return XAsyncGetStatus(async, false);
}

// Stockage d'un autre paquet : pas encore pris en charge.
HRESULT XPersistentLocalStorageMountForPackage(const char*, void** mount) noexcept {
  if (mount != nullptr) *mount = nullptr;
  return E_NOTIMPL;
}

void* const kMethods[] = {
    ANYXBOXX_IUNKNOWN_METHODS,                                         // 0-2
    ANYXBOXX_METHOD(XPersistentLocalStorageGetPathSize),               // 3
    ANYXBOXX_METHOD(XPersistentLocalStorageGetPath),                   // 4
    ANYXBOXX_METHOD(XPersistentLocalStorageGetSpaceInfo),              // 5
    ANYXBOXX_METHOD(XPersistentLocalStoragePromptUserForSpaceAsync),   // 6
    ANYXBOXX_METHOD(XPersistentLocalStoragePromptUserForSpaceResult),  // 7
    ANYXBOXX_METHOD(XPersistentLocalStorageMountForPackage),           // 8
};

const anyxboxx::StaticInterface kLocalStorage{kMethods};

}  // namespace

namespace anyxboxx {

void* LocalStorageInterface() { return const_cast<StaticInterface*>(&kLocalStorage); }

}  // namespace anyxboxx
