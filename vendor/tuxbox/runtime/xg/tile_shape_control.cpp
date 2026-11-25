// Differential guest for XGComputeTileShape, including null and aliased outputs.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <initializer_list>
struct Shape { uint32_t width, height, depth; };
struct Guarded { uint32_t before[4]; Shape shape; uint32_t after[4]; };
struct Result { int32_t hr; Guarded a, b; };
using Function = HRESULT (WINAPI*)(uint32_t,uint32_t,uint32_t,uint32_t,Shape*,Shape*);
static_assert(sizeof(Result) == 92);
int main() {
    HMODULE dll=LoadLibraryA("xg_subject.dll"); if (!dll) return 1;
    auto run=reinterpret_cast<Function>(GetProcAddress(dll,"XGComputeTileShape")); if (!run) return 2;
    FILE* out=nullptr; if (fopen_s(&out,"tile-results.bin","wb") || !out) return 3;
    unsigned cases=0;
    auto test=[&](uint32_t dim,uint32_t fmt,uint32_t bpe,uint32_t samples,unsigned mode) {
        Result r; std::memset(&r,0xa5,sizeof(r));
        Shape *p=(mode==1 || mode==3) ? nullptr : &r.a.shape;
        Shape *e=(mode==2 || mode==3) ? nullptr : (mode==4 ? &r.a.shape : &r.b.shape);
        r.hr=run(dim,fmt,bpe,samples,p,e);
        for (uint32_t v:r.a.before) if(v!=0xa5a5a5a5u) return false;
        for (uint32_t v:r.a.after) if(v!=0xa5a5a5a5u) return false;
        for (uint32_t v:r.b.before) if(v!=0xa5a5a5a5u) return false;
        for (uint32_t v:r.b.after) if(v!=0xa5a5a5a5u) return false;
        if(r.hr<0) {
            const Shape blank{0xa5a5a5a5u,0xa5a5a5a5u,0xa5a5a5a5u};
            if(std::memcmp(&r.a.shape,&blank,sizeof(blank)) || std::memcmp(&r.b.shape,&blank,sizeof(blank))) return false;
        }
        if (std::fwrite(&r,sizeof(r),1,out)!=1) return false;
        ++cases;return true;
    };
    for(uint32_t fmt=1;fmt<=120;++fmt)for(uint32_t bpe:{0u,3u,16u})for(uint32_t samples:{0u,1u,2u,4u,8u,16u})for(unsigned mode=0;mode<5;++mode)
        if(!test(3,fmt,bpe,samples,mode))return 4;
    for(uint32_t bpe:{0u,1u,2u,3u,4u,8u,16u})for(uint32_t samples:{0u,1u,2u,4u,8u,16u})for(unsigned mode=0;mode<5;++mode)
        if(!test(3,0,bpe,samples,mode))return 5;
    for(uint32_t fmt:{0u,28u})for(uint32_t bpe:{0u,1u,2u})for(uint32_t samples:{1u,2u})for(unsigned mode=0;mode<5;++mode)
        if(!test(1,fmt,bpe,samples,mode))return 6;
    for(uint32_t dim:{2u,4u})for(uint32_t samples:{1u,2u})for(unsigned mode=0;mode<5;++mode)
        if(!test(dim,0,1,samples,mode))return 7;
    for(uint32_t dim:{2u,4u})for(uint32_t fmt=1;fmt<=120;++fmt)for(uint32_t bpe:{0u,3u,16u})
        for(uint32_t samples:{1u,2u})for(unsigned mode=0;mode<5;++mode)
            if(!test(dim,fmt,bpe,samples,mode))return 9;
    if(std::fclose(out))return 8;
    std::printf("xg-tile PASS %u cases, nullable and aliased outputs\n",cases);return 0;
}
