#include "frame_abi.h"
#include <cstring>
#include <mfapi.h>
#include <mferror.h>
#include <mfidl.h>
#include <mftransform.h>
#include <vector>
using frame_abi::check;
using frame_abi::ComPtr;
static ComPtr<IMFSample> sample(DWORD capacity, const void* payload = nullptr) {
    ComPtr<IMFSample> s;
    ComPtr<IMFMediaBuffer> b;
    check(MFCreateSample(&s), "sample");
    check(MFCreateMemoryBuffer(capacity, &b), "buffer");
    if (payload) {
        BYTE* d{};
        check(b->Lock(&d, nullptr, nullptr), "lock");
        memcpy(d, payload, capacity);
        check(b->Unlock(), "unlock");
        check(b->SetCurrentLength(capacity), "set input length");
    }
    check(s->AddBuffer(b.Get()), "add buffer");
    return s;
}
int wmain(int argc, wchar_t** argv) {
    if (argc != 3)
        return 2;
    FILE* file{};
    if (_wfopen_s(&file, argv[2], L"wb"))
        return 2;
    setvbuf(file, nullptr, _IONBF, 0);
    try {
        check(CoInitializeEx(nullptr, COINIT_MULTITHREADED), "COM");
        check(MFStartup(MF_VERSION), "startup");
        if (!wcscmp(argv[1], L"register")) {
            wchar_t path[MAX_PATH]{};
            DWORD n = GetFullPathNameW(L"frosty_packaged_aac.dll", MAX_PATH, path, nullptr);
            if (!n || n >= MAX_PATH || GetFileAttributesW(path) == INVALID_FILE_ATTRIBUTES)
                throw std::runtime_error("missing packaged decoder");
            const wchar_t key[] = L"CLSID\\{32d186a7-218f-4c75-8876-dd77273a8999}\\InprocServer32";
            check(HRESULT_FROM_WIN32(
                      RegSetKeyValueW(HKEY_CLASSES_ROOT, key, nullptr, REG_SZ, path, (n + 1) * sizeof(wchar_t))),
                  "register class");
            const wchar_t threading[] = L"Both";
            check(HRESULT_FROM_WIN32(RegSetKeyValueW(HKEY_CLASSES_ROOT, key, L"ThreadingModel", REG_SZ, threading,
                                                     sizeof(threading))),
                  "register threading model");
        }
        ComPtr<IMFTransform> decoder;
        HMODULE local{};
        using CanUnload = HRESULT(WINAPI*)();
        CanUnload can_unload{};
        if (!wcscmp(argv[1], L"decode-local")) {
            wchar_t path[MAX_PATH]{}, actual[MAX_PATH]{};
            DWORD n = GetFullPathNameW(L"MSAudDecMFT.dll", MAX_PATH, path, nullptr);
            if (!n || n >= MAX_PATH || !(local = LoadLibraryExW(path, nullptr, LOAD_WITH_ALTERED_SEARCH_PATH)) ||
                !GetModuleFileNameW(local, actual, MAX_PATH) || _wcsicmp(actual, path))
                throw std::runtime_error("app-local AAC DLL unavailable");
            using GetClass = HRESULT(WINAPI*)(REFCLSID, REFIID, void**);
            auto get_class = reinterpret_cast<GetClass>(GetProcAddress(local, "DllGetClassObject"));
            can_unload = reinterpret_cast<CanUnload>(GetProcAddress(local, "DllCanUnloadNow"));
            if (!get_class || !can_unload || GetProcAddress(local, MAKEINTRESOURCEA(2)) !=
                GetProcAddress(local, "DllGetClassObject") || can_unload() != S_OK)
                throw std::runtime_error("AAC class exports");
            ComPtr<IClassFactory> factory;
            if (get_class(GUID_NULL, IID_PPV_ARGS(&factory)) != CLASS_E_CLASSNOTAVAILABLE || factory)
                throw std::runtime_error("unexpected AAC class");
            check(get_class(CLSID_MSAACDecMFT, IID_PPV_ARGS(&factory)), "AAC factory");
            check(factory->LockServer(TRUE), "lock AAC server");
            factory.Reset();
            if (can_unload() != S_FALSE) throw std::runtime_error("locked server permits unload");
            check(get_class(CLSID_MSAACDecMFT, IID_PPV_ARGS(&factory)), "reacquire AAC factory");
            check(factory->LockServer(FALSE), "unlock AAC server");
            check(factory->CreateInstance(nullptr, IID_PPV_ARGS(&decoder)), "activate app-local AAC");
            factory.Reset();
            if (can_unload() != S_FALSE) throw std::runtime_error("live decoder permits unload");
        } else {
            check(CoCreateInstance(CLSID_MSAACDecMFT, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&decoder)),
                  "activate AAC");
        }
        std::fprintf(file, "activated=1\n");
        if (!wcscmp(argv[1], L"register"))
            return 0;
        ComPtr<IMFMediaType> input;
        check(MFCreateMediaType(&input), "input type");
        check(input->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Audio), "audio type");
        check(input->SetGUID(MF_MT_SUBTYPE, MFAudioFormat_AAC), "AAC type");
        check(input->SetUINT32(MF_MT_AUDIO_SAMPLES_PER_SECOND, 48000), "input rate");
        check(input->SetUINT32(MF_MT_AUDIO_NUM_CHANNELS, 6), "input channels");
        check(input->SetUINT32(MF_MT_AUDIO_BITS_PER_SAMPLE, 16), "input depth");
        check(input->SetUINT32(MF_MT_AAC_PAYLOAD_TYPE, 0), "raw access unit");
        const BYTE userdata[] = {0, 0, 0xfe, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x11, 0xb0, 0x56, 0xe5, 0};
        check(input->SetBlob(MF_MT_USER_DATA, userdata, sizeof(userdata)), "codec config");
        check(decoder->SetInputType(0, input.Get(), 0), "set AAC input");
        ComPtr<IMFMediaType> output;
        GUID subtype{};
        UINT rate{}, channels{}, bits{};
        for (UINT n = 0;; n++) {
            output.Reset();
            check(decoder->GetOutputAvailableType(0, n, &output), "output type");
            check(output->GetGUID(MF_MT_SUBTYPE, &subtype), "output subtype");
            check(output->GetUINT32(MF_MT_AUDIO_SAMPLES_PER_SECOND, &rate), "rate");
            check(output->GetUINT32(MF_MT_AUDIO_NUM_CHANNELS, &channels), "channels");
            check(output->GetUINT32(MF_MT_AUDIO_BITS_PER_SAMPLE, &bits), "depth");
            if (channels == 6 && rate == 48000 &&
                ((subtype == MFAudioFormat_Float && bits == 32) || (subtype == MFAudioFormat_PCM && bits == 16)))
                break;
        }
        check(decoder->SetOutputType(0, output.Get(), 0), "set output");
        fprintf(file, "rate=%u\nchannels=%u\nbits=%u\nfloat=%u\n", rate, channels, bits,
                subtype == MFAudioFormat_Float);
        check(decoder->ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0), "begin");
        check(decoder->ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0), "start");
        MFT_OUTPUT_STREAM_INFO info{};
        check(decoder->GetOutputStreamInfo(0, &info), "output info");
        FILE *packets{}, *raw{};
        if (fopen_s(&packets, "access-units.bin", "rb") || fopen_s(&raw, "audio.pcm", "wb"))
            throw std::runtime_error("files unavailable");
        DWORD pk{}, idx = 0, pcmBytes = 0;
        bool drain = false;
        while (!drain) {
            if (fread(&pk, sizeof(pk), 1, packets) == 1) {
                std::vector<BYTE> data(pk);
                if (fread(data.data(), 1, pk, packets) != pk)
                    throw std::runtime_error("short packet");
                auto in = sample(pk, data.data());
                check(in->SetSampleTime(idx * 1024LL * 10000000 / 48000), "time");
                check(in->SetSampleDuration(1024LL * 10000000 / 48000), "duration");
                check(decoder->ProcessInput(0, in.Get(), 0), "compressed packet");
                ++idx;
            } else {
                check(decoder->ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0), "end");
                check(decoder->ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0), "drain");
                drain = true;
            }
            for (;;) {
                ComPtr<IMFSample> dest =
                    (info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES) ? nullptr : sample(info.cbSize);
                MFT_OUTPUT_DATA_BUFFER d{0, dest.Get(), 0, nullptr};
                DWORD flags{};
                HRESULT hr = decoder->ProcessOutput(0, 1, &d, &flags);
                if (d.pEvents)
                    d.pEvents->Release();
                if (hr == MF_E_TRANSFORM_NEED_MORE_INPUT)
                    break;
                check(hr, "decode to PCM");
                if (d.pSample != dest.Get())
                    dest.Attach(d.pSample);
                ComPtr<IMFMediaBuffer> b;
                check(dest->ConvertToContiguousBuffer(&b), "PCM");
                BYTE* ptr{};
                DWORD n{};
                check(b->Lock(&ptr, nullptr, &n), "lock PCM");
                if (fwrite(ptr, 1, n, raw) != n)
                    throw std::runtime_error("write PCM");
                check(b->Unlock(), "unlock");
                pcmBytes += n;
            }
        }
        fclose(packets);
        fclose(raw);
        fprintf(file, "access_units=%lu\npcm_bytes=%lu\n", idx, pcmBytes);
        if (!idx || !pcmBytes)
            throw std::runtime_error("no output");
        decoder.Reset();
        if (local) {
            if (can_unload() != S_OK) throw std::runtime_error("AAC decoder still retained");
            FreeLibrary(local);
        }
        MFShutdown();
        CoUninitialize();
        fclose(file);
        return 0;
    } catch (const std::exception& e) {
        fprintf(file, "error=%s\n", e.what());
        fclose(file);
        return 1;
    }
}
