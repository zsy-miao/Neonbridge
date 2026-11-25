// anyxboxx_tls.dll : TLS implicite (thread_local, __declspec(thread)) d'un jeu chargé à la main.
//
// Windows ne connaît pas l'image du jeu, il ne lui donne donc pas de TLS. Cette DLL
// « porteuse » déclare sa propre zone TLS : Windows lui attribue un vrai indice et alloue
// la zone pour chaque thread, existant ou futur, même quand d'autres DLL se chargent.
// Le jeu reçoit cet indice (_tls_index) ; sa zone est remplie avec le modèle TLS du jeu :
// pour les threads existants à l'installation, pour les nouveaux par le callback TLS
// ci-dessous (exécuté sur le thread, zone déjà allouée).
#include <windows.h>

#include <tlhelp32.h>
#include <winternl.h>

#include <atomic>
#include <cstring>
#include <new>

extern "C" ULONG _tls_index;  // indice TLS de cette DLL, posé par le chargeur de Windows

namespace {

constexpr size_t kCapacity = 256 * 1024;   // TLS du jeu au plus (modèle + zone à zéro)
constexpr size_t kTebTlsPointer = 0x58;    // TEB.ThreadLocalStoragePointer (x64)

// Réserve la place : le jeu utilise la zone TLS de la porteuse depuis son début (le CRT y
// place un marqueur avant t_area), d'où la marge de 64 octets.
__declspec(thread) unsigned char t_area[kCapacity + 64];

struct GameTls {
  const char* start;
  size_t data;
  size_t total;
  PIMAGE_TLS_CALLBACK* callbacks;
  void* base;
};

std::atomic<GameTls*> g_game{nullptr};

void Fill(void* area, const GameTls& game) {
  std::memcpy(area, game.start, game.data);
  std::memset(static_cast<char*>(area) + game.data, 0, game.total - game.data);
}

void RunCallbacks(const GameTls& game, DWORD reason) {
  for (PIMAGE_TLS_CALLBACK* callback = game.callbacks; callback != nullptr && *callback != nullptr; ++callback) {
    (*callback)(game.base, reason, nullptr);
  }
}

// Zone TLS de la porteuse pour le thread dont le TEB est `teb`.
void* AreaOf(void* teb) {
  void** array = *reinterpret_cast<void***>(static_cast<char*>(teb) + kTebTlsPointer);
  return array != nullptr ? array[_tls_index] : nullptr;
}

void NTAPI OnThread(PVOID, DWORD reason, PVOID) {
  const GameTls* game = g_game.load();
  if (game == nullptr) return;
  if (reason == DLL_THREAD_ATTACH) {
    t_area[0] = 0;  // garde la zone référencée
    if (void* area = AreaOf(NtCurrentTeb())) Fill(area, *game);
    RunCallbacks(*game, DLL_THREAD_ATTACH);
  } else if (reason == DLL_THREAD_DETACH) {
    RunCallbacks(*game, DLL_THREAD_DETACH);
  }
}

using QueryThread = NTSTATUS(NTAPI*)(HANDLE, THREADINFOCLASS, PVOID, ULONG, PULONG);

struct ThreadBasicInformation {
  NTSTATUS exit_status;
  void* teb;
  CLIENT_ID client_id;
  KAFFINITY affinity;
  LONG priority;
  LONG base_priority;
};

// Remplit la zone de chaque autre thread du processus (le jeu n'y tourne pas encore).
void FillOtherThreads(const GameTls& game) {
  const auto query = reinterpret_cast<QueryThread>(
      GetProcAddress(GetModuleHandleW(L"ntdll.dll"), "NtQueryInformationThread"));
  const HANDLE snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
  if (query == nullptr || snapshot == INVALID_HANDLE_VALUE) return;
  THREADENTRY32 entry{sizeof entry};
  for (BOOL more = Thread32First(snapshot, &entry); more; more = Thread32Next(snapshot, &entry)) {
    if (entry.th32OwnerProcessID != GetCurrentProcessId() || entry.th32ThreadID == GetCurrentThreadId()) continue;
    const HANDLE thread = OpenThread(THREAD_QUERY_INFORMATION, FALSE, entry.th32ThreadID);
    if (thread == nullptr) continue;
    ThreadBasicInformation info{};
    if (query(thread, static_cast<THREADINFOCLASS>(0), &info, sizeof info, nullptr) >= 0 && info.teb != nullptr) {
      if (void* area = AreaOf(info.teb)) Fill(area, game);
    }
    CloseHandle(thread);
  }
  CloseHandle(snapshot);
}

}  // namespace

// Callback TLS de la porteuse (section .CRT$XLB, lue par le chargeur de Windows).
#pragma comment(linker, "/INCLUDE:_tls_used")
#pragma comment(linker, "/INCLUDE:anyxboxx_tls_callback")
#pragma section(".CRT$XLB", read)
extern "C" __declspec(allocate(".CRT$XLB")) const PIMAGE_TLS_CALLBACK anyxboxx_tls_callback = OnThread;

extern "C" {

// Appelé par le chargeur natif après relocation : `directory` est le IMAGE_TLS_DIRECTORY64
// du jeu en mémoire (adresses déjà corrigées).
__declspec(dllexport) BOOL WINAPI AnyXboxX_InstallImageTls(void* base, IMAGE_TLS_DIRECTORY64* directory) {
  if (base == nullptr || directory == nullptr || directory->AddressOfIndex == 0) return FALSE;
  const auto* start = reinterpret_cast<const char*>(directory->StartAddressOfRawData);
  const auto* end = reinterpret_cast<const char*>(directory->EndAddressOfRawData);
  if (end < start) return FALSE;
  const size_t data = static_cast<size_t>(end - start);
  const size_t total = data + directory->SizeOfZeroFill;
  if (total > kCapacity) return FALSE;  // zone porteuse trop petite pour ce jeu
  auto* game = new (std::nothrow) GameTls{start, data, total,
                                         reinterpret_cast<PIMAGE_TLS_CALLBACK*>(directory->AddressOfCallBacks), base};
  if (game == nullptr) return FALSE;
  *reinterpret_cast<DWORD*>(directory->AddressOfIndex) = _tls_index;
  void* area = AreaOf(NtCurrentTeb());
  if (area == nullptr) {
    delete game;
    return FALSE;
  }
  Fill(area, *game);
  FillOtherThreads(*game);
  g_game.store(game);  // l'ancienne description reste valide : un thread peut encore la lire
  RunCallbacks(*game, DLL_PROCESS_ATTACH);
  return TRUE;
}

// Image déchargée : les nouveaux threads ne reçoivent plus son modèle.
__declspec(dllexport) void WINAPI AnyXboxX_RemoveImageTls(void* base) {
  GameTls* game = g_game.load();
  if (game != nullptr && game->base == base) g_game.store(nullptr);
}

}  // extern "C"
