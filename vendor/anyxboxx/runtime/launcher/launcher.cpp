// anyxboxx.exe : lance un jeu Xbox en mode natif, sans Python dans le processus.
//
//   anyxboxx.exe <jeu.exe | dossier du paquet> [--fps]
//
// Reprend le chargeur natif de PrototypeScarlett/emu/native : l'image est mappée à la main,
// relogée, liée (runtime C++ d'AnyXboxX pour les DLL console, registre simulé, DXGI pour le
// compteur d'images, fonctions de chargement interceptées), protégée, ses exceptions et sa
// TLS déclarées, puis son point d'entrée est appelé. Le runtime (xgameruntime.dll…) est
// cherché à côté de anyxboxx.exe.
#include <windows.h>

#include <cstdio>
#include <cstring>
#include <cwchar>
#include <string>
#include <vector>

namespace {

const wchar_t* const kRuntimeDlls[] = {L"xgameruntime.dll", L"xmem.dll", L"d3d12_xs.dll", L"kernelx.dll",
                                       L"anyxboxx_tls.dll", L"XCRDAPI.dll"};
const char* const kRegistry[] = {"RegOpenKeyW", "RegOpenKeyExW", "RegQueryValueExW", "RegCloseKey"};
const char* const kDxgi[] = {"CreateDXGIFactory", "CreateDXGIFactory1", "CreateDXGIFactory2"};

struct Game {
  std::wstring path;
  std::wstring folder;
  char* base = nullptr;
  size_t size = 0;
  HMODULE runtime[6] = {};
  bool measuring = false;
};

Game g_game;

int Fail(const wchar_t* message, DWORD error = GetLastError()) {
  fwprintf(stderr, L"[anyxboxx] %ls (erreur %lu)\n", message, error);
  return 1;
}

std::wstring Lower(std::wstring text) {
  for (wchar_t& c : text) c = static_cast<wchar_t>(towlower(c));
  return text;
}

std::wstring BaseName(const std::wstring& path) {
  const size_t slash = path.find_last_of(L"\\/");
  return Lower(slash == std::wstring::npos ? path : path.substr(slash + 1));
}

std::wstring Widen(const char* text) {
  const int length = MultiByteToWideChar(CP_ACP, 0, text, -1, nullptr, 0);
  std::wstring wide(static_cast<size_t>(length > 0 ? length - 1 : 0), L'\0');
  if (length > 1) MultiByteToWideChar(CP_ACP, 0, text, -1, wide.data(), length);
  return wide;
}

bool In(const char* name, const char* const* list, size_t count) {
  for (size_t i = 0; i < count; ++i) {
    if (std::strcmp(name, list[i]) == 0) return true;
  }
  return false;
}

// DLL du runtime qui sert `dll` (xgameruntime*.dll compris), ou nullptr.
HMODULE RuntimeFor(const std::wstring& dll) {
  std::wstring name = BaseName(dll);
  if (name.find(L'.') == std::wstring::npos) name += L".dll";
  if (name.rfind(L"xgameruntime", 0) == 0) return g_game.runtime[0];
  for (int i = 1; i < 4; ++i) {
    if (name == kRuntimeDlls[i]) return g_game.runtime[i];
  }
  return nullptr;
}

FARPROC RuntimeExport(const char* name, int dll = 0) { return GetProcAddress(g_game.runtime[dll], name); }

// --- Fonctions de chargement vues par le jeu (et ses DLL) --------------------------------
HMODULE WINAPI ShimLoadLibraryExW(LPCWSTR name, HANDLE file, DWORD flags) {
  if (name != nullptr) {
    if (HMODULE runtime = RuntimeFor(name)) return runtime;
  }
  return LoadLibraryExW(name, file, flags);
}
HMODULE WINAPI ShimLoadLibraryW(LPCWSTR name) { return ShimLoadLibraryExW(name, nullptr, 0); }
HMODULE WINAPI ShimLoadLibraryExA(LPCSTR name, HANDLE file, DWORD flags) {
  if (name != nullptr) {
    if (HMODULE runtime = RuntimeFor(Widen(name))) return runtime;
  }
  return LoadLibraryExA(name, file, flags);
}
HMODULE WINAPI ShimLoadLibraryA(LPCSTR name) { return ShimLoadLibraryExA(name, nullptr, 0); }

FARPROC WINAPI ShimGetProcAddress(HMODULE module, LPCSTR name) {
  const bool by_name = reinterpret_cast<ULONG_PTR>(name) > 0xFFFF;
  if (by_name && g_game.measuring && module == GetModuleHandleW(L"dxgi.dll") && In(name, kDxgi, 3)) {
    const std::string redirected = std::string("AnyXboxX_") + name;
    return RuntimeExport(redirected.c_str());
  }
  return GetProcAddress(module, name);
}

HMODULE WINAPI ShimGetModuleHandleW(LPCWSTR name) {
  if (name == nullptr) return reinterpret_cast<HMODULE>(g_game.base);
  if (HMODULE runtime = RuntimeFor(name)) return runtime;
  return GetModuleHandleW(name);
}
HMODULE WINAPI ShimGetModuleHandleA(LPCSTR name) {
  if (name == nullptr) return reinterpret_cast<HMODULE>(g_game.base);
  if (HMODULE runtime = RuntimeFor(Widen(name))) return runtime;
  return GetModuleHandleA(name);
}

DWORD WINAPI ShimGetModuleFileNameW(HMODULE module, LPWSTR buffer, DWORD size) {
  if (module != nullptr && module != reinterpret_cast<HMODULE>(g_game.base)) return GetModuleFileNameW(module, buffer, size);
  if (buffer == nullptr || size == 0) return 0;
  const DWORD length = static_cast<DWORD>(g_game.path.size() < size - 1 ? g_game.path.size() : size - 1);
  std::memcpy(buffer, g_game.path.c_str(), length * sizeof(wchar_t));
  buffer[length] = L'\0';
  return length;
}
DWORD WINAPI ShimGetModuleFileNameA(HMODULE module, LPSTR buffer, DWORD size) {
  if (module != nullptr && module != reinterpret_cast<HMODULE>(g_game.base)) return GetModuleFileNameA(module, buffer, size);
  if (buffer == nullptr || size == 0) return 0;
  const int length = WideCharToMultiByte(CP_ACP, 0, g_game.path.c_str(), -1, buffer, static_cast<int>(size), nullptr, nullptr);
  return length > 0 ? static_cast<DWORD>(length - 1) : 0;
}

struct Shim {
  const char* name;
  void* address;
};
const Shim kShims[] = {
    {"LoadLibraryExW", reinterpret_cast<void*>(&ShimLoadLibraryExW)},
    {"LoadLibraryW", reinterpret_cast<void*>(&ShimLoadLibraryW)},
    {"LoadLibraryExA", reinterpret_cast<void*>(&ShimLoadLibraryExA)},
    {"LoadLibraryA", reinterpret_cast<void*>(&ShimLoadLibraryA)},
    {"GetProcAddress", reinterpret_cast<void*>(&ShimGetProcAddress)},
    {"GetModuleHandleW", reinterpret_cast<void*>(&ShimGetModuleHandleW)},
    {"GetModuleHandleA", reinterpret_cast<void*>(&ShimGetModuleHandleA)},
    {"GetModuleFileNameW", reinterpret_cast<void*>(&ShimGetModuleFileNameW)},
    {"GetModuleFileNameA", reinterpret_cast<void*>(&ShimGetModuleFileNameA)},
};

bool IsKernelDll(const std::wstring& dll) {
  const std::wstring name = BaseName(dll);
  return name == L"kernel32.dll" || name == L"kernelbase.dll" || name == L"kernelx.dll" ||
         name.rfind(L"api-ms-win-core-", 0) == 0;
}

// Remplacement d'un import par AnyXboxX, ou nullptr. `game_dll` : imports d'une DLL du jeu.
void* Redirect(const std::wstring& dll, const char* name, bool game_dll) {
  const std::wstring module = BaseName(dll);
  if ((module == L"advapi32.dll" || module.rfind(L"api-ms-win-core-registry-", 0) == 0) && In(name, kRegistry, 4)) {
    return reinterpret_cast<void*>(RuntimeExport((std::string("AnyXboxX_") + name).c_str()));
  }
  if (g_game.measuring && module == L"dxgi.dll" && In(name, kDxgi, 3)) {
    return reinterpret_cast<void*>(RuntimeExport((std::string("AnyXboxX_") + name).c_str()));
  }
  if (IsKernelDll(dll)) {
    for (const Shim& shim : kShims) {
      if (std::strcmp(name, shim.name) == 0 && (!game_dll || std::strcmp(name, "GetProcAddress") == 0)) {
        return shim.address;
      }
    }
  }
  return nullptr;
}

bool WritePointer(void** slot, void* value) {
  DWORD old = 0;
  if (!VirtualProtect(slot, sizeof(void*), PAGE_READWRITE, &old)) return false;
  *slot = value;
  VirtualProtect(slot, sizeof(void*), old, &old);
  return true;
}

// DLL du jeu chargée par Windows : registre, DXGI et GetProcAddress repassent par AnyXboxX.
void PatchModule(HMODULE module) {
  auto* base = reinterpret_cast<char*>(module);
  auto* nt = reinterpret_cast<IMAGE_NT_HEADERS64*>(base + reinterpret_cast<IMAGE_DOS_HEADER*>(base)->e_lfanew);
  const IMAGE_DATA_DIRECTORY& directory = nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT];
  if (directory.VirtualAddress == 0) return;
  for (auto* entry = reinterpret_cast<IMAGE_IMPORT_DESCRIPTOR*>(base + directory.VirtualAddress); entry->Name; ++entry) {
    const std::wstring dll = Widen(base + entry->Name);
    auto* names = reinterpret_cast<IMAGE_THUNK_DATA64*>(base + (entry->OriginalFirstThunk ? entry->OriginalFirstThunk : entry->FirstThunk));
    auto* slots = reinterpret_cast<void**>(base + entry->FirstThunk);
    for (; names->u1.AddressOfData; ++names, ++slots) {
      if (IMAGE_SNAP_BY_ORDINAL64(names->u1.Ordinal)) continue;
      const char* name = reinterpret_cast<IMAGE_IMPORT_BY_NAME*>(base + names->u1.AddressOfData)->Name;
      if (void* target = Redirect(dll, name, true)) WritePointer(slots, target);
    }
  }
}

