// xmem.dll d'AnyXboxX : allocateur mémoire de la console, traduit vers la mémoire du PC.
//
// Signatures relevées aux appels (pas d'en-tête public) : XMemAlloc(taille, attributs) et
// XMemFree(pointeur, attributs), redirigées vers des crochets que le jeu peut remplacer
// (XMemSetAllocationHooks) ; XMemVirtualQuery(processus, adresse, tampon de 64 octets).
// Les attributs XALLOC_ATTRIBUTES sont décodés avec la disposition ERA (era_heap.h, porté
// de Xbox2Windows) : tas et types graphiques servis, mémoire physique refusée.
//
// Chaque bloc revient à l'allocateur qui l'a produit : les blocs par défaut sont
// enregistrés avec leur placement, et les deux crochets sont publiés ensemble (une seule
// paire atomique). Les blocs graphiques sont aussi déclarés à XMemVirtualQuery.
#include <windows.h>

#include <atomic>
#include <optional>
#include <unordered_map>

#include "era_heap.h"
#include "era_query.h"
#include "trace.h"

namespace {

using AllocRoutine = void*(WINAPI*)(SIZE_T, ULONGLONG);
using FreeRoutine = void(WINAPI*)(void*, ULONGLONG);
using anyxboxx::era::Block;
using anyxboxx::era::Placement;

struct Hooks {
  AllocRoutine alloc;
  FreeRoutine release;
};

constexpr SIZE_T kQueryBufferBytes = 64;  // tampon des appelants BVH (relevé par Xbox2Windows)

constexpr Hooks kNoHooks{nullptr, nullptr};
std::atomic<const Hooks*> g_hooks{&kNoHooks};

SRWLOCK g_blocks_lock = SRWLOCK_INIT;
std::unordered_map<void*, Placement>& DefaultBlocks() {
  static auto* blocks = new std::unordered_map<void*, Placement>();  // jamais détruit : vit jusqu'à la fin du processus
  return *blocks;
}

void Remember(const Block& block) {
  AcquireSRWLockExclusive(&g_blocks_lock);
  DefaultBlocks().emplace(block.address, block.placement);
  ReleaseSRWLockExclusive(&g_blocks_lock);
}

std::optional<Block> Forget(void* address) {
  AcquireSRWLockExclusive(&g_blocks_lock);
  std::optional<Block> found;
  auto& blocks = DefaultBlocks();
  if (const auto it = blocks.find(address); it != blocks.end()) {
    found = Block{address, it->second};
    blocks.erase(it);
  }
  ReleaseSRWLockExclusive(&g_blocks_lock);
  return found;
}

// Libère un bloc par défaut ; faux s'il n'en est pas un.
bool ReleaseDefault(void* address) {
  const std::optional<Block> block = Forget(address);
  if (!block) return false;
  if (block->placement == Placement::kGraphics) anyxboxx::era_query::UnregisterGraphicsRegion(address);
  anyxboxx::era::Release(*block);
  return true;
}

}  // namespace

extern "C" {

void* WINAPI XMemAllocDefault(SIZE_T size, ULONGLONG attributes) {
  const Block block = anyxboxx::era::Allocate(size, attributes);
  if (block.address != nullptr && block.placement == Placement::kGraphics &&
      !anyxboxx::era_query::RegisterGraphicsRegion(block.address, size == 0 ? 1 : size)) {
    anyxboxx::Log("[xmem] XMemAlloc : région graphique %p déjà enregistrée", block.address);
    anyxboxx::era::Release(block);
    SetLastError(ERROR_INVALID_ADDRESS);
    return nullptr;
  }
  if (block.address != nullptr) Remember(block);
  ANYXBOXX_TRACE("[xmem] XMemAlloc %zu octets (attributs 0x%llx) -> %p", size, attributes, block.address);
  return block.address;
}

void WINAPI XMemFreeDefault(void* address, ULONGLONG) {
  if (address == nullptr) return;
  if (!ReleaseDefault(address)) anyxboxx::Log("[xmem] XMemFreeDefault : bloc %p inconnu, ignoré", address);
}

void* WINAPI XMemAlloc(SIZE_T size, ULONGLONG attributes) {
  const Hooks* hooks = g_hooks.load(std::memory_order_acquire);
  return hooks->alloc != nullptr ? hooks->alloc(size, attributes) : XMemAllocDefault(size, attributes);
}

void WINAPI XMemFree(void* address, ULONGLONG attributes) {
  if (address == nullptr) return;
  if (ReleaseDefault(address)) return;  // alloué par défaut, même si des crochets ont été posés depuis
  const Hooks* hooks = g_hooks.load(std::memory_order_acquire);
  if (hooks->release != nullptr) {
    hooks->release(address, attributes);
  } else {
    anyxboxx::Log("[xmem] XMemFree : bloc %p inconnu (crochets retirés ?), ignoré", address);
  }
}

void WINAPI XMemSetAllocationHooks(AllocRoutine alloc, FreeRoutine release) {
  // Paires jamais libérées : un autre thread peut encore lire l'ancienne (quelques octets par appel).
  const Hooks* hooks = (alloc == nullptr && release == nullptr) ? &kNoHooks : new Hooks{alloc, release};
  g_hooks.store(hooks, std::memory_order_release);
}

void WINAPI XMemGetAllocationHooks(AllocRoutine* alloc, FreeRoutine* release) {
  const Hooks* hooks = g_hooks.load(std::memory_order_acquire);
  if (alloc != nullptr) *alloc = hooks->alloc;
  if (release != nullptr) *release = hooks->release;
}

// MEMORY_BASIC_INFORMATION puis l'octet +0x30 (bit 0 : mémoire graphique), voir era_query.h.
BOOL WINAPI XMemVirtualQuery(HANDLE process, LPCVOID address, void* buffer) {
  return anyxboxx::era_query::Query(process, address, buffer, kQueryBufferBytes) != 0;
}

// La structure XMEM_WORKING_SET_STATISTICS n'est pas publique : on signale l'échec
// (FALSE), que les appelants connus traitent comme « mémoire utilisée inconnue ».
BOOL WINAPI XMemGetWorkingSetStatistics(int type, void*) {
  ANYXBOXX_TRACE("[xmem] XMemGetWorkingSetStatistics(%d) non disponible", type);
  SetLastError(ERROR_NOT_SUPPORTED);
  return FALSE;
}

// Outil d'AnyXboxX (pas une fonction de la console) : champs ERA d'un mot d'attributs.
void WINAPI AnyXboxX_DecodeAllocAttributes(ULONGLONG attributes, anyxboxx::era::Fields* fields) {
  if (fields != nullptr) *fields = anyxboxx::era::Decode(attributes);
}

}  // extern "C"
