// Attributs d'allocation XALLOC_ATTRIBUTES et placement des blocs de xmem.dll.
//
// Adapté de Xbox2Windows, src/era_heap.h et src/era_heap.cpp
// (https://github.com/R0Y00/Xbox2Windows), sous la licence MIT suivante :
//
//   MIT License
//
//   Copyright (c) 2026 Xbox2Windows contributors
//
//   Permission is hereby granted, free of charge, to any person obtaining a copy
//   of this software and associated documentation files (the "Software"), to deal
//   in the Software without restriction, including without limitation the rights
//   to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//   copies of the Software, and to permit persons to whom the Software is
//   furnished to do so, subject to the following conditions:
//
//   The above copyright notice and this permission notice shall be included in all
//   copies or substantial portions of the Software.
//
//   THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//   IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//   FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//   AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//   LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//   OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//   SOFTWARE.
//
// Disposition publique ERA de la Xbox One (SlimEra, era.h) : objectType:14, pageSize:2,
// allocatorId:8, alignment:5 (log2), memoryType:4, reserved:31. Que la Series X garde cette
// disposition est une hypothèse ; le sens d'allocatorId et la visibilité GPU des types
// graphiques ne sont pas confirmés.
#pragma once

#include <cstddef>
#include <cstdint>

namespace anyxboxx::era {

struct Fields {
  std::uint32_t object_type;
  std::uint32_t page_size;     // 0 : 4 Kio, 1 : 64 Kio, 2 : 4 Mio, 3 : non défini
  std::uint32_t allocator_id;
  std::uint32_t alignment;     // log2 de l'alignement demandé
  std::uint32_t memory_type;   // 0 : tas, 1-6 : graphique, 7-9 : physique, 10-15 : non défini
  std::uint64_t reserved;
};

// Manière dont un bloc doit être rendu : CRT (_aligned_free) ou pages (VirtualFree).
enum class Placement : std::uint8_t { kHeap, kPages, kGraphics };

struct Block {
  void* address;
  Placement placement;
};

Fields Decode(std::uint64_t attributes) noexcept;
bool IsGraphics(std::uint32_t memory_type) noexcept;

// Bloc rempli de zéros, ou adresse nulle avec SetLastError : ERROR_INVALID_PARAMETER pour
// un champ non défini, ERROR_NOT_SUPPORTED pour la mémoire physique et les alignements que
// le PC ne sait pas tenir, ERROR_NOT_ENOUGH_MEMORY pour une taille démesurée.
Block Allocate(std::size_t bytes, std::uint64_t attributes) noexcept;
void Release(const Block& block) noexcept;

}  // namespace anyxboxx::era