// Dossier des DLL console de TomBox : ANYXBOXX_TOMBOX_DIR, sinon ..\tombox à côté du dossier du lanceur
// (runtime/build/tombox, comme emu/tombox_paths.py). Ce dossier est produit par tools/build_tombox.py.
std::wstring TomBoxFolder() {
  wchar_t folder[MAX_PATH] = {};
  if (GetEnvironmentVariableW(L"ANYXBOXX_TOMBOX_DIR", folder, MAX_PATH) > 0) return folder;
  wchar_t self[MAX_PATH] = {};
  GetModuleFileNameW(nullptr, self, MAX_PATH);
  const std::wstring self_path = self;
  return self_path.substr(0, self_path.find_last_of(L"\\/")) + L"\\..\\tombox";
}

// Chemin de la DLL de TomBox qui sert `dll`, ou "" si elle n'est pas compilée.
std::wstring TomBoxFile(const std::wstring& dll) {
  const std::wstring path = TomBoxFolder() + L"\\" + BaseName(dll);
  return GetFileAttributesW(path.c_str()) == INVALID_FILE_ATTRIBUTES ? L"" : path;
}

HMODULE LoadDependency(const std::wstring& dll) {
  if (HMODULE runtime = RuntimeFor(dll)) return runtime;
  const std::wstring local = g_game.folder + L"\\" + dll;
  if (GetFileAttributesW(local.c_str()) != INVALID_FILE_ATTRIBUTES) {
    HMODULE module = LoadLibraryExW(local.c_str(), nullptr, LOAD_WITH_ALTERED_SEARCH_PATH);
    if (module != nullptr) PatchModule(module);
    return module;
  }
  // Bibliothèque console absente d'AnyXboxX : celle de TomBox, si elle est compilée.
  const std::wstring tombox = TomBoxFile(dll);
  if (!tombox.empty()) return LoadLibraryExW(tombox.c_str(), nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
  return LoadLibraryW(dll.c_str());
}

// --- Image du jeu --------------------------------------------------------------------------
int MapImage(const std::vector<char>& file) {
  auto* dos = reinterpret_cast<const IMAGE_DOS_HEADER*>(file.data());
  if (file.size() < sizeof(IMAGE_DOS_HEADER) || dos->e_magic != IMAGE_DOS_SIGNATURE ||
      static_cast<size_t>(dos->e_lfanew) + sizeof(IMAGE_NT_HEADERS64) > file.size()) {
    return Fail(L"fichier PE invalide", 0);
  }
  auto* nt = reinterpret_cast<const IMAGE_NT_HEADERS64*>(file.data() + dos->e_lfanew);
  if (nt->FileHeader.Machine != IMAGE_FILE_MACHINE_AMD64 || nt->OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR64_MAGIC) {
    return Fail(L"seul le format PE32+ x86-64 est accepté", 0);
  }
  g_game.size = nt->OptionalHeader.SizeOfImage;
  g_game.base = static_cast<char*>(VirtualAlloc(reinterpret_cast<void*>(nt->OptionalHeader.ImageBase), g_game.size,
                                                MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE));
  if (g_game.base == nullptr) {
    g_game.base = static_cast<char*>(VirtualAlloc(nullptr, g_game.size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE));
  }
  if (g_game.base == nullptr) return Fail(L"mémoire de l'image indisponible");
  std::memcpy(g_game.base, file.data(), nt->OptionalHeader.SizeOfHeaders);
  const auto* section = IMAGE_FIRST_SECTION(nt);
  for (WORD i = 0; i < nt->FileHeader.NumberOfSections; ++i, ++section) {
    if (section->SizeOfRawData == 0) continue;
    if (static_cast<size_t>(section->PointerToRawData) + section->SizeOfRawData > file.size() ||
        static_cast<size_t>(section->VirtualAddress) + section->SizeOfRawData > g_game.size) {
      return Fail(L"section hors de l'image", 0);
    }
    std::memcpy(g_game.base + section->VirtualAddress, file.data() + section->PointerToRawData, section->SizeOfRawData);
  }
  return 0;
}

IMAGE_NT_HEADERS64* Headers() {
  return reinterpret_cast<IMAGE_NT_HEADERS64*>(g_game.base + reinterpret_cast<IMAGE_DOS_HEADER*>(g_game.base)->e_lfanew);
}

int Relocate() {
  IMAGE_NT_HEADERS64* nt = Headers();
  const auto delta = reinterpret_cast<ULONGLONG>(g_game.base) - nt->OptionalHeader.ImageBase;
  if (delta == 0) return 0;
  const IMAGE_DATA_DIRECTORY& directory = nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_BASERELOC];
  if (directory.VirtualAddress == 0) return Fail(L"base préférée occupée et aucune table de relocation", 0);
  char* cursor = g_game.base + directory.VirtualAddress;
  char* end = cursor + directory.Size;
  while (cursor < end) {
    auto* block = reinterpret_cast<IMAGE_BASE_RELOCATION*>(cursor);
    if (block->SizeOfBlock < sizeof(IMAGE_BASE_RELOCATION)) break;
    auto* entries = reinterpret_cast<WORD*>(block + 1);
    const size_t count = (block->SizeOfBlock - sizeof(IMAGE_BASE_RELOCATION)) / sizeof(WORD);
    for (size_t i = 0; i < count; ++i) {
      if ((entries[i] >> 12) == IMAGE_REL_BASED_DIR64) {
        *reinterpret_cast<ULONGLONG*>(g_game.base + block->VirtualAddress + (entries[i] & 0xFFF)) += delta;
      }
    }
    cursor += block->SizeOfBlock;
  }
  return 0;
}

