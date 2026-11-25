#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct {
    uint32_t dimension;
    uint64_t alignment, width;
    uint32_t height;
    uint16_t slices, mips;
    uint32_t format, samples, quality, layout, flags;
} Desc;
typedef HRESULT (WINAPI *Create)(const Desc *, void **);
typedef ULONG (WINAPI *Release)(void *);
typedef HRESULT (WINAPI *GetTextureLayout)(void *, void *);

int main(void) {
    HMODULE original = LoadLibraryW(L"xg_reference.dll");
    HMODULE proxy = LoadLibraryW(L"xg_xs.dll");
    if (!original || !proxy) return 1;
    Create a = (Create)(uintptr_t)GetProcAddress(original, "XGCreateTextureComputer");
    Create b = (Create)(uintptr_t)GetProcAddress(proxy, "XGCreateTextureComputer");
    if (!a || !b || GetProcAddress(original, "XGComputeTileShape") !=
        GetProcAddress(proxy, "XGComputeTileShape")) return 2;
    Desc d = {0}; d.dimension=3; d.width=128; d.height=128; d.slices=1;
    d.mips=5; d.format=28; d.samples=1; d.layout=261;
    void *ca=NULL, *cb=NULL;
    SetLastError(0x1234); HRESULT ha=a(&d,&ca); DWORD ea=GetLastError();
    SetLastError(0x1234); HRESULT hb=b(&d,&cb); DWORD eb=GetLastError();
    if (ha != hb || ea != eb || FAILED(ha) || !ca || !cb) return 3;
    void **va=*(void ***)ca, **vb=*(void ***)cb;
    unsigned char la[0x1b40], lb[0x1b40];
    memset(la,0,sizeof(la)); memset(lb,0,sizeof(lb));
    HRESULT ra=((GetTextureLayout)(uintptr_t)va[2])(ca,la);
    HRESULT rb=((GetTextureLayout)(uintptr_t)vb[2])(cb,lb);
    ULONG aa=((Release)(uintptr_t)va[1])(ca), ab=((Release)(uintptr_t)vb[1])(cb);
    if (ra != rb || memcmp(la,lb,sizeof(la)) || aa != ab) return 4;
    d.dimension = 0; ca=NULL; cb=NULL;
    SetLastError(0x4321); ha=a(&d,&ca); ea=GetLastError();
    SetLastError(0x4321); hb=b(&d,&cb); eb=GetLastError();
    if (ha != hb || ea != eb || ca != cb || SUCCEEDED(ha)) return 5;
    puts("xg-reference-proxy PASS success-layout failure last-error release forwarder");
    return 0;
}
