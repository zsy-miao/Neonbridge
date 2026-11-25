#include "surface.h"
int wmain(int argc, wchar_t** argv) {
    const bool h264 = argc == 3 && !wcscmp(argv[2], L"h264");
    if (argc != 2 && !h264)
        return 2;
    wchar_t full[MAX_PATH]{};
    const DWORD length = GetFullPathNameW(argv[1], MAX_PATH, full, nullptr);
    if (!length || length >= MAX_PATH || GetFileAttributesW(full) == INVALID_FILE_ATTRIBUTES)
        return 2;
    try {
        frame_abi::check(CoInitializeEx(nullptr, COINIT_MULTITHREADED), "initialize COM");
        frame_abi::check(MFStartup(MF_VERSION), "initialize Media Foundation");
        const wchar_t* key = h264 ? L"CLSID\\{62ce7e72-4c71-4d20-b15d-452831a87d9d}\\InprocServer32"
                                  : L"CLSID\\{73f6991b-5eb1-4eb5-912a-2d0b6f83637a}\\InprocServer32";
        frame_abi::check(
            HRESULT_FROM_WIN32(RegSetKeyValueW(HKEY_CLASSES_ROOT, key, nullptr, REG_SZ, full,
                                               static_cast<DWORD>((wcslen(full) + 1) * sizeof(wchar_t)))),
            "register provider");
        const wchar_t model[] = L"Both";
        frame_abi::check(HRESULT_FROM_WIN32(RegSetKeyValueW(HKEY_CLASSES_ROOT, key, L"ThreadingModel", REG_SZ,
                                                            model, sizeof(model))),
                         "register threading model");
        if (h264) {
            // Frosty directly activates the Microsoft decoder CLSID.
            ComPtr<IMFTransform> transform;
            frame_abi::check(CoCreateInstance(CLSID_MSH264DecoderMFT, nullptr, CLSCTX_INPROC_SERVER,
                                              IID_PPV_ARGS(&transform)), "activate H.264 transform");
            transform.Reset();
            MFShutdown();
            CoUninitialize();
            return 0;
        }
        MFT_REGISTER_TYPE_INFO in{MFMediaType_Video, MFVideoFormat_HEVC},
            out{MFMediaType_Video, MFVideoFormat_NV12};
        wchar_t name[] = L"Frosty packaged H265 decoder with D3D12 output";
        frame_abi::check(MFTRegister(adapter_clsid, MFT_CATEGORY_VIDEO_DECODER, name, MFT_ENUM_FLAG_SYNCMFT, 1,
                                     &in, 1, &out, nullptr),
                         "register HEVC to NV12");
        IMFActivate** result{};
        UINT count{};
        frame_abi::check(
            MFTEnumEx(MFT_CATEGORY_VIDEO_DECODER, MFT_ENUM_FLAG_SYNCMFT, &in, nullptr, &result, &count),
            "enumerate");
        bool found = false;
        for (UINT i = 0; i < count; i++) {
            GUID clsid{};
            if (SUCCEEDED(result[i]->GetGUID(MFT_TRANSFORM_CLSID_Attribute, &clsid)) && clsid == adapter_clsid) {
                ComPtr<IMFTransform> transform;
                frame_abi::check(result[i]->ActivateObject(IID_PPV_ARGS(&transform)), "activate HEVC transform");
                found = true;
            }
            result[i]->Release();
        }
        CoTaskMemFree(result);
        MFShutdown();
        CoUninitialize();
        return found ? 0 : 1;
    } catch (const std::exception& e) {
        std::fprintf(stderr, "%s\n", e.what());
        return 1;
    }
}
