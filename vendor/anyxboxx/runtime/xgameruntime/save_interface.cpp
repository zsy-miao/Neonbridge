// Interface XGameSave (et XGameSaveFiles) servie par AnyXboxX : sauvegardes sur le disque du PC.
//
// Fournisseur {704C3F58…}, interface {1BFFF3AF…}, méthodes 3 à 32 (gdk_api_table.inc).
// Signatures et codes d'erreur : en-têtes publics XGameSave.h et XGameSaveFiles.h.
// Le stockage lui-même est dans save_store.cpp ; il n'y a pas de synchronisation
// avec le cloud : rien n'est jamais « hors synchro ».
#include <windows.h>

#include <cstdint>
#include <cstring>
#include <ctime>
#include <memory>
#include <new>
#include <string>
#include <utility>
#include <vector>

#include "XAsync.h"
#include "async_call.h"
#include "interface_helpers.h"
#include "interfaces.h"
#include "save_store.h"
#include "trace.h"

namespace {

using namespace anyxboxx::saves;

constexpr uint32_t kProviderMagic = 0x50475841;   // « AXGP »
constexpr uint32_t kContainerMagic = 0x43475841;  // « AXGC »
constexpr uint32_t kUpdateMagic = 0x55475841;     // « AXGU »
constexpr size_t kMaxConfigurationId = 64;

struct XGameSaveBlobInfo {
  const char* name;
  uint32_t size;
};
struct XGameSaveBlob {
  XGameSaveBlobInfo info;
  uint8_t* data;
};
struct XGameSaveContainerInfo {
  const char* name;
  const char* display_name;
  uint32_t blob_count;
  uint64_t total_size;
  time_t last_modified;
  bool needs_sync;
};
using BlobCallback = bool(CALLBACK*)(const XGameSaveBlobInfo*, void*);
using ContainerCallback = bool(CALLBACK*)(const XGameSaveContainerInfo*, void*);

struct Provider {
  uint32_t magic;
  std::wstring folder;
};
struct Container {
  uint32_t magic;
  std::wstring provider;
  std::wstring folder;
  std::string name;
};
struct Update {
  uint32_t magic;
  std::wstring provider;
  std::wstring folder;
  std::string display_name;
  std::vector<UpdateOperation> operations;
};

template <typename Handle>
Handle* Checked(void* handle, uint32_t magic) {
  auto* typed = static_cast<Handle*>(handle);
  return typed != nullptr && typed->magic == magic ? typed : nullptr;
}

HRESULT OpenProvider(void* user, const char* configuration_id, Provider** provider) {
  uint64_t xuid = 0;
  if (provider == nullptr || !anyxboxx::LocalUserId(user, &xuid)) return E_INVALIDARG;
  if (!ValidName(configuration_id, kMaxConfigurationId)) return E_INVALIDARG;
  const std::wstring folder = ProviderFolder(configuration_id, xuid);
  if (folder.empty()) return HRESULT_FROM_WIN32(ERROR_PATH_NOT_FOUND);
  *provider = new (std::nothrow) Provider{kProviderMagic, folder};
  return *provider != nullptr ? S_OK : E_OUTOFMEMORY;
}

void Report(const ContainerInfo& info, void* context, ContainerCallback callback, bool* keep_going) {
  const XGameSaveContainerInfo out{info.name.c_str(), info.display_name.c_str(), info.blob_count, info.total_size,
                                   static_cast<time_t>(info.last_modified), false};
  *keep_going = callback(&out, context);
}

// Ensemble de blobs lus, disposé dans le tampon du jeu : tableau XGameSaveBlob, puis
// noms, puis données (les pointeurs visent le tampon lui-même).
struct BlobSet {
  std::vector<std::string> names;
  std::vector<std::vector<uint8_t>> data;

  size_t Required() const {
    size_t size = names.size() * sizeof(XGameSaveBlob);
    for (size_t index = 0; index < names.size(); ++index) size += names[index].size() + 1 + data[index].size();
    return size;
  }