int BindImports() {
  const IMAGE_DATA_DIRECTORY& directory = Headers()->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT];
  if (directory.VirtualAddress == 0) return 0;
  std::wstring missing;
  for (auto* entry = reinterpret_cast<IMAGE_IMPORT_DESCRIPTOR*>(g_game.base + directory.VirtualAddress); entry->Name; ++entry) {
    const std::wstring dll = Widen(g_game.base + entry->Name);
    const HMODULE module = LoadDependency(dll);
    auto* names = reinterpret_cast<IMAGE_THUNK_DATA64*>(g_game.base + (entry->OriginalFirstThunk ? entry->OriginalFirstThunk : entry->FirstThunk));
    auto* slots = reinterpret_cast<ULONGLONG*>(g_game.base + entry->FirstThunk);
    for (; names->u1.AddressOfData; ++names, ++slots) {
      void* address = nullptr;
      std::wstring label;
      if (IMAGE_SNAP_BY_ORDINAL64(names->u1.Ordinal)) {
        const WORD ordinal = static_cast<WORD>(IMAGE_ORDINAL64(names->u1.Ordinal));
        label = L"#" + std::to_wstring(ordinal);
        if (module) address = reinterpret_cast<void*>(GetProcAddress(module, MAKEINTRESOURCEA(ordinal)));
      } else {
        const char* name = reinterpret_cast<IMAGE_IMPORT_BY_NAME*>(g_game.base + names->u1.AddressOfData)->Name;
        label = Widen(name);
        address = Redirect(dll, name, false);
        if (address == nullptr && module) address = reinterpret_cast<void*>(GetProcAddress(module, name));
      }
      if (address == nullptr) {
        missing += L" " + dll + L"!" + label;
        continue;
      }
      *slots = reinterpret_cast<ULONGLONG>(address);
    }
  }
  if (!missing.empty()) {
    fwprintf(stderr, L"[anyxboxx] imports introuvables :%ls\n", missing.c_str());
    return 1;
  }
  return 0;
}

