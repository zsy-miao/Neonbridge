// XMemVirtualAlloc de xmem.dll : allocation virtuelle de la console, traduite vers VirtualAlloc.
//
// Forme relevée au premier démarrage (docs/research/first-boot-1021.md, section 4) :
// XMemVirtualAlloc(adresse, taille, type d'allocation, attributs, protection). Le type porte les
// MEM_* et XMEM_LARGE_PAGES (0x20000000) ; les attributs sont des étiquettes ERA (objectType,
// allocatorId), sans effet ici, sauf les bits réservés, refusés ; la protection porte un PAGE_*
// dans son octet bas, les autres bits (cache, emplacement) ne sont pas tenus par le PC.
// Les pages de 2 Mio ne sont pas tenues : la région est alignée sur 2 Mio et engagée en
// mémoire ordinaire. Libération : VirtualFree, comme pour toute réservation du PC.
#include <windows.h>

#include <cstddef>
#include <cstdint>
#include <optional>

#include "aligned_pages.h"
#include "era_heap.h"
#include "trace.h"

namespace anyxboxx::virtual_alloc {
namespace {

constexpr DWORD kXmemLargePages = 0x2000'0000;   // XMEM_LARGE_PAGES, aussi MEM_LARGE_PAGES
constexpr DWORD kAllocationTypeMask = MEM_COMMIT | MEM_RESERVE | MEM_TOP_DOWN;  // les autres drapeaux ne sont pas tenus
constexpr DWORD kProtectionBaseMask = 0xff;
constexpr DWORD kProtectionGuard = PAGE_GUARD;
constexpr DWORD kPageProtections[] = {PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE, PAGE_WRITECOPY,
                                      PAGE_EXECUTE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
                                      PAGE_EXECUTE_WRITECOPY};
constexpr std::size_t kPageGranularity = std::size_t{64} << 10;    // granularité de la console pour les pages 64 Kio
constexpr std::size_t kLargePageBytes = std::size_t{2} << 20;      // XMEM_LARGE_PAGES : 2 Mio
constexpr std::size_t kMinimumBytes = 1;

std::optional<DWORD> Win32Type(DWORD type) noexcept {
  const DWORD mapped = type & kAllocationTypeMask;
  if ((mapped & (MEM_COMMIT | MEM_RESERVE)) == 0) return std::nullopt;  // au moins un des deux
  return mapped;
}

std::optional<DWORD> Win32Protect(DWORD protect) noexcept {
  const DWORD base = protect & kProtectionBaseMask;
  for (const DWORD known : kPageProtections) {
    if (base == known) return base | (protect & kProtectionGuard);
  }
  return std::nullopt;
}

std::size_t Alignment(DWORD type) noexcept {
  return (type & kXmemLargePages) != 0 ? kLargePageBytes : kPageGranularity;
}

std::size_t RoundUp(std::size_t value, std::size_t unit) noexcept {
  return (value + unit - 1) & ~(unit - 1);
}

// Adresse demandée : la plage est élargie à la granularité, comme VirtualAlloc le fait.
void* AtAddress(void* address, std::size_t size, std::size_t alignment, DWORD type, DWORD protect) noexcept {
  const auto start = reinterpret_cast<std::uintptr_t>(address) & ~(alignment - 1);
  const auto end = RoundUp(reinterpret_cast<std::uintptr_t>(address) + size, alignment);
  return VirtualAlloc(reinterpret_cast<void*>(start), end - start, type, protect);
}

void* Refuse(DWORD error, DWORD type, DWORD protect) noexcept {
  anyxboxx::Log("[xmem] XMemVirtualAlloc : type 0x%lx, protection 0x%lx non définis", type, protect);
  SetLastError(error);
  return nullptr;
}

}  // namespace

void* Allocate(void* address, std::size_t size, DWORD type, std::uint64_t attributes, DWORD protect) noexcept {
  const std::optional<DWORD> win_type = Win32Type(type);
  const std::optional<DWORD> win_protect = Win32Protect(protect);
  if (!win_type || !win_protect || era::Decode(attributes).reserved != 0) {
    return Refuse(ERROR_INVALID_PARAMETER, type, protect);
  }
  const std::size_t alignment = Alignment(type);
  void* block = nullptr;
  if (address != nullptr) {
    block = AtAddress(address, size, alignment, *win_type, *win_protect);
  } else {
    const std::size_t bytes = RoundUp(size < kMinimumBytes ? kMinimumBytes : size, alignment);
    block = pages::AllocateAligned(bytes, alignment, *win_type, *win_protect);
  }
  if (block == nullptr) {
    const DWORD error = GetLastError();
    anyxboxx::Log("[xmem] XMemVirtualAlloc %zu octets, type 0x%lx -> NULL (erreur %lu)", size, type, error);
    SetLastError(error);
  }
  return block;
}

}  // namespace anyxboxx::virtual_alloc

extern "C" {

void* WINAPI XMemVirtualAlloc(void* address, SIZE_T size, DWORD type, ULONGLONG attributes, DWORD protect) {
  return anyxboxx::virtual_alloc::Allocate(address, size, type, attributes, protect);
}

}  // extern "C"
