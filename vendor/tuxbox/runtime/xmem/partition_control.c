// Exercise the single-pool desktop XMem partition profile in a fresh process.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdio.h>
#include <stdint.h>

__declspec(dllimport) BOOLEAN WINAPI XMemGetPartitionHandleAndId(UINT, HANDLE*, UINT*);
__declspec(dllimport) BOOL WINAPI XMemGetPhysicalMemoryLayout(UINT64*);
__declspec(dllimport) BOOL WINAPI XMemTransferMemory(UINT, SIZE_T);
__declspec(dllimport) BOOL WINAPI XMemAllocatePhysicalPages(UINT64, SIZE_T*, UINT64*);
__declspec(dllimport) BOOL WINAPI XMemFreePhysicalPages(SIZE_T, const UINT64*);
__declspec(dllimport) BOOL WINAPI XMemGetWorkingSetStatistics(UINT, UINT64*);
#define CHECK(expr) do { if (!(expr)) {fprintf(stderr,"line %d failed (%lu)\n",__LINE__,GetLastError()); return 1;} } while(0)

int wmain(int argc, wchar_t** argv) {
    BOOL report = argc==3 && !wcscmp(argv[1],L"--report");
    if (!report && argc!=1) return 2;
    UINT id = 0xffffffff;
    HANDLE handle = INVALID_HANDLE_VALUE;
    CHECK(!XMemGetPartitionHandleAndId(3,&handle,&id) && GetLastError()==ERROR_INVALID_PARAMETER);
    CHECK(!XMemGetPartitionHandleAndId(2,&handle,&id) && GetLastError()==ERROR_OBJECT_NOT_FOUND);
    CHECK(handle==INVALID_HANDLE_VALUE && id==UINT32_MAX);
    UINT has_handle = 0;
    for (UINT kind=0; kind<=1; ++kind) {
        CHECK(XMemGetPartitionHandleAndId(kind,NULL,NULL));
        CHECK(XMemGetPartitionHandleAndId(kind,NULL,&id));
        MEMORY_BASIC_INFORMATION info = {0};
        CHECK(VirtualQuery(&id,&info,sizeof(info))==sizeof(info));
        CHECK(id==(kind ? (UINT)info.PartitionId : 0u));
        if (XMemGetPartitionHandleAndId(kind,&handle,&id)) {
            DWORD flags;
            CHECK(GetHandleInformation(handle,&flags) && (flags & HANDLE_FLAG_INHERIT));
            CHECK(CloseHandle(handle));
            ++has_handle;
        } else {
            CHECK(handle==INVALID_HANDLE_VALUE);
        }
        handle=INVALID_HANDLE_VALUE;
    }
    UINT64 bounds[4]={0x12345678,0,0,0x76543210}, pages[2]={0};
    SIZE_T count=2;
    CHECK(XMemGetPhysicalMemoryLayout(bounds+1));
    CHECK(bounds[0]==0x12345678 && bounds[3]==0x76543210 && bounds[1]==bounds[2]);
    CHECK(XMemAllocatePhysicalPages(0x20400000,&count,pages) && count==2);
    CHECK(bounds[1]==pages[0] && pages[1]-pages[0]==16);
    CHECK(XMemFreePhysicalPages(count,pages));
    SetLastError(0x2233);
    CHECK(XMemTransferMemory(0,0) && GetLastError()==0x2233);
    CHECK(XMemTransferMemory(9,0));
    CHECK(!XMemTransferMemory(9,1) && GetLastError()==ERROR_INVALID_PARAMETER);
    CHECK(!XMemTransferMemory(1,0) && GetLastError()==ERROR_NOT_SUPPORTED);
    CHECK(!XMemTransferMemory(2,1) && GetLastError()==ERROR_NOT_SUPPORTED);
    CHECK(!XMemTransferMemory(0,SIZE_MAX) && GetLastError()==ERROR_NOT_ENOUGH_MEMORY);
    UINT64 before[15], after[15];
    CHECK(XMemGetWorkingSetStatistics(0x20000001,before));
    // Make a measurable guest budget change without allocating the bytes.
    const SIZE_T transfer=256*1024*1024-1;
    CHECK(XMemTransferMemory(0,transfer));
    CHECK(XMemGetWorkingSetStatistics(0x20000001,after));
    CHECK(before[0]>after[0] && before[0]-after[0]>=transfer/2 && after[5]==after[0]);
    FILE* out=stdout;
    if (report && _wfopen_s(&out,argv[2],L"wb")) return 2;
    fprintf(out,"xmem-partitions PASS stable_cpu_pfns=2 budget_reduced=1 ids=2 native_handles=%u\n",has_handle);
    return report && fclose(out);
}