void Protect() {
  IMAGE_NT_HEADERS64* nt = Headers();
  DWORD old = 0;
  VirtualProtect(g_game.base, nt->OptionalHeader.SizeOfHeaders, PAGE_READONLY, &old);
  const auto* section = IMAGE_FIRST_SECTION(nt);
  for (WORD i = 0; i < nt->FileHeader.NumberOfSections; ++i, ++section) {
    const DWORD flags = section->Characteristics;
    const bool exec = (flags & IMAGE_SCN_MEM_EXECUTE) != 0;
    const bool write = (flags & IMAGE_SCN_MEM_WRITE) != 0;
    const DWORD protection = exec ? (write ? PAGE_EXECUTE_READWRITE : PAGE_EXECUTE_READ)
                                  : (write ? PAGE_READWRITE : PAGE_READONLY);
    const DWORD length = section->Misc.VirtualSize > section->SizeOfRawData ? section->Misc.VirtualSize : section->SizeOfRawData;
    if (length) VirtualProtect(g_game.base + section->VirtualAddress, length, protection, &old);
  }
  FlushInstructionCache(GetCurrentProcess(), g_game.base, g_game.size);
}

int RegisterExceptionsAndTls() {
  IMAGE_NT_HEADERS64* nt = Headers();
  const IMAGE_DATA_DIRECTORY& exceptions = nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_EXCEPTION];
  if (exceptions.VirtualAddress && exceptions.Size >= sizeof(RUNTIME_FUNCTION)) {
    RtlAddFunctionTable(reinterpret_cast<PRUNTIME_FUNCTION>(g_game.base + exceptions.VirtualAddress),
                        exceptions.Size / sizeof(RUNTIME_FUNCTION), reinterpret_cast<DWORD64>(g_game.base));
  }
  const IMAGE_DATA_DIRECTORY& tls = nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_TLS];
  if (tls.VirtualAddress == 0) return 0;
  using Install = BOOL(WINAPI*)(void*, void*);
  const auto install = reinterpret_cast<Install>(RuntimeExport("AnyXboxX_InstallImageTls", 4));
  if (install == nullptr || !install(g_game.base, g_game.base + tls.VirtualAddress)) {
    return Fail(L"installation de la TLS implicite impossible", 0);
  }
  return 0;
}

