#include "aligned_pages.h"

#include <cstdint>
#include <limits>

namespace anyxboxx::pages {
namespace {

constexpr int kPlacementAttempts = 8;  // un autre thread peut prendre la plage entre les deux appels

std::size_t PageBytes() noexcept {
  static const std::size_t page = [] {
    SYSTEM_INFO info{};
    GetSystemInfo(&info);
    return static_cast<std::size_t>(info.dwPageSize);
  }();
  return page;
}

// Un engagement sur une plage libre doit aussi réserver : la réservation est faite à l'adresse alignée.
DWORD PlacementType(DWORD type) noexcept {
  return (type & MEM_COMMIT) != 0 ? (type | MEM_RESERVE) : type;
}

void* TryAligned(std::size_t bytes, std::size_t alignment, DWORD type, DWORD protect) noexcept {
  void* probe = VirtualAlloc(nullptr, bytes + alignment, MEM_RESERVE, PAGE_NOACCESS);
  if (probe == nullptr) return nullptr;
  const auto aligned = (reinterpret_cast<std::uintptr_t>(probe) + alignment - 1) & ~(alignment - 1);
  VirtualFree(probe, 0, MEM_RELEASE);
  return VirtualAlloc(reinterpret_cast<void*>(aligned), bytes, PlacementType(type), protect);
}

}  // namespace

void* AllocateAligned(std::size_t bytes, std::size_t alignment, DWORD type, DWORD protect) noexcept {
  if (alignment <= PageBytes()) return VirtualAlloc(nullptr, bytes, type, protect);
  if (bytes > std::numeric_limits<std::size_t>::max() - alignment) {
    SetLastError(ERROR_NOT_ENOUGH_MEMORY);
    return nullptr;
  }
  for (int attempt = 0; attempt < kPlacementAttempts; ++attempt) {
    void* block = TryAligned(bytes, alignment, type, protect);
    if (block != nullptr) return block;
  }
  return nullptr;
}

}  // namespace anyxboxx::pages
