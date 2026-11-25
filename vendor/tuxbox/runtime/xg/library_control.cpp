// Compare XG process state, lock ownership and layouts from packed image descriptors.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <array>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <initializer_list>
#include <vector>

struct ResourceDesc {
    UINT dimension; UINT64 alignment,width; UINT height; UINT16 slices,mips;
    UINT format,samples,quality,layout,flags;
};
struct Computer;
struct Methods {
    ULONG (WINAPI* add_ref)(Computer*);
    ULONG (WINAPI* release)(Computer*);
    HRESULT (WINAPI* layout)(Computer*, void*);
    UINT64 (WINAPI* bytes)(Computer*);
    UINT64 (WINAPI* alignment)(Computer*);
    UINT64 (WINAPI* mip_offset)(Computer*,UINT,UINT);
    UINT64 (WINAPI* address)(Computer*, UINT, UINT, UINT64, UINT, UINT, UINT, UINT*);
    HRESULT (WINAPI* coordinate)(Computer*,UINT64,UINT*,UINT*,UINT64*,UINT*,UINT*,UINT*);
    HRESULT (WINAPI* into)(Computer*, BYTE*, UINT, UINT, const BYTE*, UINT, UINT);
    HRESULT (WINAPI* from)(Computer*, BYTE*, UINT, UINT, UINT, const BYTE*, UINT, UINT);
    HRESULT (WINAPI* tiling)(Computer*,UINT*,void*,UINT*,UINT*,UINT,void*);
    HRESULT (WINAPI* image)(Computer*, UINT, UINT*);
    bool (WINAPI* sparse)(Computer*);

};
struct Computer { Methods* methods; };
using Create = HRESULT (WINAPI*)(const UINT*, Computer**);
struct LockCheck {
    HANDLE ready, done;
    Create create;
    std::array<UINT, 8> desc;
    HRESULT result;
};
static DWORD WINAPI try_factory(void* ptr) {
    auto& check = *static_cast<LockCheck*>(ptr);
    Computer* obj = nullptr;
    SetEvent(check.ready);
    check.result = check.create(check.desc.data(), &obj);
    if (obj) obj->methods->release(obj);
    SetEvent(check.done);
    return 0;
}

extern "C" {
__declspec(dllimport) HRESULT WINAPI XGInitializeLibrary();
__declspec(dllimport) HRESULT WINAPI XGTerminateLibrary();
__declspec(dllimport) void WINAPI XGSetHardwareVersion(UINT);
__declspec(dllimport) void WINAPI XGEnableExclusiveLocking(UINT);
__declspec(dllimport) BOOL WINAPI XGEnterExclusiveLock();
__declspec(dllimport) void WINAPI XGLeaveExclusiveLock();
__declspec(dllimport) HRESULT WINAPI XGCreateTextureComputer(const ResourceDesc*, Computer**);
__declspec(dllimport) HRESULT WINAPI XGCreateTextureComputerFromDescriptor(const UINT*, Computer**);
__declspec(dllimport) UINT WINAPI XGComputeOptimalDepthStencilSwizzleMode(UINT,UINT,UINT,UINT,UINT,UINT,UINT,UINT,UINT*);
}

static uint64_t result_hash = 14695981039346656037ull;
static FILE* dump;
static bool record(const void* data, size_t bytes) {
    auto ptr = static_cast<const BYTE*>(data);
    for (size_t i=0; i<bytes; ++i) result_hash = (result_hash ^ ptr[i]) * 1099511628211ull;
    return !dump || fwrite(data,bytes,1,dump)==1;
}

