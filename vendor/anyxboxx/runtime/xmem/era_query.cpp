// Régions graphiques et XMemVirtualQuery.
//
// Adapté de Xbox2Windows, src/era_query.cpp (https://github.com/R0Y00/Xbox2Windows),
// licence MIT, Copyright (c) 2026 Xbox2Windows contributors ; avis complet dans era_heap.h.
#include "era_query.h"

#include <windows.h>

#include <cstdint>
#include <cstring>
#include <iterator>
#include <map>
#include <new>

namespace anyxboxx::era_query {

static_assert(sizeof(MEMORY_BASIC_INFORMATION) == kTailOffset);

namespace {

SRWLOCK g_lock = SRWLOCK_INIT;

std::map<std::uintptr_t, std::uintptr_t>& Regions() {  // début -> fin (exclue)
  static auto* regions = new std::map<std::uintptr_t, std::uintptr_t>();  // vit jusqu'à la fin du processus
  return *regions;
}

bool IsCurrentProcess(HANDLE process) noexcept {
  return process == GetCurrentProcess() || GetProcessId(process) == GetCurrentProcessId();
}

bool IsRegistered(std::uintptr_t address) noexcept {
  AcquireSRWLockShared(&g_lock);
  const auto& regions = Regions();
  const auto next = regions.upper_bound(address);
  const bool inside = next != regions.begin() && address < std::prev(next)->second;
  ReleaseSRWLockShared(&g_lock);
  return inside;
}

}  // namespace

bool RegisterGraphicsRegion(const void* base, std::size_t size) noexcept {
  const auto begin = reinterpret_cast<std::uintptr_t>(base);
  if (base == nullptr || size == 0 || begin + size < begin) return false;
  const std::uintptr_t end = begin + size;
  AcquireSRWLockExclusive(&g_lock);
  auto& regions = Regions();
  const auto next = regions.lower_bound(begin);
  bool added = false;
  const bool overlaps = (next != regions.end() && next->first < end) ||
                        (next != regions.begin() && std::prev(next)->second > begin);
  if (!overlaps) {
    try {
      regions.emplace_hint(next, begin, end);
      added = true;
    } catch (const std::bad_alloc&) {
      added = false;
    }
  }
  ReleaseSRWLockExclusive(&g_lock);
  return added;
}

bool UnregisterGraphicsRegion(const void* base) noexcept {
  AcquireSRWLockExclusive(&g_lock);
  const bool removed = Regions().erase(reinterpret_cast<std::uintptr_t>(base)) == 1;
  ReleaseSRWLockExclusive(&g_lock);
  return removed;
}

std::size_t Query(void* process, const void* address, void* buffer, std::size_t length) noexcept {
  if (length < sizeof(MEMORY_BASIC_INFORMATION)) {
    SetLastError(ERROR_BAD_LENGTH);
    return 0;
  }
  if (buffer == nullptr) {
    SetLastError(ERROR_NOACCESS);
    return 0;
  }
  MEMORY_BASIC_INFORMATION info{};
  if (VirtualQueryEx(process, address, &info, sizeof(info)) != sizeof(info)) return 0;
  std::memcpy(buffer, &info, sizeof(info));
  if (length > kTailOffset) {
    const bool graphics = IsCurrentProcess(process) && IsRegistered(reinterpret_cast<std::uintptr_t>(address));
    static_cast<unsigned char*>(buffer)[kTailOffset] = graphics ? kTailGraphics : 0;
  }
  return sizeof(info);
}

}  // namespace anyxboxx::era_query
