// Replay independently supplied resource descriptors against an XG provider.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <cstdint>
#include <cstdio>
#include <cstring>
struct Desc { uint32_t dimension; uint64_t alignment,width; uint32_t height; uint16_t slices,mips; uint32_t format,samples,quality,layout,flags; };
struct Comp;
struct Methods { ULONG (WINAPI* add_ref)(Comp*); ULONG (WINAPI* release)(Comp*); HRESULT (WINAPI* layout)(Comp*,void*); void* other[3]; uint64_t (WINAPI* offset)(Comp*,uint32_t,uint32_t,uint64_t,uint32_t,uint32_t,uint32_t,uint32_t*); };
struct Comp { const Methods* methods; };
struct Result { HRESULT hr; uint8_t body[0x1b40]; uint64_t addresses[2]; uint32_t bits[2]; };
using Create = HRESULT (WINAPI*)(const Desc*,Comp**);
static_assert(sizeof(Desc)==56);
int main() {
    HMODULE dll=LoadLibraryA("xg_subject.dll");
    auto create=dll ? reinterpret_cast<Create>(GetProcAddress(dll,"XGCreateTextureComputer")) : nullptr;
    if(!create) return 1;
    FILE *input=nullptr,*output=nullptr;
    if(fopen_s(&input,"descriptors.bin","rb") || fopen_s(&output,"results.bin","wb")) return 2;
    Desc desc{}; unsigned count=0;
    while(fread(&desc,sizeof(desc),1,input)==1) {
        Result result{}; Comp* computer=nullptr; result.hr=create(&desc,&computer);
        if(result.hr==S_OK && computer) {
            if(computer->methods->layout(computer,result.body)!=S_OK) return 3;
            result.addresses[0]=computer->methods->offset(computer,0,0,0,0,0,0,&result.bits[0]);
            result.addresses[1]=computer->methods->offset(computer,0,desc.mips-1,0,0,desc.slices-1,0,&result.bits[1]);
            if(computer->methods->release(computer)!=0) return 4;
        }
        if(fwrite(&result,sizeof(result),1,output)!=1) return 5;
        count++;
    }
    fclose(input);fclose(output);printf("xg-trace-replay %u descriptors record-size=%zu\n",count,sizeof(Result));
}