static bool check_copy(Computer* obj, const BYTE* layout, UINT m, UINT levels,
                       UINT array, UINT depth, UINT elements_x, UINT elements_y, UINT bpe, UINT plane=0) {
    UINT64 total{}, alignment{}, level_offset{}, plane_base{};
    std::memcpy(&total,layout,8);
    std::memcpy(&alignment,layout+8,8);
    const BYTE* plane_layout = layout+0x18+0x6c8*plane;
    std::memcpy(&plane_base,plane_layout+24,8);
    std::memcpy(&level_offset,plane_layout+0x38+0x70*m+8,8);
    auto v=obj->methods;
    if (v->bytes(obj)!=total || v->alignment(obj)!=alignment ||
        v->mip_offset(obj,plane,m)!=plane_base+level_offset || v->mip_offset(obj,plane,levels)!=UINT64_MAX) return false;
    const UINT row=elements_x*bpe+16, pitch=row*elements_y+32;
    std::vector<BYTE> src(pitch*depth,0xa5), dst(src.size(),0xa5);
    std::vector<BYTE> tiled(total+32,0xcd), expected=tiled;
    for (UINT z=0;z<depth;++z) for (UINT y=0;y<elements_y;++y) for (UINT x=0;x<elements_x;++x) {
        const auto offset=plane_base+v->address(obj,plane,m,x,y,array+z,0,nullptr);
        if (offset+bpe>total) return false;
        BYTE* pixel=src.data()+z*pitch+y*row+x*bpe;
        for (UINT b=0;b<bpe;++b) pixel[b]=static_cast<BYTE>(3+z*17+y*13+x*31+b*7);
        std::memcpy(expected.data()+offset,pixel,bpe);
    }
    const UINT sub=m+array*levels;
    if (v->into(obj,tiled.data(),plane,sub,src.data(),row,pitch)!=S_OK || tiled!=expected) return false;
    if (v->from(obj,dst.data(),plane,sub,0,tiled.data()+plane_base,row,pitch)!=S_OK || dst!=src) return false;
    return true;
}

static bool check_tiling(Computer* obj) {
    const auto sparse=obj->methods->sparse(obj);
    if (!record(&sparse,sizeof(sparse))) return false;
    for (UINT first : {0u,1u,4u}) for (UINT capacity : {0u,1u,8u}) {
        UINT count=capacity, total=0x5a5a5a5a, packed[3], shape[3], tilings[24];
        std::memset(packed,0x5a,sizeof(packed));
        std::memset(shape,0x5a,sizeof(shape));
        std::memset(tilings,0x5a,sizeof(tilings));
        const HRESULT hr=obj->methods->tiling(obj,&total,packed,shape,&count,first,tilings);
        if (!record(&hr,sizeof(hr)) || !record(&count,sizeof(count)) || !record(&total,sizeof(total)) ||
            !record(packed,sizeof(packed)) || !record(shape,sizeof(shape)) || !record(tilings,sizeof(tilings))) return false;
        count=capacity;
        if (obj->methods->tiling(obj,nullptr,nullptr,nullptr,&count,first,nullptr)!=hr ||
            !record(&count,sizeof(count)) || obj->methods->tiling(obj,nullptr,nullptr,nullptr,nullptr,first,nullptr)!=hr) return false;
    }
    return true;
}

static bool check_coordinates(Computer* obj) {
    auto v = obj->methods;
    const UINT64 size = v->bytes(obj);
    for (UINT i=0;i<65;++i) {
        const auto pos = i==64 ? size : size/64*i+(i&3);
        UINT coords[5] = {17, 19, 23, 29, 31};
        UINT64 x = 0x1234567890ull;
        const HRESULT hr = v->coordinate(obj,pos,coords,coords+1,&x,coords+2,coords+3,coords+4);
        if (v->coordinate(obj,pos,nullptr,nullptr,nullptr,nullptr,nullptr,nullptr)!=hr ||
            !record(&hr,sizeof(hr)) || !record(&x,sizeof(x)) || !record(coords,sizeof(coords))) return false;
    }
    return true;
}

static bool check_image(Computer* obj, UINT plane) {
    std::array<UINT,8> words;
    words.fill(0xa5a5a5a5);
    const auto hr=obj->methods->image(obj,plane,words.data());
    return record(&hr,sizeof(hr)) && record(words.data(),sizeof(words));
}

