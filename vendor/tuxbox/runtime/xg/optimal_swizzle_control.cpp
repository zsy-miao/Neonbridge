// Differential guest for the supplied Xbox XG swizzle-selection export.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <cstdint>
#include <cstdio>
#include <initializer_list>
using Function = uint32_t (WINAPI*)(uint32_t,uint32_t,uint32_t,uint32_t,uint32_t,uint32_t,uint32_t,uint32_t);
int main() {
    HMODULE dll=LoadLibraryA("xg_subject.dll"); if (!dll) return 1;
    auto select=reinterpret_cast<Function>(GetProcAddress(dll,"XGComputeOptimalSwizzleMode")); if(!select) return 2;
    // The supplied Xbox XGS calls this with eight arguments. Verify directly
    // that changing only the last one changes the result in both branches.
    if(select(3,28,37,13,3,1,8,0) != 5 || select(3,28,37,13,3,1,8,0x08000000u) != 10 ||
       select(3,19,37,13,3,1,64,0) != 24 || select(3,19,37,13,3,1,64,0x8000u) != UINT32_MAX) return 6;
    FILE* out=nullptr; if(fopen_s(&out,"optimal-swizzle-results.bin","wb") || !out) return 3;
    unsigned cases=0;
    for(uint32_t dim:{0u,1u,2u,3u,4u,5u})for(uint32_t format_case=0;format_case<130;++format_case)
    for(uint32_t bind:{0u,8u,32u,64u,96u,0xffffffffu})
    for(uint32_t flags:{0u,0x8000u,0x08000000u,0x08008000u,0xffffffffu})
    for(uint32_t shape:{0u,1u,2u}) {
        uint32_t format=format_case<128 ? format_case : format_case==128 ? 0x80000000u : 0xffffffffu;
        uint32_t w=shape==0?0:shape==1?37:0xffffffffu;
        uint32_t h=shape==0?0:shape==1?13:0xffffffffu;
        uint32_t depth=shape==0?0:shape==1?3:0xffffffffu;
        uint32_t samples=shape==0?0:shape==1?1:0xffffffffu;
        uint32_t result=select(dim,format,w,h,depth,samples,bind,flags);
        if(std::fwrite(&result,sizeof(result),1,out)!=1)return 4;
        ++cases;
    }
    if(std::fclose(out))return 5;
    std::printf("xg-optimal-swizzle PASS %u cases\n",cases);return 0;
}
