#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <array>
#include <cstdio>
#include <cstring>

using Block = std::array<BYTE, 256>;
using Encode = HRESULT (WINAPI*)(UINT, const Block*, Block*, BYTE*, UINT*, UINT, UINT, UINT, const UINT*);
using Decode = HRESULT (WINAPI*)(UINT, const Block*, const BYTE*, Block*, UINT, UINT, UINT, const UINT*);

struct Codec { Encode encode; Decode decode; };
extern "C" __declspec(dllimport) HRESULT WINAPI XGEncodeDCC(
    UINT, const Block*, Block*, BYTE*, UINT*, UINT, UINT, UINT, const UINT*);
extern "C" __declspec(dllimport) HRESULT WINAPI XGDecodeDCC(
    UINT, const Block*, const BYTE*, Block*, UINT, UINT, UINT, const UINT*);

static bool stored_block_control() {
    // The original Scarlett encoder uses a single 32-byte chunk for this block.
    // Its first two header bits select the green bases in each half-sector.
    const BYTE bytes[] = {0x82,0xff,0xc3,0xff,0xc3,0xff,0xc3,0xff,
                          1,1,2,3,2,3,4,5,4,5,6,7,6,7};
    const UINT settings[]{0,0};
    Block encoded{}, expected{}, expanded{};
    std::memcpy(encoded.data(), bytes, sizeof(bytes));
    for (UINT i = 0; i < expected.size(); ++i) expected[i] = static_cast<BYTE>(i / 32);
    BYTE key = 8;
    if (XGDecodeDCC(1,&encoded,&key,&expanded,28,4,0,settings) != S_OK || expanded != expected) return false;
    UINT size = 0;
    if (XGEncodeDCC(1,&expected,&encoded,&key,&size,28,4,0,settings) != S_OK ||
        key != 8 || size != 32 || std::memcmp(encoded.data(), bytes, sizeof(bytes))) return false;
    const BYTE planes[] = {0x01,0x00,0x02,0x10,0xaa,0x10,0xaa,0x11,0xaa,0x10,0xaa,
                           0x10,0xaa,0x10,0xaa,0x11,0xaa,0x10,0xaa};
    for (UINT i = 0; i < encoded.size(); i += 64) std::memcpy(encoded.data()+i, planes, sizeof(planes));
    for (UINT i = 0; i < expected.size(); ++i) expected[i] = static_cast<BYTE>(i % 2);
    key = 0x55;
    if (XGDecodeDCC(1,&encoded,&key,&expanded,61,4,0,settings) != S_OK || expanded != expected) return false;
    // An unsupported second block must leave the entire output batch untouched.
    const BYTE keys[]{0xff,8};
    std::array<Block,2> in{}, out{};
    out[0].fill(0x43);
    out[1].fill(0xab);
    const auto before = out;
    return XGDecodeDCC(2,in.data(),keys,out.data(),28,4,0,settings) == E_NOTIMPL && out == before;
}
static Codec load(const char* name) {
    auto m = LoadLibraryA(name);
    return {reinterpret_cast<Encode>(GetProcAddress(m, "XGEncodeDCC")),
            reinterpret_cast<Decode>(GetProcAddress(m, "XGDecodeDCC"))};
}