static bool is_uncompressed(UINT fmt) {
    return fmt<=4 || (fmt>=9 && fmt<=18) || (fmt>=23 && fmt<=39) ||
        (fmt>=41 && fmt<=43) || (fmt>=48 && fmt<=54) || (fmt>=56 && fmt<=65) ||
        fmt==87 || fmt==88 || (fmt>=90 && fmt<=93);
}

static bool check_depth_samples(Computer* obj, const ResourceDesc& desc, const BYTE* layout, UINT plane) {
    if (!check_image(obj, plane)) return false;
    for (UINT sample = 1; sample <= desc.samples; ++sample) {
        UINT bit = 73;
        const auto offset = obj->methods->address(obj, plane, 0, 3, 2, 0, sample, &bit);
        if (!record(&offset, sizeof(offset)) || !record(&bit, sizeof(bit))) return false;
    }
    if (desc.width != 17) return true;
    const BYTE* field = layout + 0x18 + plane * 0x6c8;
    UINT bytes{};
    std::memcpy(&bytes, field + 40, sizeof(bytes));
    if (!bytes) return true; // HTILE has no element copies.
    if (!check_copy(obj, layout, 0, 1, 0, 1, 17, desc.height, bytes, plane)) return false;
    const UINT row = 17 * bytes + 8, pitch = row * desc.height;
    UINT64 size{};
    std::memcpy(&size, field + 8, sizeof(size));
    std::vector<BYTE> tiled(static_cast<size_t>(size), 0xcd), expected(pitch, 0xa5), actual(expected);
    for (UINT sample = 0; sample < desc.samples; ++sample) {
        std::fill(actual.begin(), actual.end(), static_cast<BYTE>(0xa5));
        for (UINT y = 0; y < desc.height; ++y) for (UINT x = 0; x < 17; ++x) {
            const UINT64 at = obj->methods->address(obj, plane, 0, x, y, 0, 0, nullptr) + bytes * sample;
            if (at > size || bytes > size - at) return false;
            for (UINT b = 0; b < bytes; ++b)
                tiled[at + b] = expected[y * row + x * bytes + b] = static_cast<BYTE>(x + 17*y + 51*sample + b);
        }
        if (obj->methods->from(obj, actual.data(), plane, 0, sample, tiled.data(), row, pitch) != S_OK || actual != expected) return false;
    }
    return true;
}

static bool check_large_surface(const ResourceDesc& desc, UINT64 metadata_bytes=0, UINT64 expected_bit=0) {
    Computer* obj=nullptr;
    if (XGCreateTextureComputer(&desc,&obj)!=S_OK || !obj) return false;
    std::array<BYTE,0x1b40> layout{};
    if (obj->methods->layout(obj,layout.data())!=S_OK || !record(layout.data(),layout.size())) return false;
    UINT planes{};
    std::memcpy(&planes,layout.data()+20,sizeof(planes));
    if (!planes || planes>4) return false;
    if (metadata_bytes) {
        UINT64 size{};
        std::memcpy(&size,layout.data()+0x18+0x6c8+8,sizeof(size));
        UINT bit=0;
        const UINT64 offset=obj->methods->address(obj,1,0,1024,0,0,0,&bit);
        if (size!=metadata_bytes || offset*8+bit!=expected_bit) return false;
    }
    for (UINT plane=0;plane<planes;++plane) {
        uint64_t hash=14695981039346656037ull;
        for (UINT i=0;i<800;++i) {
            UINT bit=0;
            const UINT64 offset=obj->methods->address(obj,plane,0,(i*53+15)%desc.width,
                                                      (i*101+3)%desc.height,i%desc.slices,0,&bit);
            if (offset==UINT64_MAX) return false;
            hash=(hash^(offset*8+bit))*1099511628211ull;
        }
        if (desc.samples > 1 && !check_depth_samples(obj, desc, layout.data(), plane)) return false;
        if (!record(&hash,sizeof(hash))) return false;
    }
    return obj->methods->release(obj)==0;
}

