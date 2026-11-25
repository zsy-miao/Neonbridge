#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <winhttp.h>
#include <cstdio>
#include <cstring>
#include <cwchar>

// Xbox guest loader record. Each import resolves by module and symbol name.
struct Entry { const char* module; const char* name; FARPROC address; };
struct Table { UINT version, count; const Entry* entries; };
struct Hooks {
    decltype(&WinHttpOpen) open;
    decltype(&WinHttpConnect) connect;
    decltype(&WinHttpOpenRequest) request;
    decltype(&WinHttpWebSocketCompleteUpgrade) upgrade;
    decltype(&WinHttpCloseHandle) close;
    decltype(&WinHttpReceiveResponse) receive;
    decltype(&WinHttpSetOption) set_option;
};
extern "C" __declspec(dllimport) void WINAPI XMemSetWinHttpHooks(const Hooks*);

static int token;
static unsigned calls;
static HINTERNET WINAPI open(LPCWSTR agent, DWORD access, LPCWSTR, LPCWSTR, DWORD) {
    if (access == WINHTTP_ACCESS_TYPE_NO_PROXY && !std::wcscmp(agent, L"TuxBox")) calls |= 1;
    return &token;
}
static HINTERNET WINAPI connect(HINTERNET h, LPCWSTR host, INTERNET_PORT port, DWORD) {
    if (h == &token && port == 8443 && !std::wcscmp(host, L"localhost")) calls |= 2;
    return &token;
}
static HINTERNET WINAPI request(HINTERNET h, LPCWSTR verb, LPCWSTR, LPCWSTR, LPCWSTR, LPCWSTR*, DWORD flags) {
    if (h == &token && flags == WINHTTP_FLAG_SECURE && !std::wcscmp(verb,L"GET")) calls |= 4;
    return &token;
}
static HINTERNET WINAPI upgrade(HINTERNET h, DWORD_PTR context) {
    if (h == &token && context == 321) calls |= 8;
    return &token;
}
static BOOL WINAPI close_handle(HINTERNET h) {
    if (h == &token) calls |= 16;
    const Hooks reset{};
    XMemSetWinHttpHooks(&reset);
    return TRUE;
}
static BOOL WINAPI receive(HINTERNET h, LPVOID reserved) {
    if (h == &token && !reserved) calls |= 32;
    return TRUE;
}
static BOOL WINAPI set_option(HINTERNET h, DWORD opt, LPVOID value, DWORD size) {
    if(h == &token && opt == 42 && value == &token && size == sizeof(token)) calls |= 64;
    return TRUE;
}
int wmain(int argc, wchar_t** argv) {
    Hooks hooks{open,connect,request,upgrade,close_handle,receive,set_option};
    XMemSetWinHttpHooks(&hooks);
    HMODULE module = GetModuleHandleW(L"tuxbox_xhttp.dll");
    const auto table = reinterpret_cast<const Table*>(GetProcAddress(module, "__RedirectionInformation__"));
    if (!table || table->version != 1 || table->count != 7) return 1;
    auto resolve = [table](const char* name) -> FARPROC {
        for (UINT i=0; i<table->count; ++i) {
            const auto& e = table->entries[i];
            if (!_stricmp(e.module,"winhttp.dll") && !std::strcmp(e.name,name)) return e.address;
        }
        return nullptr;
    };
#define IMPORT(fn) reinterpret_cast<decltype(&fn)>(resolve(#fn))
    Hooks api{IMPORT(WinHttpOpen), IMPORT(WinHttpConnect), IMPORT(WinHttpOpenRequest),
              IMPORT(WinHttpWebSocketCompleteUpgrade), IMPORT(WinHttpCloseHandle),
              IMPORT(WinHttpReceiveResponse), IMPORT(WinHttpSetOption)};
    if (!api.open || !api.connect || !api.request || !api.upgrade || !api.close || !api.receive || !api.set_option)
        return 1;
    // A source-built guest links the same seven Windows SDK names through
    // TuxBox. Both entry paths must use the same callbacks and system fallback.
    if (api.open != &WinHttpOpen || api.connect != &WinHttpConnect ||
        api.request != &WinHttpOpenRequest || api.upgrade != &WinHttpWebSocketCompleteUpgrade ||
        api.close != &WinHttpCloseHandle || api.receive != &WinHttpReceiveResponse ||
        api.set_option != &WinHttpSetOption) return 1;
    const auto session = WinHttpOpen(L"TuxBox", WINHTTP_ACCESS_TYPE_NO_PROXY, nullptr, nullptr, 0);
    const auto conn = WinHttpConnect(session,L"localhost",8443,0);
    const auto req = WinHttpOpenRequest(conn,L"GET",nullptr,nullptr,nullptr,nullptr,WINHTTP_FLAG_SECURE);
    if (!api.set_option(req,42,&token,sizeof(token)) || !api.receive(req,nullptr) ||
        api.upgrade(req,321) != &token || !api.close(req) || calls != 127) return 1;
    const auto host = api.open(L"TuxBox", WINHTTP_ACCESS_TYPE_NO_PROXY, nullptr, nullptr, 0);
    if (!host || host == &token || !api.close(host)) return 1;
    FILE* out = stdout;
    if (argc == 3 && !std::wcscmp(argv[1],L"--report") && _wfopen_s(&out,argv[2],L"wb")) return 2;
    std::fputs("xmem-http PASS callbacks=7 host-session=1\n",out);
    return out != stdout && std::fclose(out);
}
