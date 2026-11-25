// Match the runtime's size-only process-token query. No attributes are retrieved.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <appmodel.h>
#include <winternl.h>
#include <stdio.h>

__declspec(dllimport) NTSTATUS NTAPI NtQueryInformationToken(
    HANDLE token, TOKEN_INFORMATION_CLASS info, void *buffer, ULONG length, ULONG *required);

int wmain(int argc, wchar_t **argv)
{
    if (argc != 2) return 2;
    FILE *report;
    if (_wfopen_s(&report, argv[1], L"wb") != 0) return 2;
    _Static_assert(TokenSecurityAttributes == 39, "unexpected token information class");
    ULONG required = 0;
    NTSTATUS status = NtQueryInformationToken(GetCurrentProcessToken(), TokenSecurityAttributes, NULL, 0, &required);
    ULONG error = RtlNtStatusToDosError(status);
    WCHAR full_name[PACKAGE_FULL_NAME_MAX_LENGTH + 1];
    UINT32 chars = ARRAYSIZE(full_name);
    LONG identity = GetCurrentPackageFullName(&chars, full_name);
    int written = fprintf(report,
        "{\"ntstatus\":\"0x%08lx\",\"win32\":%lu,\"hresult_from_win32\":\"0x%08lx\",\"required_bytes\":%lu,"
        "\"package_status\":%ld,\"package_full_name\":%s%ls%s}\n",
        (ULONG)status, error, (ULONG)HRESULT_FROM_WIN32(error), required, identity,
        identity == ERROR_SUCCESS ? "\"" : "", identity == ERROR_SUCCESS ? full_name : L"null",
        identity == ERROR_SUCCESS ? "\"" : "");
    return fclose(report) != 0 || written < 0;
}