static bool check_map_targets() {
    for (UINT fmt=1;fmt<=99;++fmt) {
        if (!is_uncompressed(fmt)) continue;
        for (UINT flags : {1u,5u,0x100001u,0x100005u,0x200001u,0x200005u,
                           0x300001u,0x300005u,0xc00001u,0xc00005u}) {
            for (UINT width : {4096u,16384u}) {
                ResourceDesc desc{3,0,width,width==4096 ? 4096u : 8193u,1,1,fmt,1,0,0,flags};
                if (!check_large_surface(desc)) return false;
            }
        }
    }
    for (UINT fmt : {57u,61u}) {
        ResourceDesc desc{3,0,2560,1440,1,1,fmt,1,0,0,0xc00005};
        if (!check_large_surface(desc,fmt==57 ? 49152 : 32768,fmt==57 ? 65536 : 1024)) return false;
    }
    for (UINT fmt : {19u,20u,39u,40u,44u,45u,53u,55u}) {
        for (UINT flags : {0x1000002u,0x1000006u,0x81300002u,0x100000au}) {
            for (UINT width : {160u,1024u,2560u,4096u,16384u}) for (UINT height : {90u,1440u}) {
                ResourceDesc desc{3,0,width,height,1,1,fmt,1,0,0,flags};
                if (!check_large_surface(desc)) return false;
            }
        }
    }
    for (UINT flags : {0x1000002u,0x81300002u}) {
        for (UINT fmt : {19u,39u,53u}) {
            for (UINT slices : {6u,17u}) {
                ResourceDesc desc{3,0,1024,1024,static_cast<UINT16>(slices),1,fmt,1,0,0,flags};
                if (!check_large_surface(desc)) return false;
            }
        }
    }
    struct Shape { UINT width, height; UINT16 slices; };
    const Shape surfaces[] = {{17,13,1}, {257,144,6}, {2976,1676,1}, {3840,2160,1}};
    for (const auto& surface : surfaces) for (UINT fmt : {19u,20u,39u,40u,44u,45u,53u,55u})
        for (UINT samples : {2u,4u,8u}) for (UINT flags : {0x1000002u,0x1000006u,0x100000au,0x81300002u}) {
            ResourceDesc desc{3,0,surface.width,surface.height,surface.slices,1,fmt,samples,0,0,flags};
            if (!check_large_surface(desc)) return false;
        }
    return true;
}