  void Layout(void* buffer) const {
    auto* blobs = static_cast<XGameSaveBlob*>(buffer);
    auto* cursor = reinterpret_cast<uint8_t*>(blobs + names.size());
    for (size_t index = 0; index < names.size(); ++index) {
      std::memcpy(cursor, names[index].c_str(), names[index].size() + 1);
      blobs[index].info.name = reinterpret_cast<const char*>(cursor);
      cursor += names[index].size() + 1;
      std::memcpy(cursor, data[index].data(), data[index].size());
      blobs[index].info.size = static_cast<uint32_t>(data[index].size());
      blobs[index].data = cursor;
      cursor += data[index].size();
    }
  }
};

HRESULT Collect(Container* container, const char** names, uint32_t count, BlobSet* set) {
  if (names == nullptr || count == 0) {
    for (const BlobInfo& blob : ListBlobs(container->folder, nullptr)) set->names.push_back(blob.name);
  } else {
    for (uint32_t index = 0; index < count; ++index) {
      if (names[index] == nullptr) return E_INVALIDARG;
      set->names.emplace_back(names[index]);
    }
  }
  for (const std::string& name : set->names) {
    std::vector<uint8_t> data;
    const HRESULT read = ReadBlob(container->folder, name, &data);
    if (FAILED(read)) return read;
    set->data.push_back(std::move(data));
  }
  return S_OK;
}

thread_local uint32_t t_blobs_laid_out = 0;  // posé par la copie du résultat asynchrone

const char kInitializeIdentity = 0;
const char kQuotaIdentity = 0;
const char kEmptyIdentity = 0;
const char kReadIdentity = 0;
const char kFolderIdentity = 0;

anyxboxx::AsyncCall Status(HRESULT result) {
  anyxboxx::AsyncCall call;
  call.result = result;
  return call;
}

template <typename Value>
anyxboxx::AsyncCall ValueResult(HRESULT result, Value value) {
  anyxboxx::AsyncCall call = Status(result);
  call.size = sizeof(Value);
  call.write = [value](void* buffer) { std::memcpy(buffer, &value, sizeof(Value)); };
  return call;
}

// --- Fournisseur ---------------------------------------------------------------------------
HRESULT XGameSaveInitializeProvider(void* user, const char* configuration_id, bool, void** provider) noexcept {
  if (provider == nullptr) return E_INVALIDARG;
  *provider = nullptr;
  Provider* opened = nullptr;
  const HRESULT result = OpenProvider(user, configuration_id, &opened);
  *provider = opened;
  return result;
}

HRESULT XGameSaveInitializeProviderAsync(void* user, const char* configuration_id, bool,
                                         XAsyncBlock* async) noexcept {
  Provider* opened = nullptr;
  const HRESULT result = OpenProvider(user, configuration_id, &opened);
  anyxboxx::AsyncCall call = ValueResult<void*>(result, opened);
  call.discard = [opened] { delete opened; };
  return anyxboxx::BeginAsyncCall(async, &kInitializeIdentity, "XGameSaveInitializeProviderAsync", std::move(call));
}

HRESULT XGameSaveInitializeProviderResult(XAsyncBlock* async, void** provider) noexcept {
  return XAsyncGetResult(async, &kInitializeIdentity, sizeof(void*), provider, nullptr);
}

void XGameSaveCloseProvider(void* handle) noexcept {
  if (Provider* provider = Checked<Provider>(handle, kProviderMagic)) {
    provider->magic = 0;
    delete provider;
  }
}

int64_t RemainingQuota(const Provider& provider) { return kQuotaBytes - UsedBytes(provider.folder); }

HRESULT XGameSaveGetRemainingQuota(void* handle, int64_t* remaining) noexcept {
  Provider* provider = Checked<Provider>(handle, kProviderMagic);
  if (provider == nullptr || remaining == nullptr) return E_INVALIDARG;
  *remaining = RemainingQuota(*provider);
  return S_OK;
}

HRESULT XGameSaveGetRemainingQuotaAsync(void* handle, XAsyncBlock* async) noexcept {
  Provider* provider = Checked<Provider>(handle, kProviderMagic);
  if (provider == nullptr) return E_INVALIDARG;
  return anyxboxx::BeginAsyncCall(async, &kQuotaIdentity, "XGameSaveGetRemainingQuotaAsync",
                                  ValueResult<int64_t>(S_OK, RemainingQuota(*provider)));
}

HRESULT XGameSaveGetRemainingQuotaResult(XAsyncBlock* async, int64_t* remaining) noexcept {
  return XAsyncGetResult(async, &kQuotaIdentity, sizeof(int64_t), remaining, nullptr);
}

HRESULT XGameSaveDeleteContainer(void* handle, const char* name) noexcept {
  Provider* provider = Checked<Provider>(handle, kProviderMagic);
  if (provider == nullptr) return E_INVALIDARG;
  if (!ValidName(name, kMaxContainerName)) return E_GS_INVALID_CONTAINER_NAME;
  return DeleteContainer(provider->folder, name);
}

HRESULT XGameSaveDeleteContainerAsync(void* handle, const char* name, XAsyncBlock* async) noexcept {
  return anyxboxx::BeginAsyncCall(async, &kEmptyIdentity, "XGameSaveDeleteContainerAsync",
                                  Status(XGameSaveDeleteContainer(handle, name)));
}

HRESULT XGameSaveEmptyResult(XAsyncBlock* async) noexcept { return XAsyncGetStatus(async, false); }

// Conteneur absent : le rappel n'est pas appelé (le jeu teste l'existence par le rappel).
HRESULT XGameSaveGetContainerInfo(void* handle, const char* name, void* context, ContainerCallback callback) noexcept {
  Provider* provider = Checked<Provider>(handle, kProviderMagic);
  if (provider == nullptr || callback == nullptr || name == nullptr) return E_INVALIDARG;
  ContainerInfo info;
  bool keep_going = true;
  if (FindContainer(provider->folder, name, &info)) Report(info, context, callback, &keep_going);
  return S_OK;
}

HRESULT XGameSaveEnumerateContainerInfoByName(void* handle, const char* prefix, void* context,
                                              ContainerCallback callback) noexcept {
  Provider* provider = Checked<Provider>(handle, kProviderMagic);
  if (provider == nullptr || callback == nullptr) return E_INVALIDARG;
  bool keep_going = true;
  for (const ContainerInfo& info : ListContainers(provider->folder, prefix)) {
    Report(info, context, callback, &keep_going);
    if (!keep_going) break;
  }
  return S_OK;
}

HRESULT XGameSaveEnumerateContainerInfo(void* handle, void* context, ContainerCallback callback) noexcept {
  return XGameSaveEnumerateContainerInfoByName(handle, nullptr, context, callback);
}

// --- Conteneurs et blobs ----------------------------------------------------------------
HRESULT XGameSaveCreateContainer(void* handle, const char* name, void** container) noexcept {
  Provider* provider = Checked<Provider>(handle, kProviderMagic);
  if (provider == nullptr || container == nullptr) return E_INVALIDARG;
  *container = nullptr;
  if (!ValidName(name, kMaxContainerName)) return E_GS_INVALID_CONTAINER_NAME;
  *container = new (std::nothrow)
      Container{kContainerMagic, provider->folder, ContainerFolder(provider->folder, name), name};
  return *container != nullptr ? S_OK : E_OUTOFMEMORY;
}

void XGameSaveCloseContainer(void* handle) noexcept {
  if (Container* container = Checked<Container>(handle, kContainerMagic)) {
    container->magic = 0;
    delete container;
  }
}

HRESULT XGameSaveEnumerateBlobInfoByName(void* handle, const char* prefix, void* context,
                                         BlobCallback callback) noexcept {
  Container* container = Checked<Container>(handle, kContainerMagic);
  if (container == nullptr || callback == nullptr) return E_INVALIDARG;
  for (const BlobInfo& blob : ListBlobs(container->folder, prefix)) {
    const XGameSaveBlobInfo info{blob.name.c_str(), blob.size};
    if (!callback(&info, context)) break;
  }
  return S_OK;
}

HRESULT XGameSaveEnumerateBlobInfo(void* handle, void* context, BlobCallback callback) noexcept {
  return XGameSaveEnumerateBlobInfoByName(handle, nullptr, context, callback);
}

HRESULT XGameSaveReadBlobData(void* handle, const char** names, uint32_t* count, size_t size,
                              XGameSaveBlob* blobs) noexcept {
  Container* container = Checked<Container>(handle, kContainerMagic);
  if (container == nullptr || count == nullptr) return E_INVALIDARG;
  BlobSet set;
  const HRESULT collected = Collect(container, names, *count, &set);
  if (FAILED(collected)) return collected;
  if (blobs == nullptr || size < set.Required()) return E_GS_PROVIDED_BUFFER_TOO_SMALL;
  set.Layout(blobs);
  *count = static_cast<uint32_t>(set.names.size());
  return S_OK;
}

HRESULT XGameSaveReadBlobDataAsync(void* handle, const char** names, uint32_t count, XAsyncBlock* async) noexcept {
  Container* container = Checked<Container>(handle, kContainerMagic);
  if (container == nullptr) return E_INVALIDARG;
  auto set = std::make_shared<BlobSet>();
  anyxboxx::AsyncCall call = Status(Collect(container, names, count, set.get()));
  call.size = set->Required();
  call.write = [set](void* buffer) {
    set->Layout(buffer);
    t_blobs_laid_out = static_cast<uint32_t>(set->names.size());
  };
  return anyxboxx::BeginAsyncCall(async, &kReadIdentity, "XGameSaveReadBlobDataAsync", std::move(call));
}

HRESULT XGameSaveReadBlobDataResult(XAsyncBlock* async, size_t size, XGameSaveBlob* blobs, uint32_t* count) noexcept {
  t_blobs_laid_out = 0;
  const HRESULT result = XAsyncGetResult(async, &kReadIdentity, size, blobs, nullptr);
  if (count != nullptr) *count = SUCCEEDED(result) ? t_blobs_laid_out : 0;
  return result;
}

// --- Mises à jour : appliquées d'un bloc à la soumission -----------------------------------
HRESULT XGameSaveCreateUpdate(void* handle, const char* display_name, void** update) noexcept {
  Container* container = Checked<Container>(handle, kContainerMagic);
  if (container == nullptr || update == nullptr) return E_INVALIDARG;
  *update = nullptr;
  if (display_name == nullptr || std::strlen(display_name) > kMaxDisplayName) return E_INVALIDARG;
  *update = new (std::nothrow) Update{kUpdateMagic, container->provider, container->folder, display_name, {}};
  return *update != nullptr ? S_OK : E_OUTOFMEMORY;
}

void XGameSaveCloseUpdate(void* handle) noexcept {
  if (Update* update = Checked<Update>(handle, kUpdateMagic)) {
    update->magic = 0;
    delete update;
  }
}

HRESULT XGameSaveSubmitBlobWrite(void* handle, const char* name, const uint8_t* data, size_t size) noexcept {
  Update* update = Checked<Update>(handle, kUpdateMagic);
  if (update == nullptr || !ValidName(name, kMaxBlobName) || (data == nullptr && size > 0)) return E_INVALIDARG;
  if (size > kMaxBlobBytes) return E_GS_UPDATE_TOO_BIG;
  try {
    update->operations.push_back({name, false, std::vector<uint8_t>(data, data + size)});
  } catch (const std::bad_alloc&) {
    return E_OUTOFMEMORY;
  }
  return S_OK;
}

HRESULT XGameSaveSubmitBlobDelete(void* handle, const char* name) noexcept {
  Update* update = Checked<Update>(handle, kUpdateMagic);
  if (update == nullptr || !ValidName(name, kMaxBlobName)) return E_INVALIDARG;
  try {
    update->operations.push_back({name, true, {}});
  } catch (const std::bad_alloc&) {
    return E_OUTOFMEMORY;
  }
  return S_OK;
}

HRESULT XGameSaveSubmitUpdate(void* handle) noexcept {
  Update* update = Checked<Update>(handle, kUpdateMagic);
  if (update == nullptr) return E_INVALIDARG;
  return ApplyUpdate(update->provider, update->folder, update->display_name, update->operations);
}

HRESULT XGameSaveSubmitUpdateAsync(void* handle, XAsyncBlock* async) noexcept {
  return anyxboxx::BeginAsyncCall(async, &kEmptyIdentity, "XGameSaveSubmitUpdateAsync",
                                  Status(XGameSaveSubmitUpdate(handle)));
}

// --- XGameSaveFiles : un dossier ordinaire par utilisateur ---------------------------------
HRESULT XGameSaveFilesGetFolderWithUiAsync(void* user, const char* configuration_id, XAsyncBlock* async) noexcept {
  uint64_t xuid = 0;
  if (!anyxboxx::LocalUserId(user, &xuid) || !ValidName(configuration_id, kMaxConfigurationId)) return E_INVALIDARG;
  const std::string folder = Utf8(FilesFolder(configuration_id, xuid));
  anyxboxx::AsyncCall call = Status(folder.empty() ? HRESULT_FROM_WIN32(ERROR_PATH_NOT_FOUND) : S_OK);
  call.size = folder.size() + 1;
  call.write = [folder](void* buffer) { std::memcpy(buffer, folder.c_str(), folder.size() + 1); };
  return anyxboxx::BeginAsyncCall(async, &kFolderIdentity, "XGameSaveFilesGetFolderWithUiAsync", std::move(call));
}

HRESULT XGameSaveFilesGetFolderWithUiResult(XAsyncBlock* async, size_t size, char* folder) noexcept {
  return XAsyncGetResult(async, &kFolderIdentity, size, folder, nullptr);
}

HRESULT XGameSaveFilesGetRemainingQuota(void* user, const char* configuration_id, int64_t* remaining) noexcept {
  uint64_t xuid = 0;
  if (remaining == nullptr || !anyxboxx::LocalUserId(user, &xuid) || !ValidName(configuration_id, kMaxConfigurationId)) {
    return E_INVALIDARG;
  }
  *remaining = kQuotaBytes - UsedBytes(FilesFolder(configuration_id, xuid));
  return S_OK;
}

void* const kMethods[] = {
    ANYXBOXX_IUNKNOWN_METHODS,                                // 0-2
    ANYXBOXX_METHOD(XGameSaveInitializeProvider),             // 3
    ANYXBOXX_METHOD(XGameSaveInitializeProviderAsync),        // 4
    ANYXBOXX_METHOD(XGameSaveInitializeProviderResult),       // 5
    ANYXBOXX_METHOD(XGameSaveCloseProvider),                  // 6
    ANYXBOXX_METHOD(XGameSaveGetRemainingQuota),              // 7
    ANYXBOXX_METHOD(XGameSaveGetRemainingQuotaAsync),         // 8
    ANYXBOXX_METHOD(XGameSaveGetRemainingQuotaResult),        // 9
    ANYXBOXX_METHOD(XGameSaveDeleteContainer),                // 10
    ANYXBOXX_METHOD(XGameSaveDeleteContainerAsync),           // 11
    ANYXBOXX_METHOD(XGameSaveEmptyResult),                    // 12 XGameSaveDeleteContainerResult
    ANYXBOXX_METHOD(XGameSaveGetContainerInfo),               // 13
    ANYXBOXX_METHOD(XGameSaveEnumerateContainerInfo),         // 14
    ANYXBOXX_METHOD(XGameSaveEnumerateContainerInfoByName),   // 15
    ANYXBOXX_METHOD(XGameSaveCreateContainer),                // 16
    ANYXBOXX_METHOD(XGameSaveCloseContainer),                 // 17
    ANYXBOXX_METHOD(XGameSaveEnumerateBlobInfo),              // 18
    ANYXBOXX_METHOD(XGameSaveEnumerateBlobInfoByName),        // 19
    ANYXBOXX_METHOD(XGameSaveReadBlobData),                   // 20
    ANYXBOXX_METHOD(XGameSaveReadBlobDataAsync),              // 21
    ANYXBOXX_METHOD(XGameSaveReadBlobDataResult),             // 22
    ANYXBOXX_METHOD(XGameSaveCreateUpdate),                   // 23
    ANYXBOXX_METHOD(XGameSaveCloseUpdate),                    // 24
    ANYXBOXX_METHOD(XGameSaveSubmitBlobWrite),                // 25
    ANYXBOXX_METHOD(XGameSaveSubmitBlobDelete),               // 26
    ANYXBOXX_METHOD(XGameSaveSubmitUpdate),                   // 27
    ANYXBOXX_METHOD(XGameSaveSubmitUpdateAsync),              // 28
    ANYXBOXX_METHOD(XGameSaveEmptyResult),                    // 29 XGameSaveSubmitUpdateResult
    ANYXBOXX_METHOD(XGameSaveFilesGetFolderWithUiAsync),      // 30
    ANYXBOXX_METHOD(XGameSaveFilesGetFolderWithUiResult),     // 31
    ANYXBOXX_METHOD(XGameSaveFilesGetRemainingQuota),         // 32
};

const anyxboxx::StaticInterface kGameSave{kMethods};

}  // namespace

namespace anyxboxx {

void* GameSaveInterface() { return const_cast<StaticInterface*>(&kGameSave); }

}  // namespace anyxboxx
