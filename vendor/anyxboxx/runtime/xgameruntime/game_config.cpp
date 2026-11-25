// Lecture minimale de MicrosoftGame.config : seulement les éléments dont le runtime a besoin.
// Le fichier vient du jeu : taille bornée, aucun DTD interprété, simple recherche de texte.
#include "game_config.h"

#include <windows.h>

#include <cstdlib>
#include <cstring>
#include <string>

#include "trace.h"

namespace {

constexpr DWORD kMaxConfigBytes = 1024 * 1024;

// Contenu du fichier ramené à de l'ASCII (les balises et le Title ID le sont toujours).
std::string ReadConfigText() {
  wchar_t folder[MAX_PATH] = {};
  const DWORD length = GetEnvironmentVariableW(L"ANYXBOXX_GAME_DIR", folder, MAX_PATH);
  if (length == 0 || length >= MAX_PATH - 24) return {};
  std::wstring path(folder, length);
  path += L"\\MicrosoftGame.config";
  const HANDLE file = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING,
                                  FILE_ATTRIBUTE_NORMAL, nullptr);
  if (file == INVALID_HANDLE_VALUE) return {};
  std::string bytes;
  LARGE_INTEGER size{};
  if (GetFileSizeEx(file, &size) && size.QuadPart > 0 && size.QuadPart <= kMaxConfigBytes) {
    bytes.resize(static_cast<size_t>(size.QuadPart));
    DWORD read = 0;
    if (!ReadFile(file, bytes.data(), static_cast<DWORD>(bytes.size()), &read, nullptr)) read = 0;
    bytes.resize(read);
  }
  CloseHandle(file);
  // UTF-16 : on garde les octets non nuls, suffisant pour des balises et des chiffres ASCII.
  const bool utf16 = bytes.size() >= 2 && ((bytes[0] == '\xFF' && bytes[1] == '\xFE') ||
                                           (bytes[0] == '\xFE' && bytes[1] == '\xFF'));
  if (!utf16) return bytes;
  std::string narrow;
  narrow.reserve(bytes.size() / 2);
  for (char byte : bytes) {
    if (byte != '\0') narrow.push_back(byte);
  }
  return narrow;
}

std::string ElementText(const std::string& text, const char* name) {
  const std::string open = std::string("<") + name + ">";
  const std::string close = std::string("</") + name + ">";
  const size_t start = text.find(open);
  if (start == std::string::npos) return {};
  const size_t end = text.find(close, start + open.size());
  if (end == std::string::npos) return {};
  return text.substr(start + open.size(), end - start - open.size());
}

// Valeur de l'attribut `attribute` du premier élément `<element …>`.
std::string AttributeText(const std::string& text, const char* element, const char* attribute) {
  const size_t start = text.find(std::string("<") + element);
  if (start == std::string::npos) return {};
  const size_t end = text.find('>', start);
  const std::string tag = text.substr(start, end == std::string::npos ? std::string::npos : end - start);
  const std::string key = std::string(" ") + attribute + "=\"";
  const size_t value = tag.find(key);
  if (value == std::string::npos) return {};
  const size_t close = tag.find('"', value + key.size());
  if (close == std::string::npos) return {};
  return tag.substr(value + key.size(), close - value - key.size());
}

}  // namespace

namespace anyxboxx {

GameIdentity ReadGameIdentity() {
  const std::string text = ReadConfigText();
  GameIdentity identity;
  identity.name = AttributeText(text, "Identity", "Name");
  identity.publisher = AttributeText(text, "Identity", "Publisher");
  identity.version = AttributeText(text, "Identity", "Version");
  identity.display_name = AttributeText(text, "ShellVisuals", "DefaultDisplayName");
  identity.store_id = ElementText(text, "StoreId");
  wchar_t folder[MAX_PATH] = {};
  const DWORD length = GetEnvironmentVariableW(L"ANYXBOXX_GAME_DIR", folder, MAX_PATH);
  if (length > 0 && length < MAX_PATH) identity.folder.assign(folder, length);
  return identity;
}

uint32_t GameTitleId() {
  const std::string value = ElementText(ReadConfigText(), "TitleId");
  if (value.empty() || value.size() > 8) return 0;
  char* end = nullptr;
  const unsigned long title_id = std::strtoul(value.c_str(), &end, 16);
  if (end == nullptr || *end != '\0') return 0;
  ANYXBOXX_TRACE("[runtime] Title ID du jeu : %08lX", title_id);
  return static_cast<uint32_t>(title_id);
}

uint64_t GameLocalStorageBytes() {
  const std::string value = ElementText(ReadConfigText(), "SizeMB");
  if (value.empty() || value.size() > 9) return 0;
  char* end = nullptr;
  const unsigned long megabytes = std::strtoul(value.c_str(), &end, 10);
  if (end == nullptr || *end != '\0') return 0;
  return static_cast<uint64_t>(megabytes) * 1024 * 1024;
}

}  // namespace anyxboxx