static unsigned check_typed_images() {
    unsigned count=0;
    for (UINT fmt=1;fmt<=99;++fmt) {
        const bool uncompressed=is_uncompressed(fmt);
        if (!uncompressed && !(fmt>=70 && fmt<=84) && fmt<94) continue;
        for (UINT dimension : {2u,3u,4u}) for (UINT flags : {0u,1u,4u,0x100001u,0xc00001u}) {
            if ((!uncompressed && (dimension!=3 || (flags && flags!=4))) ||
                (dimension!=3 && flags)) continue;
            for (UINT levels : {1u,3u}) {
                if (flags==0xc00001u && levels>1) continue;
                ResourceDesc desc{dimension,0,128,dimension==2 ? 1u : 64u,1,
                    static_cast<UINT16>(levels),fmt,1,0,dimension==4 ? 256u : 0u,flags};
                Computer* obj=nullptr;
                if (XGCreateTextureComputer(&desc,&obj)!=S_OK || !obj) return 0;
                for (UINT p=0;p<3;++p) if (!check_image(obj,p)) return 0;
                if (!check_tiling(obj) || (fmt==28 && !check_coordinates(obj))) return 0;
                if (obj->methods->release(obj)!=0) return 0;
                ++count;
            }
        }
    }
    for (UINT format : {61u,49u,28u,10u,2u}) for (UINT swizzle : {10u,17u,18u}) {
        ResourceDesc desc{3,0,513,129,3,1,format,1,0,256+swizzle,0};
        Computer* obj = nullptr;
        if (XGCreateTextureComputer(&desc,&obj)!=S_OK || !obj) return 0;
        if (!check_image(obj,0) || !check_tiling(obj) || obj->methods->release(obj)!=0) return 0;
        ++count;
    }
    for (UINT hw : {3u,4u}) {
        XGSetHardwareVersion(hw);
        for (UINT format : {61u,49u,28u,10u,2u}) for (UINT slices : {1u,3u}) {
            ResourceDesc desc{3,0,512,1024,static_cast<UINT16>(slices),11,format,1,0,0,0x100001};
            Computer* obj=nullptr;
            if (XGCreateTextureComputer(&desc,&obj)!=S_OK || !obj) return 0;
            std::array<BYTE,0x1b40> layout{};
            if (obj->methods->layout(obj,layout.data())!=S_OK || !record(layout.data(),layout.size()) ||
                !check_image(obj,0) || !check_tiling(obj)) return 0;
            for (UINT m=0;m<desc.mips;++m) for (UINT slice=0;slice<slices;++slice) {
                const UINT w=(512u>>m)?(512u>>m):1, h=1024u>>m;
                for (UINT i=0;i<32;++i) {
                    const UINT64 addr=obj->methods->address(obj,0,m,(i*37u+1u)%w,(i*23u+5u)%h,slice,0,nullptr);
                    if (!record(&addr,sizeof(addr))) return 0;
                }
            }
            if (obj->methods->release(obj)!=0) return 0;
            ++count;
        }
    }
    for (UINT hw : {3u,4u}) {
        XGSetHardwareVersion(hw);
        for (UINT fmt : {61u,49u,28u,10u,2u}) for (UINT flags : {1u,0xc00001u}) {
            ResourceDesc desc{3,0,1024,513,8,1,fmt,1,0,0,flags};
            Computer* obj=nullptr;
            if (XGCreateTextureComputer(&desc,&obj)!=S_OK || !obj) return 0;
            std::array<BYTE,0x1b40> layout{};
            if (obj->methods->layout(obj,layout.data())!=S_OK || !record(layout.data(),layout.size()) ||
                !check_image(obj,1) || !check_tiling(obj)) return 0;
            uint64_t hash=14695981039346656037ull;
            for (UINT slice=0;slice<desc.slices;++slice) for (UINT y=0;y<desc.height;y+=11)
                for (UINT x=0;x<desc.width;x+=13) {
                    UINT bit=~0u;
                    auto offset=obj->methods->address(obj,1,0,x,y,slice,0,&bit);
                    hash=(hash^(offset*8+bit))*1099511628211ull;
                }
            if (!record(&hash,sizeof(hash)) || obj->methods->release(obj)!=0) return 0;
            ++count;
        }
    }
    XGSetHardwareVersion(3);
    return count;
}

#include "color_msaa_control.h"

