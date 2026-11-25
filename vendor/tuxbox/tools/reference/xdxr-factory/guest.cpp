#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <objbase.h>
#include <wrl/client.h>
#include <wrl/implements.h>
#include <atomic>
#include <cstdio>
#include <cstdint>

constexpr GUID clsid{0x16bb3c85,0x006d,0x4b72,{0x9e,0x10,0x63,0xcd,0x97,0xdf,0x57,0xf0}};
constexpr GUID no_marshal{0xecc8691b,0xc1db,0x4dc0,{0x85,0x5e,0x65,0xf6,0xc5,0x51,0xaf,0x49}};
constexpr GUID bad{0x12345678,0x8765,0x4321,{1,2,3,4,5,6,7,8}};
using Factory = HRESULT (WINAPI*)(REFCLSID, REFIID, void**);
using Factory2 = HRESULT (WINAPI*)(IMalloc*, REFCLSID, REFIID, void**);
class Allocator final : public Microsoft::WRL::RuntimeClass<
        Microsoft::WRL::RuntimeClassFlags<Microsoft::WRL::ClassicCom>, IMalloc> {
public:
    std::atomic<ULONG> allocations{0}, frees{0};
    void* STDMETHODCALLTYPE Alloc(SIZE_T n) override { void* p=HeapAlloc(GetProcessHeap(),0,n); if(p) ++allocations; return p; }
    void* STDMETHODCALLTYPE Realloc(void* p, SIZE_T n) override {
        if (!p) return Alloc(n);
        if (n) return HeapReAlloc(GetProcessHeap(),0,p,n);
        Free(p); return nullptr;
    }
    void STDMETHODCALLTYPE Free(void* p) override { if(p && HeapFree(GetProcessHeap(),0,p)) ++frees; }
    SIZE_T STDMETHODCALLTYPE GetSize(void* p) override { return p ? HeapSize(GetProcessHeap(),0,p) : SIZE_T(-1); }
    int STDMETHODCALLTYPE DidAlloc(void*) override { return -1; }
    void STDMETHODCALLTYPE HeapMinimize() override {}
};
int wmain(int argc, wchar_t **argv) {
    if (argc != 2) return 2;
    HMODULE module=LoadLibraryW(L"dxcompiler_xs.dll"); if(!module) return 3;
    const auto p=GetProcAddress(module,"CreateXdxrCompiler"), q=GetProcAddress(module,"CreateXdxrCompiler2");
    if(!p || !q || p!=GetProcAddress(module,MAKEINTRESOURCEA(2)) || q!=GetProcAddress(module,MAKEINTRESOURCEA(1))) return 4;
    auto create=reinterpret_cast<Factory>(p); auto create2=reinterpret_cast<Factory2>(q);
    if (create(clsid, IID_IUnknown, nullptr)!=E_POINTER || create2(nullptr,clsid,IID_IUnknown,nullptr)!=E_POINTER) return 5;
    void* invalid=reinterpret_cast<void*>(uintptr_t(0x1234));
    HRESULT bad_class=create(bad,IID_IUnknown,&invalid);
    if(bad_class!=REGDB_E_CLASSNOTREG || invalid) return 6;
    invalid=reinterpret_cast<void*>(uintptr_t(0x1234));
    HRESULT bad_iid=create(clsid,bad,&invalid);
    if(bad_iid!=E_NOINTERFACE || invalid) return 7;
    IUnknown* obj=nullptr; HRESULT hr=create(clsid,IID_IUnknown,reinterpret_cast<void**>(&obj));
    if(hr!=S_OK || !obj) return 8;
    void* no_marshal_obj=nullptr; void* class_obj=nullptr; void* identity=nullptr;
    HRESULT no_marshal_hr=obj->QueryInterface(no_marshal,&no_marshal_obj), class_hr=obj->QueryInterface(clsid,&class_obj);
    HRESULT identity_hr=obj->QueryInterface(IID_IUnknown,&identity);
    if(no_marshal_hr!=S_OK || class_hr!=S_OK || identity_hr!=S_OK || no_marshal_obj!=obj || class_obj!=obj || identity!=obj) return 9;
    void* bad_query = reinterpret_cast<void*>(uintptr_t(0x1234));
    const HRESULT query_hr = obj->QueryInterface(bad, &bad_query);
    if (query_hr != E_NOINTERFACE || bad_query != reinterpret_cast<void*>(uintptr_t(0x1234))) return 16;
    auto allocator = Microsoft::WRL::Make<Allocator>();
    if (!allocator) return 13;
    IUnknown* custom_alloc_obj=nullptr;
    if (create2(allocator.Get(),clsid,no_marshal,reinterpret_cast<void**>(&custom_alloc_obj))!=S_OK ||
        !custom_alloc_obj || !allocator->allocations) return 14;
    if (custom_alloc_obj->Release()!=0 || allocator->allocations!=allocator->frees) return 15;
    void* vtable=*reinterpret_cast<void**>(obj); auto slots=static_cast<void**>(vtable);
    const auto base=reinterpret_cast<uintptr_t>(module);
    FILE* f=nullptr;if(_wfopen_s(&f,argv[1],L"wb"))return 10;
    std::fprintf(f,"bad_class=%08lx\nbad_iid=%08lx\nno_marshal_qi=%08lx\nclass_qi=%08lx\nidentity=%08lx\n",bad_class,bad_iid,no_marshal_hr,class_hr,identity_hr);
    std::fprintf(f,"bad_query=%08lx\nbad_query_output_unchanged=1\n",query_hr);
    std::fprintf(f,"allocator_allocations=%lu\nallocator_frees=%lu\n",allocator->allocations.load(),allocator->frees.load());
    for(int i=0;i<13;i++)std::fprintf(f,"slot%d=%llx\n",i,static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(slots[i])-base));
    static_cast<IUnknown*>(identity)->Release(); static_cast<IUnknown*>(class_obj)->Release(); static_cast<IUnknown*>(no_marshal_obj)->Release();
    ULONG final=obj->Release(); std::fprintf(f,"last_release=%lu\n",final); if(std::fclose(f))return 11;
    return final==0?0:12;
}