// Le jeu prend sa propre ligne de commande (tampons de GetCommandLineW/A réécrits sur place).
void AdoptCommandLine() {
  const std::wstring line = L"\"" + g_game.path + L"\"";
  wchar_t* wide = GetCommandLineW();
  if (wcslen(wide) >= line.size()) std::memcpy(wide, line.c_str(), (line.size() + 1) * sizeof(wchar_t));
  char* narrow = GetCommandLineA();
  std::string ansi(line.size() * 2 + 1, '\0');
  const int length = WideCharToMultiByte(CP_ACP, 0, line.c_str(), -1, ansi.data(), static_cast<int>(ansi.size()), nullptr, nullptr);
  if (length > 0 && std::strlen(narrow) + 1 >= static_cast<size_t>(length)) std::memcpy(narrow, ansi.c_str(), length);
}

// La première exception (0x80000003 compris) sur stderr, avec son module et sa RVA ; le traitement
// continue comme avant. Les exceptions d'information (0x4xxxxxxx : OutputDebugString, nom de thread)
// ne comptent pas. Sans allocation : l'exception peut venir du tas.
LONG WINAPI ReportFirstException(EXCEPTION_POINTERS* pointers) {
  static LONG reported = 0;
  const DWORD code = pointers->ExceptionRecord->ExceptionCode;
  if ((code >> 30) == 1 || InterlockedExchange(&reported, 1) != 0) return EXCEPTION_CONTINUE_SEARCH;
  const auto address = reinterpret_cast<uintptr_t>(pointers->ExceptionRecord->ExceptionAddress);
  auto start = reinterpret_cast<uintptr_t>(g_game.base);
  wchar_t path[MAX_PATH] = L"?";
  HMODULE module = nullptr;
  if (address - start < g_game.size) {
    wcsncpy_s(path, g_game.path.c_str(), _TRUNCATE);  // mappé à la main : inconnu du chargeur de Windows
  } else if (GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                                reinterpret_cast<LPCWSTR>(address), &module)) {
    GetModuleFileNameW(module, path, MAX_PATH);
    start = reinterpret_cast<uintptr_t>(module);
  } else {
    start = 0;
  }
  const wchar_t* name = path;
  for (const wchar_t* c = path; *c != L'\0'; ++c) {
    if (*c == L'\\' || *c == L'/') name = c + 1;
  }
  fwprintf(stderr, L"[anyxboxx] première exception 0x%08lX : %ls+0x%llX\n", code, name,
           static_cast<unsigned long long>(address - start));
  fflush(stderr);
  return EXCEPTION_CONTINUE_SEARCH;
}