int wmain(int argc, wchar_t** argv) {
    if (argc == 2 && !wcscmp(argv[1], L"--color-msaa")) {
        if (!check_color_msaa() || result_hash != 0x8b6aadde5f4cd7c1ull) return 36;
        std::printf("xg-color-msaa 36 layouts %016llx\n", result_hash);
        return 0;
    }
    const bool report = argc==3 && !wcscmp(argv[1],L"--report");
    if (!report && argc!=1) return 2;
    auto init = XGInitializeLibrary;
    auto term = XGTerminateLibrary;
    auto set_hw = XGSetHardwareVersion;
    auto enable = XGEnableExclusiveLocking;
    auto enter = XGEnterExclusiveLock;
    auto leave = XGLeaveExclusiveLock;
    auto create = XGCreateTextureComputerFromDescriptor;
    auto depth = XGComputeOptimalDepthStencilSwizzleMode;
    if (term() != S_FALSE || init() != S_OK || init() != S_FALSE ||
        term() != S_OK || term() != S_FALSE) return 4;
    LockCheck check{CreateEventW(nullptr, TRUE, FALSE, nullptr), CreateEventW(nullptr, TRUE, FALSE, nullptr),
                    create, {0, 56u << 20, 0, 9u << 28, 0, 0, 0, 0}, E_FAIL};
    if (!check.ready || !check.done || enter()) return 5;
    enable(1);
    if (!enter() || !enter()) return 6;
    auto worker = CreateThread(nullptr, 0, try_factory, &check, 0, nullptr);
    if (!worker || WaitForSingleObject(check.ready, 5000) != WAIT_OBJECT_0) return 7;
    leave();
    if (WaitForSingleObject(check.done, 75) != WAIT_TIMEOUT) return 8;
    enable(0);
    leave();
    if (WaitForSingleObject(worker, 5000) != WAIT_OBJECT_0 || check.result != S_OK ||
        enter() || init() != S_FALSE || term() != S_OK) return 9;
    CloseHandle(worker);
    CloseHandle(check.ready);
    CloseHandle(check.done);

    if (!report && fopen_s(&dump, "xg-library.bin", "wb")) return 10;
    for (UINT format = 0; format <= 128; ++format) {
        for (UINT flags = 0; flags < 4; ++flags) {
            UINT out = 0x235;
            UINT mode = depth(format, 37, 13, 1, 1, 4, flags & 1, flags >> 1, &out);
            if (out != mode || mode != depth(format, 1, 1, 1, 1, 1, flags & 1, flags >> 1, nullptr))
                return 11;
            if (!record(&mode,sizeof(mode))) return 12;
        }
    }
    unsigned records = 0;
    for (UINT hw : {3u,4u,3u}) {
        set_hw(hw);
        if (init() != S_FALSE) return 13;
        for (UINT type : {8u,9u,10u,12u,13u}) for (UINT sw : {0u,5u,10u,17u,18u,27u}) {
            for (UINT format : {1u,14u,56u,65u,75u}) for (UINT mips : {1u,3u}) {
                if ((type == 8 || type == 12) && sw != 0) continue;
                if (((sw == 10 || sw == 17 || sw == 18) && (type == 10 || mips > 1)) || (sw == 27 && type == 10) || (type == 10 && sw == 5 && mips > 1)) continue;
                UINT w = 128, h = type == 8 || type == 12 ? 1:64;
                UINT slices = type == 12 || type == 13 || type == 10 ? 3:1;
                const UINT desc[] = {0, (format << 20) | (((w-1)&3) << 30),
                    ((w-1)>>2) | ((h-1)<<14), (type<<28) | (sw<<20) | ((mips-1)<<16),
                    slices-1, (mips-1)<<4, 0, 0};
                Computer* obj = nullptr;
                auto status = create(desc, &obj);
                if (status != S_OK || !obj) {
                    std::fprintf(stderr,"raw descriptor failed %u %u %u %u %u 0x%lx\n",hw,type,sw,format,mips,status);
                    return 14;
                }
                alignas(8) std::array<BYTE, 0x1b40> layout{};
                if (obj->methods->layout(obj, layout.data()) != S_OK ||
                    !record(layout.data(),layout.size())) return 15;
                for (UINT m=0; m<mips; ++m) for (UINT i=0; i<8; ++i) {
                    const UINT x=(19*i+3)%(w>>m), y=(13*i+1)%((h>>m)?(h>>m):1);
                    const UINT slice = type==10 ? 0 : i%slices;
                    const auto addr=obj->methods->address(obj,0,m,x,y,slice,0,nullptr);
                    if (!record(&addr,sizeof(addr))) return 16;
                }
                const UINT bpe = format==1 ? 1 : format==14 ? 2 : format==56 ? 4 : format==65 ? 8 : 16;
                for (UINT m=0; m<mips; ++m) {
                    const UINT mip_depth = type==10 ? ((slices>>m)?(slices>>m):1) : 1;
                    if (!check_copy(obj,layout.data(),m,mips,type==10?0:slices-1,mip_depth,
                                    w>>m, (h>>m)?(h>>m):1,bpe)) {
                        std::fprintf(stderr,"copy mismatch hw=%u type=%u sw=%u format=%u mip=%u\n",hw,type,sw,format,m);
                        return 21;
                    }
                }
                UINT descriptor[8]{};
                if (!check_tiling(obj) || !check_coordinates(obj) || obj->methods->image(obj,0,descriptor)!=S_OK ||
                    std::memcmp(desc,descriptor,sizeof(desc)) || !check_image(obj,1)) return 28;
                if (obj->methods->release(obj) != 0) return 17;
                ++records;
            }
        }
    }
    unsigned depth_surfaces = 0;
    for (UINT flags : {0x01000006u,0x81300002u}) {
        for (UINT width : {1u,64u,257u,1024u}) for (UINT height : {127u,1024u}) {
            ResourceDesc desc{3,0,width,height,1,1,19,1,0,0,flags};
            Computer* obj = nullptr;
            if (XGCreateTextureComputer(&desc,&obj) != S_OK || !obj) return 22;
            std::array<BYTE,0x1b40> layout{};
            if (obj->methods->layout(obj,layout.data()) != S_OK || !record(layout.data(),layout.size())) return 23;
            const UINT has_htile = flags == 0x01000006u;
            for (UINT plane=0;plane<2+has_htile;++plane) {
                if (!check_image(obj,plane)) return 29;
                uint64_t address_hash = 14695981039346656037ull;
                for (UINT y=0;y<height;++y) for (UINT x=0;x<width;++x) {
                    UINT bit = UINT_MAX;
                    const UINT64 addr = obj->methods->address(obj,plane,0,x,y,0,0,&bit);
                    if (addr==UINT64_MAX || bit) return 24;
                    address_hash = (address_hash ^ addr) * 1099511628211ull;
                }
                if (!record(&address_hash,sizeof(address_hash))) return 25;
                if (plane>=has_htile && !check_copy(obj,layout.data(),0,1,0,1,width,height,
                                                    plane==has_htile ? 4 : 1,plane)) return 26;
            }
            if (!check_tiling(obj) || !check_coordinates(obj) || obj->methods->release(obj)!=0) return 27;
            ++depth_surfaces;
        }
    }
    const UINT typed_images=check_typed_images();
    if (!typed_images) return 30;
    // Streamed color textures first seen during the Construct map load.
    for (UINT fmt : {28u,71u,77u,83u,97u,98u}) {
        for (UINT flags : {0u,4u,0x100000u,0x200000u,0x300000u,0x300004u}) {
            for (UINT width : {257u,4096u}) for (UINT levels : {1u,4u}) {
                ResourceDesc desc{3,0,width,width==257 ? 129u : 4096u,1,
                    static_cast<UINT16>(levels),fmt,1,0,0,flags};
                Computer* obj=nullptr;
                if (XGCreateTextureComputer(&desc,&obj)!=S_OK || !obj) return 31;
                std::array<BYTE,0x1b40> layout{};
                if (obj->methods->layout(obj,layout.data())!=S_OK || !record(layout.data(),layout.size())) return 32;
                const UINT block=fmt==28 ? 1 : 4;
                for (UINT level=0;level<levels;++level) {
                    UINT w=((width>>level)+block-1)/block, h=((desc.height>>level)+block-1)/block;
                    for (UINT i=0;i<32;++i) {
                        UINT64 addr=obj->methods->address(obj,0,level,(i*53u+7)%w,(i*29u+3)%h,0,0,nullptr);
                        if (!record(&addr,sizeof(addr))) return 33;
                    }
                }
                if (obj->methods->release(obj)!=0) return 34;
            }
        }
    }
    if (!check_map_targets()) return 35;
    if (dump && fclose(dump)) return 18;
    if (result_hash != 0x6df5bf4b600049faull) { std::fprintf(stderr,"XG library hash %016llx\n",result_hash); return 19; }
    FILE* out = stdout;
    if (report && _wfopen_s(&out,argv[2],L"wb")) return 20;
    std::fprintf(out,"xg-library PASS depth=516 descriptors=%u lock=recursive+factory copies=bidirectional depth_surfaces=%u typed_images=%u map_targets=1474 msaa_depth=384\n",records,depth_surfaces,typed_images);
    return report && fclose(out);
}
