#include "surface.h"

static HMODULE module_handle;

class Transform final : public Classic<IMFTransform> {
  public:
    ComPtr<IMFTransform> decoder;
    ComPtr<ID3D12CommandQueue> queue;
    Nv12TextureBatch textures;
    HMODULE module{};
    unsigned long frame_count{};
    bool h264{};
    ~Transform() {
        decoder.Reset();
        if (module)
            FreeLibrary(module);
    }
    HRESULT initialize(bool use_h264) {
        h264 = use_h264;
        wchar_t path[MAX_PATH];
        DWORD n = GetModuleFileNameW(module_handle, path, MAX_PATH);
        if (!n || n >= MAX_PATH)
            return HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER);
        wchar_t* slash = wcsrchr(path, L'\\');
        if (!slash)
            return E_FAIL;
        if (wcscpy_s(slash + 1, MAX_PATH - (slash + 1 - path), h264 ? L"frosty_packaged_h264.dll" : L"mfhevcdec.dll"))
            return E_INVALIDARG;
        module = LoadLibraryExW(path, nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
        if (!module)
            return HRESULT_FROM_WIN32(GetLastError());
        using GetClass = HRESULT(WINAPI*)(REFCLSID, REFIID, void**);
        auto get = reinterpret_cast<GetClass>(GetProcAddress(module, "DllGetClassObject"));
        if (!get)
            return HRESULT_FROM_WIN32(ERROR_PROC_NOT_FOUND);
        ComPtr<IClassFactory> factory;
        CHECK_MEDIA(get(h264 ? CLSID_MSH264DecoderMFT : CLSID_MSH265DecoderMFT, IID_PPV_ARGS(&factory)));
        return factory->CreateInstance(nullptr, IID_PPV_ARGS(&decoder));
    }
    STDMETHODIMP GetStreamLimits(DWORD* a, DWORD* b, DWORD* c, DWORD* d) override {
        return decoder->GetStreamLimits(a, b, c, d);
    }
    STDMETHODIMP GetStreamCount(DWORD* a, DWORD* b) override { return decoder->GetStreamCount(a, b); }
    STDMETHODIMP GetStreamIDs(DWORD a, DWORD* b, DWORD c, DWORD* d) override {
        return decoder->GetStreamIDs(a, b, c, d);
    }
    STDMETHODIMP GetInputStreamInfo(DWORD id, MFT_INPUT_STREAM_INFO* info) override {
        return decoder->GetInputStreamInfo(id, info);
    }
    STDMETHODIMP GetOutputStreamInfo(DWORD id, MFT_OUTPUT_STREAM_INFO* info) override {
        CHECK_MEDIA(decoder->GetOutputStreamInfo(id, info));
        if (queue)
            info->dwFlags =
                (info->dwFlags & ~MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES) | MFT_OUTPUT_STREAM_PROVIDES_SAMPLES;
        return S_OK;
    }
    STDMETHODIMP GetAttributes(IMFAttributes** a) override { return decoder->GetAttributes(a); }
    STDMETHODIMP GetInputStreamAttributes(DWORD id, IMFAttributes** a) override {
        return decoder->GetInputStreamAttributes(id, a);
    }
    STDMETHODIMP GetOutputStreamAttributes(DWORD id, IMFAttributes** a) override {
        return decoder->GetOutputStreamAttributes(id, a);
    }
    STDMETHODIMP DeleteInputStream(DWORD id) override { return decoder->DeleteInputStream(id); }
    STDMETHODIMP AddInputStreams(DWORD n, DWORD* ids) override { return decoder->AddInputStreams(n, ids); }
    STDMETHODIMP GetInputAvailableType(DWORD id, DWORD n, IMFMediaType** t) override {
        return decoder->GetInputAvailableType(id, n, t);
    }
    STDMETHODIMP GetOutputAvailableType(DWORD id, DWORD n, IMFMediaType** t) override {
        if (!queue)
            return decoder->GetOutputAvailableType(id, n, t);
        if (!t)
            return E_POINTER;
        *t = nullptr;
        for (DWORD i = 0; i < UINT_MAX; ++i) {
            ComPtr<IMFMediaType> available;
            CHECK_MEDIA(decoder->GetOutputAvailableType(id, i, &available));
            GUID subtype{};
            CHECK_MEDIA(available->GetGUID(MF_MT_SUBTYPE, &subtype));
            if (subtype == MFVideoFormat_NV12 && !n--)
                return available.CopyTo(t);
        }
        return MF_E_NO_MORE_TYPES;
    }
    STDMETHODIMP SetInputType(DWORD id, IMFMediaType* t, DWORD flags) override {
        return decoder->SetInputType(id, t, flags);
    }
    STDMETHODIMP SetOutputType(DWORD id, IMFMediaType* t, DWORD flags) override {
        if (queue && t) {
            GUID subtype{};
            CHECK_MEDIA(t->GetGUID(MF_MT_SUBTYPE, &subtype));
            if (subtype != MFVideoFormat_NV12)
                return MF_E_INVALIDMEDIATYPE;
        }
        return decoder->SetOutputType(id, t, flags);
    }
    STDMETHODIMP GetInputCurrentType(DWORD id, IMFMediaType** t) override {
        return decoder->GetInputCurrentType(id, t);
    }
    STDMETHODIMP GetOutputCurrentType(DWORD id, IMFMediaType** t) override {
        return decoder->GetOutputCurrentType(id, t);
    }
    STDMETHODIMP GetInputStatus(DWORD id, DWORD* flags) override { return decoder->GetInputStatus(id, flags); }
    STDMETHODIMP GetOutputStatus(DWORD* f) override { return decoder->GetOutputStatus(f); }
    STDMETHODIMP SetOutputBounds(LONGLONG a, LONGLONG b) override { return decoder->SetOutputBounds(a, b); }
    STDMETHODIMP ProcessEvent(DWORD id, IMFMediaEvent* e) override { return decoder->ProcessEvent(id, e); }
    STDMETHODIMP ProcessMessage(MFT_MESSAGE_TYPE msg, ULONG_PTR arg) override {
        if (msg != MFT_MESSAGE_SET_D3D_MANAGER)
            return decoder->ProcessMessage(msg, arg);
        if (!arg) {
            textures.reset();
            queue.Reset();
            return S_OK;
        }
        ComPtr<IMFDXGIDeviceManager> manager;
        CHECK_MEDIA(reinterpret_cast<IUnknown*>(arg)->QueryInterface(IID_PPV_ARGS(&manager)));
        ComPtr<ID3D12CommandQueue> candidate;
        CHECK_MEDIA(decode_queue(manager.Get(), &candidate));
        textures.reset();
        queue = std::move(candidate);
        return S_OK;
    }
    STDMETHODIMP ProcessInput(DWORD id, IMFSample* s, DWORD f) override { return decoder->ProcessInput(id, s, f); }
    STDMETHODIMP ProcessOutput(DWORD flags, DWORD count, MFT_OUTPUT_DATA_BUFFER* out, DWORD* status) override {
        if (!queue)
            return decoder->ProcessOutput(flags, count, out, status);
        if (count != 1 || !out || !status || out->pSample)
            return E_INVALIDARG;
        MFT_OUTPUT_STREAM_INFO info{};
        CHECK_MEDIA(decoder->GetOutputStreamInfo(out->dwStreamID, &info));
        ComPtr<IMFSample> cpu;
        if (!(info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES)) {
            ComPtr<IMFMediaBuffer> buffer;
            CHECK_MEDIA(MFCreateSample(&cpu));
            CHECK_MEDIA(MFCreateAlignedMemoryBuffer(info.cbSize, info.cbAlignment, &buffer));
            CHECK_MEDIA(cpu->AddBuffer(buffer.Get()));
        }
        MFT_OUTPUT_DATA_BUFFER decoded{out->dwStreamID, cpu.Get(), 0, nullptr};
        HRESULT hr = decoder->ProcessOutput(flags, count, &decoded, status);
        out->dwStatus = decoded.dwStatus;
        out->pEvents = decoded.pEvents;
        if (decoded.pSample != cpu.Get())
            cpu.Attach(decoded.pSample);
        CHECK_MEDIA(hr);
        if (!cpu)
            return E_UNEXPECTED;
        ComPtr<IMFMediaType> type;
        CHECK_MEDIA(decoder->GetOutputCurrentType(out->dwStreamID, &type));
        UINT w{}, h{};
        CHECK_MEDIA(MFGetAttributeSize(type.Get(), MF_MT_FRAME_SIZE, &w, &h));
        CHECK_MEDIA(upload_nv12(textures, queue.Get(), w, h, cpu.Get()));
        ++frame_count;
        if (frame_count <= 3 || frame_count % 60 == 0) {
            char message[144];
            std::snprintf(message, sizeof(message), "FrostyMF %s delivered D3D12 NV12 frame %lu (%ux%u)",
                          h264 ? "H264" : "HEVC", frame_count, w, h);
            OutputDebugStringA(message);
        }
        out->pSample = cpu.Detach();
        return S_OK;
    }
};

class Factory final : public Classic<IClassFactory> {
  public:
    bool h264{};
    STDMETHODIMP CreateInstance(IUnknown* outer, REFIID iid, void** out) override {
        if (!out)
            return E_POINTER;
        *out = nullptr;
        if (outer)
            return CLASS_E_NOAGGREGATION;
        auto object = Make<Transform>();
        if (!object)
            return E_OUTOFMEMORY;
        CHECK_MEDIA(object->initialize(h264));
        return object.CopyTo(iid, out);
    }
    STDMETHODIMP LockServer(BOOL) override { return S_OK; }
};
extern "C" HRESULT WINAPI DllGetClassObject(REFCLSID clsid, REFIID iid, void** out) {
    if (!out)
        return E_POINTER;
    *out = nullptr;
    if (clsid != adapter_clsid && clsid != CLSID_MSH264DecoderMFT)
        return CLASS_E_CLASSNOTAVAILABLE;
    auto factory = Make<Factory>();
    if (!factory) return E_OUTOFMEMORY;
    factory->h264 = clsid == CLSID_MSH264DecoderMFT;
    return factory.CopyTo(iid,out);
}
extern "C" BOOL WINAPI DllMain(HMODULE module, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH)
        module_handle = module;
    return TRUE;
}