// Dossier de paquet : l'exécutable déclaré par <Executable Name="…"> de MicrosoftGame.config.
std::wstring ResolveTarget(const std::wstring& target) {
  const DWORD attributes = GetFileAttributesW(target.c_str());
  if (attributes == INVALID_FILE_ATTRIBUTES || !(attributes & FILE_ATTRIBUTE_DIRECTORY)) return target;
  FILE* file = nullptr;
  if (_wfopen_s(&file, (target + L"\\MicrosoftGame.config").c_str(), L"rb") != 0 || file == nullptr) return L"";
  std::string text(1024 * 1024, '\0');
  text.resize(std::fread(text.data(), 1, text.size(), file));
  std::fclose(file);
  std::string narrow;
  for (char c : text) {
    if (c != '\0') narrow.push_back(c);  // UTF-16 : balises ASCII
  }
  const size_t tag = narrow.find("<Executable ");
  const size_t name = tag == std::string::npos ? tag : narrow.find("Name=\"", tag);
  if (name == std::string::npos) return L"";
  const size_t end = narrow.find('"', name + 6);
  const std::string exe = narrow.substr(name + 6, end - name - 6);
  if (exe.find("..") != std::string::npos || exe.find(':') != std::string::npos) return L"";  // reste dans le dossier
  return target + L"\\" + Widen(exe.c_str());
}

}  // namespace

