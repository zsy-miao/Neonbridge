// Placement des blocs de xmem.dll selon XALLOC_ATTRIBUTES.
//
// Adapté de Xbox2Windows, src/era_heap.cpp (https://github.com/R0Y00/Xbox2Windows),
// licence MIT, Copyright (c) 2026 Xbox2Windows contributors ; avis complet dans era_heap.h.
//
// Tas (type 0) : CRT à l'alignement demandé, ou réservation propre au-delà d'une page.
// Graphique (types 1 à 6) : région VirtualAlloc séparée, mémoire CPU ordinaire.
// Physique (types 7 à 9) : mémoire de la machine, servie comme des pages committées (le titre
// l'utilise comme de la RAM mappée) ; un alignement au-delà de la granularité est refusé tracé.
#include "era_heap.h"

#include <windows.h>

#include <malloc.h>

#include <algorithm>
#include <cstring>
#include <limits>

#include "aligned_pages.h"
#include "trace.h"

namespace anyxboxx::era {
namespace {

constexpr std::uint32_t kHeap = 0;
constexpr std::uint32_t kGraphicsFirst = 1;
constexpr std::uint32_t kGraphicsLast = 6;
constexpr std::uint32_t kPhysicalLast = 9;
constexpr std::uint32_t kUndefinedPageSize = 3;
constexpr std::uint32_t kMinimumAlignmentLog2 = 4;  // ERA : 16 octets au moins
// Politique du PC, plus stricte que l'ERA : ligne de cache, et une page pour les gros blocs.
constexpr std::size_t kSmallAlignment = 64;
constexpr std::size_t kPageAlignment = 4096;
constexpr std::size_t kPageBytes[] = {std::size_t{4} << 10, std::size_t{64} << 10, std::size_t{4} << 20};

struct PageGeometry {
  std::size_t page;
  std::size_t granularity;
};

const PageGeometry& Geometry() noexcept {
  static const PageGeometry value = [] {
    SYSTEM_INFO info{};
    GetSystemInfo(&info);
    return PageGeometry{info.dwPageSize, info.dwAllocationGranularity};
  }();
  return value;
}

Block Refuse(DWORD error) noexcept {
  SetLastError(error);
  return {nullptr, Placement::kHeap};
}

// Alignement au-delà d'une page : réservation propre dont seules les pages utiles sont
// engagées (VirtualAlloc les rend déjà à zéro).
void* AllocatePages(std::size_t bytes, std::size_t alignment) noexcept {
  const std::size_t page = Geometry().page;
  const std::size_t committed = (bytes + page - 1) & ~(page - 1);
  return pages::AllocateAligned(committed, alignment, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
}

Block AllocateHeap(std::size_t bytes, std::size_t requested) noexcept {
  const std::size_t floor = bytes >= kPageAlignment ? kPageAlignment : kSmallAlignment;
  const std::size_t alignment = std::max(requested, floor);
  if (alignment > Geometry().page) {
    void* block = AllocatePages(bytes, alignment);
    return block != nullptr ? Block{block, Placement::kPages} : Refuse(ERROR_NOT_ENOUGH_MEMORY);
  }
  void* block = _aligned_malloc(bytes, alignment);
  if (block == nullptr) return Refuse(ERROR_NOT_ENOUGH_MEMORY);
  std::memset(block, 0, bytes);
  return {block, Placement::kHeap};
}

// Mémoire physique du titre : pages committées de la machine, à l'alignement demandé tant
// qu'il tient dans la granularité (64 Kio), refusé et tracé au-delà.
Block AllocatePhysical(std::size_t bytes, std::size_t alignment) noexcept {
  if (alignment > Geometry().granularity) {
    anyxboxx::Log("[xmem] XMemAlloc : mémoire physique, alignement %zu au-delà de la granularité non tenu", alignment);
    return Refuse(ERROR_NOT_SUPPORTED);
  }
  void* block = AllocatePages(bytes, alignment);
  return block != nullptr ? Block{block, Placement::kPages} : Refuse(ERROR_NOT_ENOUGH_MEMORY);
}

Block AllocateGraphics(std::size_t bytes, const Fields& fields, std::size_t alignment) noexcept {
  // Une région VirtualAlloc commence sur la granularité d'allocation (64 Kio sur x64), ce qui
  // couvre les pages de 4 et 64 Kio. Les pages de 4 Mio et les alignements plus grands ne
  // sont pas tenus.
  if (std::max(kPageBytes[fields.page_size], alignment) > Geometry().granularity) {
    anyxboxx::Log("[xmem] XMemAlloc : type graphique %u, page %zu, alignement %zu non tenus sur PC",
                  fields.memory_type, kPageBytes[fields.page_size], alignment);
    return Refuse(ERROR_NOT_SUPPORTED);
  }
  void* block = VirtualAlloc(nullptr, bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
  return block != nullptr ? Block{block, Placement::kGraphics} : Refuse(GetLastError());
}

}  // namespace

Fields Decode(std::uint64_t attributes) noexcept {
  return {
      static_cast<std::uint32_t>(attributes & 0x3fff),
      static_cast<std::uint32_t>((attributes >> 14) & 0x3),
      static_cast<std::uint32_t>((attributes >> 16) & 0xff),
      static_cast<std::uint32_t>((attributes >> 24) & 0x1f),
      static_cast<std::uint32_t>((attributes >> 29) & 0xf),
      attributes >> 33,
  };
}

bool IsGraphics(std::uint32_t memory_type) noexcept {
  return memory_type >= kGraphicsFirst && memory_type <= kGraphicsLast;
}

Block Allocate(std::size_t bytes, std::uint64_t attributes) noexcept {
  const Fields fields = Decode(attributes);
  // objectType et allocatorId sont des étiquettes : le tas par défaut de SlimEra les ignore.
  if (fields.reserved != 0 || fields.page_size == kUndefinedPageSize || fields.memory_type > kPhysicalLast) {
    anyxboxx::Log("[xmem] XMemAlloc : attributs 0x%llx non définis", attributes);
    return Refuse(ERROR_INVALID_PARAMETER);
  }
  bytes = std::max<std::size_t>(bytes, 1);  // taille nulle : bloc unique d'un octet, comme avant
  const std::size_t alignment = std::size_t{1} << std::max(kMinimumAlignmentLog2, fields.alignment);
  const auto limit = static_cast<std::size_t>(std::numeric_limits<std::ptrdiff_t>::max());
  if (bytes > limit - alignment - sizeof(void*)) return Refuse(ERROR_NOT_ENOUGH_MEMORY);
  if (fields.memory_type == kHeap) return AllocateHeap(bytes, alignment);
  if (fields.memory_type > kGraphicsLast) return AllocatePhysical(bytes, alignment);
  return AllocateGraphics(bytes, fields, alignment);
}

void Release(const Block& block) noexcept {
  if (block.address == nullptr) return;
  if (block.placement == Placement::kHeap) {
    _aligned_free(block.address);
  } else {
    VirtualFree(block.address, 0, MEM_RELEASE);
  }
}

}  // namespace anyxboxx::era
