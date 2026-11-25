#include <windows.h>
#include <stdio.h>

/* Run each supplied DLL in its own process so its dependency graph is cold. */
int wmain(int argc, wchar_t **argv) {
    if (argc != 2) return 2;
    wchar_t expected[MAX_PATH], loaded[MAX_PATH];
    DWORD length = GetFullPathNameW(argv[1], MAX_PATH, expected, NULL);
    if (!length || length >= MAX_PATH) return 2;
    HMODULE library = LoadLibraryW(expected);
    DWORD error = library ? ERROR_SUCCESS : GetLastError();
    length = library ? GetModuleFileNameW(library, loaded, MAX_PATH) : 0;
    int local = length && length < MAX_PATH && _wcsicmp(expected, loaded) == 0;
    FILE *receipt = NULL;
    if (fopen_s(&receipt, "load.txt", "w") != 0) return 2;
    fprintf(receipt, "%lu\n%d\n", error, local);
    fclose(receipt);
    if (library) FreeLibrary(library);
    return local ? 0 : 1;
}
