// Photo de profil générée pour le profil local hors ligne (gamer_picture.cpp).
#pragma once

#include <cstdint>
#include <vector>

namespace anyxboxx {

// Côté en pixels pour un XUserGamerPictureSize (0 Small 64, 1 Medium 208, 2 Large 424,
// 3 ExtraLarge 1080) ; faux pour une valeur inconnue.
bool GamerPictureSide(uint32_t size_code, uint32_t* side);

// Image PNG RGB carrée de `side` pixels.
std::vector<uint8_t> GamerPicturePng(uint32_t side);

}  // namespace anyxboxx