int wmain(int argc, wchar_t** argv) {
  if (argc < 2) {
    fwprintf(stderr, L"Usage : anyxboxx.exe <jeu.exe | dossier du paquet> [--fps]\n");
    return 1;
  }
  for (int i = 2; i < argc; ++i) {
    if (std::wcscmp(argv[i], L"--fps") == 0) SetEnvironmentVariableW(L"ANYXBOXX_FPS", L"1");
  }
  wchar_t full[MAX_PATH];
  const std::wstring target = ResolveTarget(argv[1]);
  if (target.empty() || GetFullPathNameW(target.c_str(), MAX_PATH, full, nullptr) == 0) return Fail(L"jeu introuvable", 0);
  g_game.path = full;
  g_game.folder = g_game.path.substr(0, g_game.path.find_last_of(L"\\/"));
  if (GetFileAttributesW(full) == INVALID_FILE_ATTRIBUTES) {
    // Paquet incomplet : le code du jeu manque, rien ne peut le remplacer.
    fwprintf(stderr, L"[anyxboxx] exécutable du jeu absent : %ls\n", full);
    fwprintf(stderr, L"[anyxboxx] le paquet ne contient que les données et les DLL ; "
                     L"le code du jeu est dans cet exécutable.\n");
    return 1;
  }
  wchar_t value[8] = {};
  g_game.measuring = GetEnvironmentVariableW(L"ANYXBOXX_FPS", value, 8) > 0 ||
                     GetEnvironmentVariableW(L"ANYXBOXX_FPS_CSV", value, 8) > 0;

  // Runtime d'AnyXboxX, à côté de ce lanceur.
  wchar_t self[MAX_PATH];
  GetModuleFileNameW(nullptr, self, MAX_PATH);
  const std::wstring self_path = self;
  const std::wstring runtime_folder = self_path.substr(0, self_path.find_last_of(L"\\/"));
  for (int i = 0; i < 6; ++i) {
    std::wstring path = runtime_folder + L"\\" + kRuntimeDlls[i];
    // xmem, d3d12_xs et XCRDAPI : celles de TomBox (plus complètes) quand elles sont compilées.
    const bool prefer_tombox = i == 1 || i == 2 || i == 5;
    const std::wstring tombox = prefer_tombox ? TomBoxFile(kRuntimeDlls[i]) : L"";
    if (!tombox.empty()) path = tombox;
    wchar_t alternative[MAX_PATH] = {};
    // ANYXBOXX_D3D12_XS : une d3d12_xs.dll précise, prioritaire.
    if (i == 2 && GetEnvironmentVariableW(L"ANYXBOXX_D3D12_XS", alternative, MAX_PATH) > 0) path = alternative;
    g_game.runtime[i] = LoadLibraryExW(path.c_str(), nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
    if (g_game.runtime[i] == nullptr) return Fail((L"runtime introuvable : " + path).c_str());
  }

  FILE* file = nullptr;
  if (_wfopen_s(&file, g_game.path.c_str(), L"rb") != 0 || file == nullptr) return Fail(L"lecture du jeu impossible", 0);
  std::vector<char> bytes;
  char chunk[65536];
  for (size_t read; (read = std::fread(chunk, 1, sizeof chunk, file)) > 0;) bytes.insert(bytes.end(), chunk, chunk + read);
  std::fclose(file);

  SetEnvironmentVariableW(L"ANYXBOXX_GAME_DIR", g_game.folder.c_str());
  SetCurrentDirectoryW(g_game.folder.c_str());
  if (MapImage(bytes) || Relocate() || BindImports()) return 1;
  Protect();
  AddVectoredExceptionHandler(1, ReportFirstException);  // avant les rappels TLS, qui sont du code du jeu
  if (RegisterExceptionsAndTls()) return 1;
  AdoptCommandLine();
  using Entry = int(WINAPI*)();
  const auto entry = reinterpret_cast<Entry>(g_game.base + Headers()->OptionalHeader.AddressOfEntryPoint);
  return entry();  // un jeu avec CRT termine lui-même le processus
}
