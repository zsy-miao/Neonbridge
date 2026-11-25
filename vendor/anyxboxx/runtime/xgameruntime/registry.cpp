// Registre vu par le jeu : la valeur qui fait de la machine une Xbox Series X.
//
// xgameruntime.lib (lié dans le jeu) calcule XSystemGetDeviceType en lisant
// HKLM\Software\Microsoft\Windows NT\CurrentVersion\OEM, valeur ConsoleMode ;
// 0x64 donne XboxScarlettAnaconda (relevé au désassemblage du GDK 2610). Le mode
// natif d'AnyXboxX remplace les fonctions de registre du jeu par celles-ci : seule
// cette clé est simulée, tout le reste va au vrai registre.
#include <windows.h>

#include <cstring>
#include <cwchar>

#include "trace.h"

namespace {

constexpr wchar_t kOemKey[] = L"Software\\Microsoft\\Windows NT\\CurrentVersion\\OEM";
constexpr wchar_t kConsoleMode[] = L"ConsoleMode";
constexpr DWORD kScarlettAnaconda = 0x64;
// Handle fictif rendu pour la clé simulée : jamais transmis au vrai registre.
const HKEY kOemHandle = reinterpret_cast<HKEY>(static_cast<ULONG_PTR>(0x41585853));  // « AXXS »

bool IsOemKey(HKEY root, const wchar_t* path) {
  if (root != HKEY_LOCAL_MACHINE || path == nullptr) return false;
  if (*path == L'\\') ++path;
  return _wcsicmp(path, kOemKey) == 0;
}

}  // namespace

extern "C" {

LSTATUS WINAPI AnyXboxX_RegOpenKeyExW(HKEY root, const wchar_t* path, DWORD options, REGSAM access,
                                      HKEY* result) {
  if (IsOemKey(root, path) && result != nullptr) {
    ANYXBOXX_TRACE("[runtime] registre : clé OEM simulée (Xbox Series X)");
    *result = kOemHandle;
    return ERROR_SUCCESS;
  }
  return RegOpenKeyExW(root, path, options, access, result);
}

LSTATUS WINAPI AnyXboxX_RegOpenKeyW(HKEY root, const wchar_t* path, HKEY* result) {
  return AnyXboxX_RegOpenKeyExW(root, path, 0, MAXIMUM_ALLOWED, result);
}

LSTATUS WINAPI AnyXboxX_RegQueryValueExW(HKEY key, const wchar_t* name, DWORD* reserved, DWORD* type,
                                         BYTE* data, DWORD* size) {
  if (key != kOemHandle) return RegQueryValueExW(key, name, reserved, type, data, size);
  if (name == nullptr || _wcsicmp(name, kConsoleMode) != 0) return ERROR_FILE_NOT_FOUND;
  if (type != nullptr) *type = REG_DWORD;
  if (data != nullptr) {
    if (size == nullptr || *size < sizeof(DWORD)) {
      if (size != nullptr) *size = sizeof(DWORD);
      return ERROR_MORE_DATA;
    }
    std::memcpy(data, &kScarlettAnaconda, sizeof(DWORD));
  }
  if (size != nullptr) *size = sizeof(DWORD);
  return ERROR_SUCCESS;
}

LSTATUS WINAPI AnyXboxX_RegCloseKey(HKEY key) {
  return key == kOemHandle ? ERROR_SUCCESS : RegCloseKey(key);
}

}  // extern "C"
