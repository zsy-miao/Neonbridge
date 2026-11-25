// Stockage des sauvegardes sur le disque du PC (XGameSave, XGameSaveFiles, stockage local).
//
// <racine>\<TitleId>\saves\<configuration>\<XUID>\<conteneur>\<blob>.blob
// <racine> = ANYXBOXX_SAVE_DIR, sinon %LOCALAPPDATA%\AnyXboxX. Les noms du jeu sont
// encodés (%XX hors [A-Za-z0-9_-]) : aucun caractère ne peut sortir du dossier.
#pragma once

#include <windows.h>

#include <cstdint>
#include <string>
#include <vector>

namespace anyxboxx::saves {

constexpr int64_t kQuotaBytes = 256LL * 1024 * 1024;  // quota simulé par configuration
constexpr size_t kMaxContainerName = 255;
constexpr size_t kMaxDisplayName = 127;
constexpr size_t kMaxBlobName = 64;
constexpr size_t kMaxBlobBytes = 16 * 1024 * 1024;

constexpr HRESULT E_GS_INVALID_CONTAINER_NAME = static_cast<HRESULT>(0x80830001);
constexpr HRESULT E_GS_UPDATE_TOO_BIG = static_cast<HRESULT>(0x80830005);
constexpr HRESULT E_GS_QUOTA_EXCEEDED = static_cast<HRESULT>(0x80830006);
constexpr HRESULT E_GS_PROVIDED_BUFFER_TOO_SMALL = static_cast<HRESULT>(0x80830007);
constexpr HRESULT E_GS_BLOB_NOT_FOUND = static_cast<HRESULT>(0x80830008);

struct BlobInfo {
  std::string name;
  uint32_t size;
};

struct ContainerInfo {
  std::string name;
  std::string display_name;
  uint32_t blob_count;
  uint64_t total_size;
  int64_t last_modified;  // secondes depuis 1970 (time_t)
};

struct UpdateOperation {
  std::string blob;
  bool erase;
  std::vector<uint8_t> data;
};

bool ValidName(const char* name, size_t max_length);

// Dossiers (créés au besoin). Chaîne vide si la racine est inaccessible.
std::wstring TitleFolder();
std::wstring ProviderFolder(const char* configuration_id, uint64_t xuid);
std::wstring FilesFolder(const char* configuration_id, uint64_t xuid);
std::wstring LocalStorageFolder();
std::wstring ContainerFolder(const std::wstring& provider, const std::string& name);

std::vector<ContainerInfo> ListContainers(const std::wstring& provider, const char* prefix);
bool FindContainer(const std::wstring& provider, const std::string& name, ContainerInfo* info);
HRESULT DeleteContainer(const std::wstring& provider, const std::string& name);
std::vector<BlobInfo> ListBlobs(const std::wstring& container, const char* prefix);
HRESULT ReadBlob(const std::wstring& container, const std::string& name, std::vector<uint8_t>* data);
HRESULT ApplyUpdate(const std::wstring& provider, const std::wstring& container, const std::string& display_name,
                    const std::vector<UpdateOperation>& operations);
int64_t UsedBytes(const std::wstring& folder);

std::string Utf8(const std::wstring& text);

}  // namespace anyxboxx::saves
