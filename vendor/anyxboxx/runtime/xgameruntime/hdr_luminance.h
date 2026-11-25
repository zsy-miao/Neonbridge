// Luminances de tone mapping rendues par XDisplayTryEnableHdrMode (XDisplayHdrModeInfo, XDisplay.h du GDK).
//
// DXGI (IDXGIOutput6::GetDesc1) lit ces valeurs dans les métadonnées HDR statiques de l'EDID.
// Un écran qui n'en annonce pas (ou les met à zéro) rend 0 ; CTA-861.3 lit 0 comme « non défini »,
// et un tone mapping qui plafonne ou divise par la luminance maximale n'a alors rien d'utilisable.
// Valeurs de repli : celles de DXVK (NormalizeDisplayMetadata, src/wsi/wsi_edid.h, zlib),
// qui reprennent ce que DXGI de Windows rend pour les dalles à EDID défaillant.
#pragma once

#include <algorithm>

namespace anyxboxx {

struct HdrModeInfo {
  float min_tone_map_luminance;
  float max_tone_map_luminance;
  float max_full_frame_tone_map_luminance;
};

constexpr float kFallbackMinLuminance = 0.01f;           // nits
constexpr float kFallbackMaxLuminance = 1499.0f;         // nits
constexpr float kFallbackMaxFullFrameLuminance = 799.0f; // nits

// Garde les valeurs de l'écran si elles sont cohérentes (0 <= min < max). Sinon (absentes, NaN,
// inversées), le bloc entier est inutilisable : les trois valeurs de repli le remplacent. Une
// luminance plein écran absente ou au-dessus du pic devient min(repli, max).
inline HdrModeInfo NormalizeHdrLuminance(HdrModeInfo reported) noexcept {
  const float min = reported.min_tone_map_luminance;
  const float max = reported.max_tone_map_luminance;
  if (!(min >= 0.0f) || !(max > min)) {
    return HdrModeInfo{kFallbackMinLuminance, kFallbackMaxLuminance, kFallbackMaxFullFrameLuminance};
  }
  float full_frame = reported.max_full_frame_tone_map_luminance;
  if (!(full_frame > 0.0f)) full_frame = std::min(kFallbackMaxFullFrameLuminance, max);
  return HdrModeInfo{min, max, std::min(full_frame, max)};
}

}  // namespace anyxboxx
