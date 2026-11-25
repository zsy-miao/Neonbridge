// Réservation de pages alignées au-delà de la page, partagée par xmem.dll (tas, mémoire
// physique, XMemVirtualAlloc). VirtualAlloc ne garantit que la granularité (64 Kio) ; ici
// l'alignement demandé est tenu en réservant une plage plus grande, puis en la rendant.
#pragma once

#include <windows.h>

#include <cstddef>

namespace anyxboxx::pages {

// Bloc de `bytes` octets, adresse multiple de `alignment` (puissance de deux). Rend NULL avec
// la dernière erreur posée. Sans alignement au-delà d'une page, c'est un simple VirtualAlloc.
void* AllocateAligned(std::size_t bytes, std::size_t alignment, DWORD type, DWORD protect) noexcept;

}  // namespace anyxboxx::pages
