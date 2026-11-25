// Stockage des sauvegardes sur le disque du PC. Voir save_store.h pour l'organisation.
#include "save_store.h"

#include <windows.h>

#include <cstdio>
#include <cstring>

#include "game_config.h"
#include "trace.h"

namespace anyxboxx::saves {
namespace {

constexpr wchar_t kBlobSuffix[] = L".blob";
constexpr wchar_t kDisplayFile[] = L"~display";  // « ~ » est toujours encodé dans les noms du jeu
constexpr int64_t kUnixEpochIn100ns = 116444736000000000LL;

bool Plain(unsigned char byte) {
  return (byte >= 'a' && byte <= 'z') || (byte >= 'A' && byte <= 'Z') || (byte >= '0' && byte <= '9') ||
         byte == '-' || byte == '_';
}

// Nom du jeu (UTF-8 quelconque) → nom de fichier sûr, réversible.
std::wstring Encode(const std::string& name) {
  std::wstring out;
  for (unsigned char byte : name) {
    if (Plain(byte)) {
      out.push_back(static_cast<wchar_t>(byte));
    } else {
      wchar_t escaped[4];
      swprintf_s(escaped, L"%%%02X", byte);
      out += escaped;
    }
  }
  return out;
}

std::string Decode(const std::wstring& name) {
  std::string out;
  for (size_t index = 0; index < name.size(); ++index) {
    if (name[index] == L'%' && index + 2 < name.size()) {
      out.push_back(static_cast<char>(std::wcstoul(name.substr(index + 1, 2).c_str(), nullptr, 16)));
      index += 2;
    } else {
      out.push_back(static_cast<char>(name[index]));
    }
  }
  return out;
}

bool EnsureFolder(const std::wstring& folder) {
  // Crée chaque niveau : CreateDirectoryW ne crée que le dernier.
  for (size_t slash = folder.find(L'\\', 3); slash != std::wstring::npos; slash = folder.find(L'\\', slash + 1)) {
    CreateDirectoryW(folder.substr(0, slash).c_str(), nullptr);
  }
  return CreateDirectoryW(folder.c_str(), nullptr) || GetLastError() == ERROR_ALREADY_EXISTS;
}

std::wstring Root() {
  wchar_t value[MAX_PATH] = {};
  if (GetEnvironmentVariableW(L"ANYXBOXX_SAVE_DIR", value, MAX_PATH) > 0) return value;
  if (GetEnvironmentVariableW(L"LOCALAPPDATA", value, MAX_PATH) > 0) return std::wstring(value) + L"\\AnyXboxX";
  return {};
}

std::wstring Child(const std::wstring& parent, const std::wstring& name) {
  if (parent.empty()) return {};
  const std::wstring folder = parent + L"\\" + name;
  return EnsureFolder(folder) ? folder : std::wstring();
}

bool StartsWith(const std::string& text, const char* prefix) {
  return prefix == nullptr || text.compare(0, std::strlen(prefix), prefix) == 0;
}

template <typename Visit>
void ForEachEntry(const std::wstring& folder, const wchar_t* pattern, Visit visit) {
  WIN32_FIND_DATAW entry{};
  const HANDLE search = FindFirstFileW((folder + L"\\" + pattern).c_str(), &entry);
  if (search == INVALID_HANDLE_VALUE) return;
  do {
    if (entry.cFileName[0] != L'.') visit(entry);
  } while (FindNextFileW(search, &entry));
  FindClose(search);
}

std::string ReadText(const std::wstring& path) {
  FILE* file = nullptr;
  if (_wfopen_s(&file, path.c_str(), L"rb") != 0 || file == nullptr) return {};
  char buffer[256];
  const size_t read = std::fread(buffer, 1, sizeof buffer, file);
  std::fclose(file);
  return std::string(buffer, read);
}

bool WriteAtomically(const std::wstring& path, const uint8_t* data, size_t size) {
  const std::wstring temporary = path + L".tmp";
  FILE* file = nullptr;
  if (_wfopen_s(&file, temporary.c_str(), L"wb") != 0 || file == nullptr) return false;
  const bool written = size == 0 || std::fwrite(data, 1, size, file) == size;
  const bool closed = std::fclose(file) == 0;
  if (!written || !closed) {
    DeleteFileW(temporary.c_str());
    return false;
  }
  return MoveFileExW(temporary.c_str(), path.c_str(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) != 0;
}

int64_t ToUnixTime(const FILETIME& time) {
  ULARGE_INTEGER value{};
  value.LowPart = time.dwLowDateTime;
  value.HighPart = time.dwHighDateTime;
  return (static_cast<int64_t>(value.QuadPart) - kUnixEpochIn100ns) / 10000000;
}

ContainerInfo Describe(const std::wstring& provider, const WIN32_FIND_DATAW& entry) {
  const std::wstring folder = provider + L"\\" + entry.cFileName;
  ContainerInfo info{Decode(entry.cFileName), ReadText(folder + L"\\" + kDisplayFile), 0, 0,
                     ToUnixTime(entry.ftLastWriteTime)};
  for (const BlobInfo& blob : ListBlobs(folder, nullptr)) {
    ++info.blob_count;
    info.total_size += blob.size;
  }
  return info;
}

}  // namespace

bool ValidName(const char* name, size_t max_length) {
  if (name == nullptr) return false;
  const size_t length = std::strlen(name);
  return length > 0 && length <= max_length;
}

std::wstring TitleFolder() {
  wchar_t title[16];
  swprintf_s(title, L"%08X", GameTitleId());
  return Child(Root(), title);
}

std::wstring ProviderFolder(const char* configuration_id, uint64_t xuid) {
  wchar_t user[24];
  swprintf_s(user, L"%016llX", static_cast<unsigned long long>(xuid));
  return Child(Child(Child(TitleFolder(), L"saves"), Encode(configuration_id ? configuration_id : "")), user);
}

std::wstring FilesFolder(const char* configuration_id, uint64_t xuid) {
  wchar_t user[24];
  swprintf_s(user, L"%016llX", static_cast<unsigned long long>(xuid));
  return Child(Child(Child(TitleFolder(), L"files"), Encode(configuration_id ? configuration_id : "")), user);
}

std::wstring LocalStorageFolder() { return Child(TitleFolder(), L"local"); }

std::wstring ContainerFolder(const std::wstring& provider, const std::string& name) {
  return provider + L"\\" + Encode(name);
}

std::vector<ContainerInfo> ListContainers(const std::wstring& provider, const char* prefix) {
  std::vector<ContainerInfo> containers;
  ForEachEntry(provider, L"*", [&](const WIN32_FIND_DATAW& entry) {
    if ((entry.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) && StartsWith(Decode(entry.cFileName), prefix)) {
      containers.push_back(Describe(provider, entry));
    }
  });
  return containers;
}

bool FindContainer(const std::wstring& provider, const std::string& name, ContainerInfo* info) {
  WIN32_FIND_DATAW entry{};
  const HANDLE search = FindFirstFileW(ContainerFolder(provider, name).c_str(), &entry);
  if (search == INVALID_HANDLE_VALUE) return false;
  FindClose(search);
  if (!(entry.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY)) return false;
  if (info != nullptr) *info = Describe(provider, entry);
  return true;
}

HRESULT DeleteContainer(const std::wstring& provider, const std::string& name) {
  const std::wstring folder = ContainerFolder(provider, name);
  ForEachEntry(folder, L"*", [&](const WIN32_FIND_DATAW& entry) {
    DeleteFileW((folder + L"\\" + entry.cFileName).c_str());
  });
  return RemoveDirectoryW(folder.c_str()) || GetLastError() == ERROR_FILE_NOT_FOUND ? S_OK
                                                                                    : HRESULT_FROM_WIN32(GetLastError());
}

std::vector<BlobInfo> ListBlobs(const std::wstring& container, const char* prefix) {
  std::vector<BlobInfo> blobs;
  ForEachEntry(container, L"*.blob", [&](const WIN32_FIND_DATAW& entry) {
    std::wstring file = entry.cFileName;
    file.resize(file.size() - (sizeof kBlobSuffix / sizeof(wchar_t) - 1));
    const std::string name = Decode(file);
    if (StartsWith(name, prefix)) blobs.push_back({name, entry.nFileSizeLow});
  });
  return blobs;
}

HRESULT ReadBlob(const std::wstring& container, const std::string& name, std::vector<uint8_t>* data) {
  FILE* file = nullptr;
  const std::wstring path = container + L"\\" + Encode(name) + kBlobSuffix;
  if (_wfopen_s(&file, path.c_str(), L"rb") != 0 || file == nullptr) return E_GS_BLOB_NOT_FOUND;
  std::fseek(file, 0, SEEK_END);
  const long size = std::ftell(file);
  std::fseek(file, 0, SEEK_SET);
  data->assign(size > 0 ? static_cast<size_t>(size) : 0, 0);
  const bool read = data->empty() || std::fread(data->data(), 1, data->size(), file) == data->size();
  std::fclose(file);
  return read ? S_OK : E_FAIL;
}

HRESULT ApplyUpdate(const std::wstring& provider, const std::wstring& container, const std::string& display_name,
                    const std::vector<UpdateOperation>& operations) {
  size_t written = 0;
  for (const UpdateOperation& operation : operations) written += operation.data.size();
  if (written > kMaxBlobBytes) return E_GS_UPDATE_TOO_BIG;
  if (UsedBytes(provider) + static_cast<int64_t>(written) > kQuotaBytes) return E_GS_QUOTA_EXCEEDED;
  if (!EnsureFolder(container)) return HRESULT_FROM_WIN32(GetLastError());
  for (const UpdateOperation& operation : operations) {
    const std::wstring path = container + L"\\" + Encode(operation.blob) + kBlobSuffix;
    if (operation.erase) {
      DeleteFileW(path.c_str());
    } else if (!WriteAtomically(path, operation.data.data(), operation.data.size())) {
      return HRESULT_FROM_WIN32(GetLastError());
    }
  }
  const std::wstring display = container + L"\\" + kDisplayFile;
  if (!WriteAtomically(display, reinterpret_cast<const uint8_t*>(display_name.data()), display_name.size())) {
    return HRESULT_FROM_WIN32(GetLastError());
  }
  ANYXBOXX_TRACE("[sauvegarde] %zu opération(s) écrite(s) dans %s", operations.size(), Utf8(container).c_str());
  return S_OK;
}

int64_t UsedBytes(const std::wstring& folder) {
  int64_t total = 0;
  ForEachEntry(folder, L"*", [&](const WIN32_FIND_DATAW& entry) {
    if (entry.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) {
      total += UsedBytes(folder + L"\\" + entry.cFileName);
    } else {
      total += (static_cast<int64_t>(entry.nFileSizeHigh) << 32) | entry.nFileSizeLow;
    }
  });
  return total;
}

std::string Utf8(const std::wstring& text) {
  if (text.empty()) return {};
  const int length =
      WideCharToMultiByte(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), nullptr, 0, nullptr, nullptr);
  std::string narrow(static_cast<size_t>(length), '\0');
  WideCharToMultiByte(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), narrow.data(), length, nullptr, nullptr);
  return narrow;
}

}  // namespace anyxboxx::saves
