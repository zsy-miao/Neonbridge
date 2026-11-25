// Photo de profil du profil local hors ligne : un PNG RGB généré (fond vert Xbox, disque clair).
//
// Format PNG (RFC 2083) : signature, IHDR, IDAT (flux zlib à blocs « stockés », RFC 1950/1951),
// IEND ; chaque bloc porte un CRC-32. Pas de compression : l'image reste simple et sûre.
#include "gamer_picture.h"

#include <array>

namespace {

constexpr uint8_t kSignature[] = {0x89, 'P', 'N', 'G', '\r', '\n', 0x1A, '\n'};
constexpr uint8_t kBackground[] = {0x10, 0x7C, 0x10};   // vert Xbox
constexpr uint8_t kDisc[] = {0xE6, 0xE6, 0xE6};
constexpr uint32_t kChannels = 3;
constexpr size_t kStoredBlockMax = 65535;
constexpr uint32_t kAdlerModulo = 65521;
constexpr uint32_t kSides[] = {64, 208, 424, 1080};     // XUserGamerPictureSize : Small … ExtraLarge

uint32_t Crc32(const uint8_t* data, size_t size) {
  static const std::array<uint32_t, 256> table = [] {
    std::array<uint32_t, 256> values{};
    for (uint32_t n = 0; n < 256; ++n) {
      uint32_t c = n;
      for (int k = 0; k < 8; ++k) c = (c & 1) ? 0xEDB88320u ^ (c >> 1) : c >> 1;
      values[n] = c;
    }
    return values;
  }();
  uint32_t crc = 0xFFFFFFFFu;
  for (size_t i = 0; i < size; ++i) crc = table[(crc ^ data[i]) & 0xFF] ^ (crc >> 8);
  return ~crc;
}

uint32_t Adler32(const std::vector<uint8_t>& data) {
  uint32_t a = 1, b = 0;
  for (uint8_t byte : data) {
    a = (a + byte) % kAdlerModulo;
    b = (b + a) % kAdlerModulo;
  }
  return (b << 16) | a;
}

void PutBigEndian(std::vector<uint8_t>& out, uint32_t value) {
  for (int shift = 24; shift >= 0; shift -= 8) out.push_back(static_cast<uint8_t>(value >> shift));
}

void PutChunk(std::vector<uint8_t>& out, const char type[4], const std::vector<uint8_t>& data) {
  PutBigEndian(out, static_cast<uint32_t>(data.size()));
  const size_t start = out.size();
  out.insert(out.end(), type, type + 4);
  out.insert(out.end(), data.begin(), data.end());
  PutBigEndian(out, Crc32(out.data() + start, out.size() - start));
}

// Lignes filtrées (octet de filtre 0) : disque centré sur fond uni.
std::vector<uint8_t> Scanlines(uint32_t side) {
  std::vector<uint8_t> raw;
  raw.reserve(static_cast<size_t>(side) * (1 + kChannels * side));
  const int64_t centre = side / 2, radius = side * 3 / 10;
  for (uint32_t y = 0; y < side; ++y) {
    raw.push_back(0);
    for (uint32_t x = 0; x < side; ++x) {
      const int64_t dx = static_cast<int64_t>(x) - centre, dy = static_cast<int64_t>(y) - centre;
      const uint8_t* colour = dx * dx + dy * dy <= radius * radius ? kDisc : kBackground;
      raw.insert(raw.end(), colour, colour + kChannels);
    }
  }
  return raw;
}

std::vector<uint8_t> ZlibStored(const std::vector<uint8_t>& raw) {
  std::vector<uint8_t> out = {0x78, 0x01};
  size_t offset = 0;
  do {
    const size_t length = raw.size() - offset < kStoredBlockMax ? raw.size() - offset : kStoredBlockMax;
    const bool last = offset + length >= raw.size();
    const uint16_t len = static_cast<uint16_t>(length), nlen = static_cast<uint16_t>(~len);
    out.push_back(last ? 1 : 0);
    out.insert(out.end(), {static_cast<uint8_t>(len), static_cast<uint8_t>(len >> 8),
                           static_cast<uint8_t>(nlen), static_cast<uint8_t>(nlen >> 8)});
    out.insert(out.end(), raw.begin() + offset, raw.begin() + offset + length);
    offset += length;
  } while (offset < raw.size());
  PutBigEndian(out, Adler32(raw));
  return out;
}

}  // namespace

namespace anyxboxx {

bool GamerPictureSide(uint32_t size_code, uint32_t* side) {
  if (side == nullptr || size_code >= sizeof kSides / sizeof kSides[0]) return false;
  *side = kSides[size_code];
  return true;
}

std::vector<uint8_t> GamerPicturePng(uint32_t side) {
  std::vector<uint8_t> header;
  PutBigEndian(header, side);
  PutBigEndian(header, side);
  header.insert(header.end(), {8, 2, 0, 0, 0});   // 8 bits, RGB, deflate, filtre 0, sans entrelacement
  std::vector<uint8_t> png(kSignature, kSignature + sizeof kSignature);
  PutChunk(png, "IHDR", header);
  PutChunk(png, "IDAT", ZlibStored(Scanlines(side)));
  PutChunk(png, "IEND", {});
  return png;
}

}  // namespace anyxboxx