int main(int argc, char** argv) {
    if (argc == 3 && std::strcmp(argv[1],"--report") == 0) {
        if (!stored_block_control()) return 1;
        FILE* file = nullptr;
        if (fopen_s(&file,argv[2],"wb")) return 2;
        std::fputs("xg-dcc PASS xbox-blocks=2 encode-bytes=32 atomic-batch=1\n",file);
        return std::fclose(file) != 0;
    }
    const auto xbox = load("xg_reference.dll");
    const Codec tux{XGEncodeDCC, XGDecodeDCC};
    if (!xbox.encode || !xbox.decode || !tux.encode || !tux.decode) return 2;
    UINT seed = 55987, count = 0;
    for (UINT fmt : {28u,29u,30u,31u,32u,49u,50u,51u,52u,61u,62u,63u,64u,65u,87u,88u,91u,93u})
    for (UINT sw = 0; sw < 32; sw += 4)
    for (UINT independent : {0u, 1u}) for (UINT sample = 0; sample < 16; ++sample) {
        UINT settings[]{0, independent};
        UINT channels = fmt >= 61 && fmt <= 65 ? 1 : fmt >= 49 && fmt <= 52 ? 2 : 4;
        std::array<Block, 5> raw{}, packed{}, result{};
        BYTE keys[5]; UINT sizes[5];
        for (UINT i = 0; i < 256; ++i) {
            raw[0][i] = static_cast<BYTE>(sample);
            raw[1][i] = static_cast<BYTE>((i / 64 + sample + 1) * (i % channels));
            seed = seed * 1664525 + 1013904223;
            raw[2][i] = static_cast<BYTE>(seed >> 24);
            raw[3][i] = (sample & (1u << (i/64))) ? raw[2][i] : raw[1][i];
            const auto channel = i % channels;
            raw[4][i] = static_cast<BYTE>((sample & (1u << channel)) ?
                        (i / 32 + 1) * (channel + 1) : 3 * channel);
        }
        for (UINT direction = 0; direction < 2; ++direction) {
            auto encode = direction ? tux.encode : xbox.encode;
            auto decode = direction ? xbox.decode : tux.decode;
            if (encode(5, raw.data(), packed.data(), keys, sizes, fmt, sw, 0, settings) != S_OK ||
                sizes[0] != 128 / channels || sizes[1] != 128 / channels || sizes[2] != 256 ||
                decode(5, packed.data(), keys, result.data(), fmt, sw, 0, settings) != S_OK ||
                result != raw) {
                std::fprintf(stderr, "format %u swizzle %u config %u sample %u direction %u keys %02x/%02x/%02x sizes %u/%u/%u\n",
                             fmt, sw, independent, sample, direction, keys[0], keys[1], keys[2], sizes[0], sizes[1], sizes[2]);
                return 1;
            }
            count += 5;
        }
    }
    const UINT formats[] = {2,3,4,10,11,12,13,14,16,17,18,24,25,26,28,29,30,31,32,34,35,36,37,38,41,42,43,
                            49,50,51,52,54,56,57,58,59,61,62,63,64,65,87,88,91,93};
    UINT clear_checks = 0;
    for (UINT fmt : formats) for (UINT sw = 0; sw < 32; sw += 4)
    for (UINT block_size : {0u,1u,2u}) for (UINT independent : {0u,1u}) for (UINT dimension : {0u,3u,4u}) {
        UINT settings[]{block_size, independent};
        const std::array<BYTE, 6> keys{0,0x40,0x80,0xc0,0x10,0xff};
        std::array<Block, 6> data{}, expected{}, actual{};
        for (auto& block : data) for (auto& b : block) {
            seed = seed * 1664525 + 1013904223;
            b = static_cast<BYTE>(seed >> 24);
        }
        if (xbox.decode(6, data.data(), keys.data(), expected.data(), fmt, sw, dimension, settings) != S_OK ||
            tux.decode(6, data.data(), keys.data(), actual.data(), fmt, sw, dimension, settings) != S_OK ||
            expected != actual) {
            for (UINT i=0;i<6;++i) if(expected[i]!=actual[i])
                std::fprintf(stderr,"clear mismatch key %02x format %u sw %u dimension %u config %u/%u\n",keys[i],fmt,sw,dimension,block_size,independent);
            return 1;
        }
        clear_checks += 6;
        // Includes constant non-clear colors and random data. Re-encode
        // decoded blocks, then let the original DLL consume the results.
        std::array<Block, 6> packed{};
        std::array<BYTE, 6> out_keys{};
        if (tux.encode(6, actual.data(), packed.data(), out_keys.data(), nullptr, fmt, sw, dimension, settings) != S_OK ||
            xbox.decode(6, packed.data(), out_keys.data(), actual.data(), fmt, sw, dimension, settings) != S_OK ||
            expected != actual) return 1;
        count += 6;
    }
    UINT delta_checks = 0;
    for (UINT channels : {1u,2u,4u}) for (UINT independent : {0u,1u}) for (UINT dimension : {0u,3u,4u})
    for (UINT block_size : {0u,1u,2u}) for (UINT base : {0u,22u,73u,135u,254u}) for (UINT bits = 0; bits < 7; ++bits)
    for (UINT sample = 0; sample < 32; ++sample) {
        Block raw{}, packed{}, expanded{};
        const UINT fmt = channels == 1 ? 61 : channels == 2 ? 49 : 28;
        const UINT settings[]{block_size,independent};
        for (UINT i = 0; i < raw.size(); ++i) {
            const UINT x = i / channels, channel = i % channels;
            seed = seed * 1664525u + 1013904223u;
            const UINT v = sample & 1 ? seed >> 24 : sample & 2 ? x :
                           sample & 4 ? x % 8 == ((sample >> 3) & 3) : x >> 2;
            const UINT offset = (sample & 8) && channel ? 23 * channel : base;
            raw[i] = static_cast<BYTE>(offset + (channel <= (sample >> 3) ? v & ((1u << bits) - 1) : 0));
        }
        for (UINT direction = 0; direction < 2; ++direction) {
            const auto encode = direction ? tux.encode : xbox.encode;
            const auto decode = direction ? xbox.decode : tux.decode;
            BYTE key = 0;
            if (encode(1,&raw,&packed,&key,nullptr,fmt,4,dimension,settings) != S_OK) return 1;
            const HRESULT hr = decode(1,&packed,&key,&expanded,fmt,4,dimension,settings);
            if (hr != S_OK || expanded != raw) {
                std::fprintf(stderr,"delta fmt %u config %u base %u bits %u sample %u direction %u key %02x\n",
                             fmt,independent,base,bits,sample,direction,key);
                return 1;
            }
            ++delta_checks;
        }
    }
    std::printf("dcc PASS %u blocks cross-decoded, %u clear/raw blocks matched, %u delta controls\n",
                count + delta_checks, clear_checks, delta_checks);
}
