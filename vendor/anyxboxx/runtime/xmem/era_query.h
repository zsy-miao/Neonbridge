// XMemVirtualQuery : MEMORY_BASIC_INFORMATION suivie d'un octet de drapeaux.
//
// Adapté de Xbox2Windows, src/era_query.h et src/era_query.cpp
// (https://github.com/R0Y00/Xbox2Windows), licence MIT, Copyright (c) 2026 Xbox2Windows
// contributors ; avis complet dans era_heap.h et THIRD_PARTY.md.
//
// Forme relevée aux quatre appels BVH étudiés par Xbox2Windows : processus, adresse, tampon
// de 64 octets mis à zéro, sans longueur ; résultat 32 bits testé. Les octets 0x00-0x2F
// suivent MEMORY_BASIC_INFORMATION (State lu à +0x20, Protect à +0x24). L'octet +0x30 n'a
// pas de définition publique : ici, bit 0 = adresse dans une région graphique enregistrée.
// C'est une politique du PC, pas un champ confirmé de la Series X.
#pragma once

#include <cstddef>

namespace anyxboxx::era_query {

constexpr std::size_t kTailOffset = 0x30;
constexpr unsigned char kTailGraphics = 0x01;

// Les régions ne se chevauchent pas : faux pour une région nulle, vide, qui déborde ou
// qui chevauche une région déjà enregistrée.
bool RegisterGraphicsRegion(const void* base, std::size_t size) noexcept;
bool UnregisterGraphicsRegion(const void* base) noexcept;

// Écrit MEMORY_BASIC_INFORMATION dans buffer[0, 0x30) et, si length > 0x30, l'octet +0x30 ;
// les octets suivants restent intacts. Rend le nombre d'octets MEMORY_BASIC_INFORMATION
// écrits, ou 0 en cas d'échec (dernière erreur posée, comme VirtualQueryEx).
std::size_t Query(void* process, const void* address, void* buffer, std::size_t length) noexcept;

}  // namespace anyxboxx::era_query
