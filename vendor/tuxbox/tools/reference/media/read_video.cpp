#include "session.h"
#include <d3d11.h>
#include <mfapi.h>
#include <mfidl.h>
#include <mfreadwrite.h>
using frame_abi::check;
using Microsoft::WRL::ComPtr;

int wmain(int argc, wchar_t** argv) {
    if (argc != 4) return 2;
    FILE* out{};
    if (_wfopen_s(&out, argv[2], L"wb")) return 2;
    int result = 0;
    try {
        // Keep the renderer alive while MF drains its 11-on-12 worker threads.
        ComPtr<ID3D12Device> gpu;
        MediaSession session;
        ComPtr<IMFDXGIDeviceManager> manager;
        UINT token{};
        check(MFCreateDXGIDeviceManager(&token, &manager), "create manager");
        check(D3D12CreateDevice(nullptr, D3D_FEATURE_LEVEL_11_0, IID_PPV_ARGS(&gpu)), "create device");
        using Reset = HRESULT(WINAPI*)(IMFDXGIDeviceManager*, IUnknown*, UINT);
        auto reset = reinterpret_cast<Reset>(
            GetProcAddress(GetModuleHandleW(L"mfplat.dll"), "MFResetDXGIDeviceManagerX"));
        if (!reset) throw std::runtime_error("no Xbox reset");
        check(reset(manager.Get(), gpu.Get(), token), "bind manager");
        ComPtr<IMFAttributes> attrs;
        check(MFCreateAttributes(&attrs, 2), "reader attrs");
        check(attrs->SetUnknown(MF_SOURCE_READER_D3D_MANAGER, manager.Get()), "reader manager");
        check(attrs->SetUINT32(MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, TRUE), "hardware transforms flag");
        ComPtr<IMFSourceReader> reader;
        check(MFCreateSourceReaderFromURL(argv[1], attrs.Get(), &reader), "reader for file");
        ComPtr<IMFMediaType> native;
        check(reader->GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM, 0, &native), "native media type");
        GUID subtype{};
        check(native->GetGUID(MF_MT_SUBTYPE, &subtype), "video subtype");
        UINT w{}, h{};
        check(MFGetAttributeSize(native.Get(), MF_MT_FRAME_SIZE, &w, &h), "native frame size");
        std::fprintf(out, "native_fourcc=%08lx\nsize=%u,%u\n", subtype.Data1, w, h);
        std::fflush(out);
        ComPtr<IMFMediaType> target;
        check(MFCreateMediaType(&target), "create decoded type");
        check(target->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video), "major type");
        check(target->SetGUID(MF_MT_SUBTYPE, MFVideoFormat_NV12), "NV12 type");
        const HRESULT activate = reader->SetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM, nullptr,
                                                             target.Get());
        std::fprintf(out, "activate_decoder_hr=0x%08lx\n", static_cast<unsigned long>(activate));
        std::fflush(out);
        check(activate, "activate NV12 decoder");
        bool received = false;
        for (int i = 0; i < 12; ++i) {
            DWORD flags{};
            LONGLONG ts{};
            ComPtr<IMFSample> frame;
            check(reader->ReadSample(MF_SOURCE_READER_FIRST_VIDEO_STREAM, 0, nullptr, &flags, &ts, &frame),
                  "read decoded frame");
            if (flags & MF_SOURCE_READERF_ERROR) throw std::runtime_error("source reader error");
            if (!frame) {
                if (flags & MF_SOURCE_READERF_ENDOFSTREAM) break;
                continue;
            }
            ComPtr<IMFMediaBuffer> bytes;
            check(frame->ConvertToContiguousBuffer(&bytes), "map decoded frame");
            BYTE* data{};
            DWORD n{};
            check(bytes->Lock(&data, nullptr, &n), "lock frame bytes");
            FILE* raw{};
            size_t written = 0;
            if (!_wfopen_s(&raw, argv[3], L"wb")) {
                written = std::fwrite(data, 1, n, raw);
                if (std::fclose(raw)) written = 0;
            }
            check(bytes->Unlock(), "unlock frame");
            if (!written || written != n) throw std::runtime_error("could not write frame");
            std::fprintf(out, "decoded_bytes=%lu\ntimestamp=%lld\n", n, ts);
            std::fflush(out);
            if (n != static_cast<UINT64>(w) * h * 3 / 2) throw std::runtime_error("decoded frame size differs");
            received = true;
            break;
        }
        if (!received) throw std::runtime_error("no video frame");
    } catch (const std::exception& e) {
        std::fprintf(out, "error=%s\n", e.what());
        result = 1;
    }
    fclose(out);
    return result;
}
