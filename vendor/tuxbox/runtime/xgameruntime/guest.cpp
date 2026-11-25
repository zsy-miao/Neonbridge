#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <cstdio>
#include <cstring>
#include <initializer_list>

extern "C" {
__declspec(dllimport) HRESULT WINAPI InitializeApiImpl(UINT, UINT);
__declspec(dllimport) HRESULT WINAPI InitializeApiImplEx(UINT, UINT, UINT64);
__declspec(dllimport) void WINAPI UninitializeApiImpl();
__declspec(dllimport) HRESULT WINAPI QueryApiImpl(const GUID*, const GUID*, void**);
}

static const GUID threading{0x073b7dcb, 0x1fcf, 0x4030, {0x94, 0xbe, 0xe3, 0xc9, 0xeb, 0x62, 0x34, 0x28}};
static const GUID system_api{0xe349bd1a, 0xfc20, 0x4e40, {0xb9, 0x9c, 0x41, 0x78, 0xcc, 0x6b, 0x40, 0x9f}};
static const GUID system_revision{0x1861cf2e, 0xe18b, 0x4834, {0xa9, 0xf5, 0xb4, 0xa4, 0xe6, 0xef, 0xb4, 0xcf}};
static const GUID unknown{};
using Handle = UINT_PTR;
using Action = void(WINAPI*)(void*, bool);
using Finished = void(WINAPI*)(void*);
using Create = HRESULT(WINAPI*)(void*, UINT, UINT, Handle*);
using Duplicate = HRESULT(WINAPI*)(void*, Handle, Handle*);
using Dispatch = bool(WINAPI*)(void*, Handle, UINT, UINT);
using Close = void(WINAPI*)(void*, Handle);
using Submit = HRESULT(WINAPI*)(void*, Handle, UINT, UINT, void*, Action);
using Terminate = HRESULT(WINAPI*)(void*, Handle, bool, void*, Finished);
using GetProcessQueue = bool(WINAPI*)(void*, Handle*);
using ComQuery = HRESULT(WINAPI*)(void*, const GUID*, void**);
using Ref = ULONG(WINAPI*)(void*);

template<class F> static F method(void* object, UINT slot) {
    return reinterpret_cast<F>((*static_cast<void***>(object))[slot]);
}

struct Observation {
    LONG ran{}, canceled{}, ended{};
    HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    ~Observation() { CloseHandle(event); }
};
static void WINAPI run(void* context, bool canceled) {
    auto& o = *static_cast<Observation*>(context);
    InterlockedIncrement(canceled ? &o.canceled : &o.ran);
    SetEvent(o.event);
}
static void WINAPI terminated(void* context) {
    auto& o = *static_cast<Observation*>(context);
    InterlockedIncrement(&o.ended);
    SetEvent(o.event);
}
#define CHECK(x) do { if (!(x)) {std::fprintf(stderr, "runtime control failed at line %d\n", __LINE__); return 1;} } while (0)

int main(int argc, char** argv) {
    void* obj = reinterpret_cast<void*>(1);
    CHECK(FAILED(QueryApiImpl(&threading, &threading, &obj)) && !obj);
    CHECK(SUCCEEDED(InitializeApiImplEx(10003, 7032, 2)));
    CHECK(SUCCEEDED(InitializeApiImpl(10003, 7032)));
    UninitializeApiImpl();
    CHECK(QueryApiImpl(&unknown, &threading, &obj) == E_NOINTERFACE && !obj);
    CHECK(QueryApiImpl(&threading, &unknown, &obj) == E_NOINTERFACE && !obj);
    void *system{};
    CHECK(SUCCEEDED(QueryApiImpl(&system_api, &system_revision, &system)) && system);
    using SandboxId = HRESULT(WINAPI*)(void*, size_t, char*, size_t*);
    auto sandbox_id = method<SandboxId>(system, 4);
    char id[17];
    std::memset(id, 0x5a, sizeof(id));
    size_t used = 0x5a;
    CHECK(sandbox_id(system, 15, id, &used) == HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER));
    CHECK(id[0] == 0x5a && used == 0x5a);
    CHECK(sandbox_id(system, 16, id, &used) == S_OK && used == 7);
    CHECK(std::strcmp(id, "RETAIL") == 0 && id[15] == 0 && id[16] == 0x5a);
    method<Ref>(system, 2)(system);
    CHECK(SUCCEEDED(QueryApiImpl(&threading, &threading, &obj)) && obj);
    void* same{};
    CHECK(SUCCEEDED(method<ComQuery>(obj, 0)(obj, &threading, &same)) && same == obj);
    method<Ref>(same, 2)(same);

    const auto create = method<Create>(obj,12);
    const auto close = method<Close>(obj,17);
    const auto dispatch = method<Dispatch>(obj,16);
    const auto submit = method<Submit>(obj,19);
    const auto terminate = method<Terminate>(obj,22);
    Handle queue = 1, copy{};
    CHECK(FAILED(create(obj, 99, 0, &queue)) && !queue);
    CHECK(SUCCEEDED(create(obj, 0, 0, &queue)));
    CHECK(SUCCEEDED(method<Duplicate>(obj,15)(obj, queue, &copy)) && copy);
    close(obj,queue);
    Observation o;
    CHECK(o.event && !dispatch(obj,copy,0,0));
    CHECK(SUCCEEDED(submit(obj,copy,0,0,&o,run)));
    CHECK(SUCCEEDED(submit(obj,copy,1,60000,&o,run)));
    CHECK(!dispatch(obj,copy,1,0));
    CHECK(dispatch(obj,copy,0,0) && o.ran==1 && !o.canceled);
    CHECK(SUCCEEDED(terminate(obj,copy,false,&o,terminated)));
    CHECK(submit(obj,copy,0,0,&o,run)==E_ABORT);
    CHECK(dispatch(obj,copy,1,0) && o.canceled==1 && !o.ended);
    CHECK(dispatch(obj,copy,1,0) && o.ended==1);
    CHECK(!dispatch(obj,copy,1,INFINITE));
    close(obj,copy);
    CHECK(!dispatch(obj,copy,1,0));

    // Frosty's startup creates a pool/manual queue, requests asynchronous
    // termination without a callback and dispatches its completion port forever.
    CHECK(SUCCEEDED(create(obj,1,0,&queue)));
    CHECK(SUCCEEDED(terminate(obj,queue,false,nullptr,nullptr)));
    CHECK(!dispatch(obj,queue,1,INFINITE));
    close(obj,queue);

    for (UINT mode : {1u, 2u, 3u}) {
        CHECK(SUCCEEDED(create(obj,mode,mode,&queue)));
        ResetEvent(o.event);
        const LONG before = o.ran;
        CHECK(SUCCEEDED(submit(obj,queue,0,0,&o,run)));
        CHECK(WaitForSingleObject(o.event,5000)==WAIT_OBJECT_0 && o.ran==before+1);
        CHECK(SUCCEEDED(terminate(obj,queue,true,nullptr,nullptr)));
        close(obj,queue);
    }
    CHECK(method<GetProcessQueue>(obj,25)(obj,&queue) && queue);
    close(obj,queue);
    method<Ref>(obj,2)(obj);
    UninitializeApiImpl();
    CHECK(FAILED(QueryApiImpl(&threading, &threading, &obj)) && !obj);
    FILE* report = stdout;
    if (argc == 2 && fopen_s(&report, argv[1], "wb")) return 1;
    std::fputs("xgameruntime PASS init=balanced interfaces=checked manual=ordered termination=canceled automatic=3 stale=rejected\n", report);
    if (argc == 2 && std::fclose(report)) return 1;
    return 0;
}
