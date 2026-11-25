// Pages physiques de xmem.dll : XMemAllocatePhysicalPages, XMemFreePhysicalPages,
// XMemMapPhysicalPages et XMemMapPhysicalPagesScatter, plus les deux appels de mémoire d'outils.
//
// Le PC n'a pas de pages physiques à partager avec le titre. Les numéros de page émis ici sont
// des identifiants de notre table, validés à chaque appel ; un mappage engage sa propre mémoire
// de 64 Kio par page. Deux mappages d'une même page ne sont donc PAS en alias : une écriture
// par l'un n'apparaît pas dans l'autre. C'est la limite de cette version.
//
// Formes : XMemAllocatePhysicalPages(attributs, *compte, *tableau) avec le compte en pages de
// 64 Kio (entrée et sortie) ; XMemFreePhysicalPages(compte, *tableau) ; XMemMapPhysicalPages(
// adresse, compte, *tableau) rend l'adresse mappée. XMemMapPhysicalPagesScatter : forme supposée
// identique (à vérifier sur un exécutable qui l'appelle).
#include <windows.h>

#include <cstddef>
#include <cstdint>
#include <unordered_set>

#include "era_heap.h"
#include "trace.h"

namespace anyxboxx::physical {
namespace {

constexpr std::size_t kPageBytes = std::size_t{64} << 10;
constexpr std::size_t kMaxPages = std::size_t{0x1'0000};  // 4 Gio de pages au plus par appel
constexpr std::size_t kFirstFrame = 0x100;                 // les petits numéros ne sont jamais émis

SRWLOCK g_lock = SRWLOCK_INIT;
std::size_t g_next_frame = kFirstFrame;

std::unordered_set<std::size_t>& LiveFrames() {
  static auto* frames = new std::unordered_set<std::size_t>();  // jamais détruit, comme les blocs de xmem
  return *frames;
}

bool ValidRequest(std::size_t count, const std::size_t* frames) noexcept {
  return frames != nullptr && count > 0 && count <= kMaxPages;
}

// Vrai si chaque numéro est une page vivante. Appelant : g_lock tenu.
bool AllLive(std::size_t count, const std::size_t* frames) {
  for (std::size_t index = 0; index < count; ++index) {
    if (LiveFrames().count(frames[index]) == 0) return false;
  }
  return true;
}

void* MapFrames(void* address, std::size_t count, const std::size_t* frames) noexcept {
  if (!ValidRequest(count, frames)) {
    SetLastError(ERROR_INVALID_PARAMETER);
    return nullptr;
  }
  AcquireSRWLockShared(&g_lock);
  const bool live = AllLive(count, frames);
  ReleaseSRWLockShared(&g_lock);
  if (!live) {
    SetLastError(ERROR_INVALID_PARAMETER);
    anyxboxx::Log("[xmem] XMemMapPhysicalPages : numéro de page inconnu, refusé");
    return nullptr;
  }
  const std::size_t bytes = count * kPageBytes;
  void* mapped = address != nullptr ? VirtualAlloc(address, bytes, MEM_COMMIT, PAGE_READWRITE)
                                    : VirtualAlloc(nullptr, bytes, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
  ANYXBOXX_TRACE("[xmem] XMemMapPhysicalPages %zu pages -> %p (sans alias entre mappages)", count, mapped);
  return mapped;
}

}  // namespace
}  // namespace anyxboxx::physical

extern "C" {

BOOL WINAPI XMemAllocatePhysicalPages(ULONGLONG attributes, SIZE_T* count, SIZE_T* frames) {
  if (count == nullptr || frames == nullptr || anyxboxx::era::Decode(attributes).reserved != 0 ||
      !anyxboxx::physical::ValidRequest(*count, frames)) {
    SetLastError(ERROR_INVALID_PARAMETER);
    return FALSE;
  }
  AcquireSRWLockExclusive(&anyxboxx::physical::g_lock);
  for (std::size_t index = 0; index < *count; ++index) {
    frames[index] = anyxboxx::physical::g_next_frame++;
    anyxboxx::physical::LiveFrames().insert(frames[index]);
  }
  ReleaseSRWLockExclusive(&anyxboxx::physical::g_lock);
  ANYXBOXX_TRACE("[xmem] XMemAllocatePhysicalPages %zu pages", *count);
  return TRUE;
}

BOOL WINAPI XMemFreePhysicalPages(SIZE_T count, SIZE_T* frames) {
  if (!anyxboxx::physical::ValidRequest(count, frames)) {
    SetLastError(ERROR_INVALID_PARAMETER);
    return FALSE;
  }
  AcquireSRWLockExclusive(&anyxboxx::physical::g_lock);
  const bool live = anyxboxx::physical::AllLive(count, frames);
  if (live) {
    for (std::size_t index = 0; index < count; ++index) anyxboxx::physical::LiveFrames().erase(frames[index]);
  }
  ReleaseSRWLockExclusive(&anyxboxx::physical::g_lock);
  if (!live) SetLastError(ERROR_INVALID_PARAMETER);
  return live ? TRUE : FALSE;
}

void* WINAPI XMemMapPhysicalPages(void* address, SIZE_T count, SIZE_T* frames) {
  return anyxboxx::physical::MapFrames(address, count, frames);
}

void* WINAPI XMemMapPhysicalPagesScatter(void* address, SIZE_T count, SIZE_T* frames) {
  return anyxboxx::physical::MapFrames(address, count, frames);
}

// Mémoire d'outils : absente d'une console de vente.
BOOL WINAPI XMemIsToolsMemAvailable() {
  return FALSE;
}

// Non tenue : la forme de XMemTransferMemory n'est pas établie. Erreur tracée, jamais de succès.
BOOL WINAPI XMemTransferMemory() {
  anyxboxx::Log("[xmem] XMemTransferMemory non implémentée");
  SetLastError(ERROR_NOT_SUPPORTED);
  return FALSE;
}

}  // extern "C"
