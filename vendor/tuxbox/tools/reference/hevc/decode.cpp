#include "readback.h"
#include <mfapi.h>
#include <mferror.h>
#include <mfidl.h>
#include <mftransform.h>
#include <algorithm>
#include <charconv>
#include <vector>
using frame_abi::check;
using frame_abi::ComPtr;
static ComPtr<IMFSample> sample(UINT32 size, const BYTE* data) {
    ComPtr<IMFSample> sm;
    check(MFCreateSample(&sm), "sample");
    ComPtr<IMFMediaBuffer> b;
    check(MFCreateMemoryBuffer(size, &b), "buffer");
    if (data) {
        BYTE* dst{};
        check(b->Lock(&dst, nullptr, nullptr), "lock");
        memcpy(dst, data, size);
        check(b->Unlock(), "unlock");
        check(b->SetCurrentLength(size), "size");
    }
    check(sm->AddBuffer(b.Get()), "attach");
    return sm;
}
int main(int argc, char** argv) {
    const bool custom = argc >= 8;
    const bool h264 = argc >= 3 && !strcmp(argv[2], "h264");
    if (!custom && argc != 2 && !(argc == 3 && h264)) return 2;
    UINT width = h264 ? 1024 : 3840, height = h264 ? 512 : 2160;
    UINT rate = h264 ? 30 : 24, denominator = 1;
    std::vector<UINT> selected{0, 12, 13, 14};
    if (custom) {
        auto number = [](const char* value, UINT& dest) {
            const char* end = value + strlen(value);
            const auto parsed = std::from_chars(value, end, dest);
            return parsed.ptr == end && parsed.ec == std::errc{};
        };
        if (!number(argv[3], width) || !number(argv[4], height) || !number(argv[5], rate)
            || !number(argv[6], denominator) || !width || !height || !rate || !denominator) return 2;
        selected.clear();
        for (int i = 7; i < argc; ++i) {
            UINT frame{};
            if (!number(argv[i], frame) || (!selected.empty() && frame <= selected.back())) return 2;
            selected.push_back(frame);
        }
    }
    bool gpu = strcmp(argv[1], "gpu") == 0;
    FILE* f{};
    if (fopen_s(&f, gpu ? "gpu.txt" : "cpu.txt", "wb"))
        return 2;
    setvbuf(f, nullptr, _IONBF, 0);
    try {
        check(CoInitializeEx(nullptr, COINIT_MULTITHREADED), "COM");
        check(MFStartup(MF_VERSION), "startup");
        ComPtr<IMFTransform> decoder;
        check(CoCreateInstance(h264 ? CLSID_MSH264DecoderMFT : adapter_clsid, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&decoder)), "decoder");
        ComPtr<ID3D12Device> device;
        ComPtr<IMFDXGIDeviceManager> manager;
        if (gpu) {
            UINT token{};
            check(D3D12CreateDevice(nullptr, D3D_FEATURE_LEVEL_11_0, IID_PPV_ARGS(&device)), "D3D12 device");
            check(MFCreateDXGIDeviceManager(&token, &manager), "manager");
            using Reset = HRESULT(WINAPI*)(IMFDXGIDeviceManager*, IUnknown*, UINT);
            auto reset = reinterpret_cast<Reset>(
                GetProcAddress(GetModuleHandleW(L"mfplat.dll"), "MFResetDXGIDeviceManagerX"));
            if (!reset)
                throw std::runtime_error("no bridge");
            check(reset(manager.Get(), device.Get(), token), "bind D3D12");
            check(decoder->ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, reinterpret_cast<ULONG_PTR>(manager.Get())),
                  "bind decoder");
        }
        ComPtr<IMFMediaType> input;
        check(MFCreateMediaType(&input), "type");
        check(input->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video), "video");
        check(input->SetGUID(MF_MT_SUBTYPE, h264 ? MFVideoFormat_H264 : MFVideoFormat_HEVC), "HEVC");
        check(MFSetAttributeSize(input.Get(), MF_MT_FRAME_SIZE, width, height), "shape");
        check(MFSetAttributeRatio(input.Get(), MF_MT_FRAME_RATE, rate, denominator), "rate");
        check(input->SetUINT32(MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive), "progressive");
        check(decoder->SetInputType(0, input.Get(), 0), "input type");
        auto outputType = [&]() {
            ComPtr<IMFMediaType> t;
            GUID sub{};
            for (UINT n = 0;; n++) {
                t.Reset();
                check(decoder->GetOutputAvailableType(0, n, &t), "supported output type");
                check(t->GetGUID(MF_MT_SUBTYPE, &sub), "subtype");
                if (sub == MFVideoFormat_NV12)
                    break;
            }
            check(decoder->SetOutputType(0, t.Get(), 0), "NV12");
        };
        outputType();
        check(decoder->ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0), "begin");
        check(decoder->ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0), "start");
        MFT_OUTPUT_STREAM_INFO info{};
        check(decoder->GetOutputStreamInfo(0, &info), "output info");
        fprintf(f, "output_flags=%lx size=%lu\n", info.dwFlags, info.cbSize);
        FILE *bits{}, *index{};
        if (fopen_s(&bits, h264 ? "source.h264" : "logo.h265", "rb") || fopen_s(&index, "packets.csv", "r"))
            throw std::runtime_error("input missing");
        UINT pk = 0;
        UINT64 pos{};
        DWORD frames = 0;
        ComPtr<ID3D12Resource> first_texture;
        std::vector<BYTE> first_pixels;
        for (UINT i = 0;; ++i) {
            const int fields = fscanf_s(index, "%u,%llu", &pk, &pos);
            if (fields == EOF) {
                check(decoder->ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0), "end of stream");
                check(decoder->ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0), "drain decoder");
            } else {
                if (fields != 2)
                    throw std::runtime_error("invalid packet index");
                std::vector<BYTE> bytes(pk);
                if (fread(bytes.data(), 1, pk, bits) != pk)
                    throw std::runtime_error("input truncated");
                auto in = sample(pk, bytes.data());
                check(in->SetSampleTime(i * 10000000LL * denominator / rate), "timestamp");
                check(in->SetSampleDuration(10000000LL * denominator / rate), "duration");
                if (!i)
                    check(in->SetUINT32(MFSampleExtension_CleanPoint, TRUE), "keyframe");
                check(decoder->ProcessInput(0, in.Get(), 0), "input");
            }
            for (;;) {
                ComPtr<IMFSample> provided =
                    (info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES) ? nullptr : sample(info.cbSize, nullptr);
                MFT_OUTPUT_DATA_BUFFER output{0, provided.Get(), 0, nullptr};
                DWORD status{};
                HRESULT hr = decoder->ProcessOutput(0, 1, &output, &status);
                if (output.pEvents)
                    output.pEvents->Release();
                if (hr == MF_E_TRANSFORM_NEED_MORE_INPUT)
                    break;
                if (hr == MF_E_TRANSFORM_STREAM_CHANGE) {
                    outputType();
                    check(decoder->GetOutputStreamInfo(0, &info), "update info");
                    continue;
                }
                check(hr, "output");
                ComPtr<IMFSample> got;
                if (output.pSample == provided.Get())
                    got = provided;
                else
                    got.Attach(output.pSample);
                if (!got)
                    throw std::runtime_error("no output sample");
                if (std::binary_search(selected.begin(), selected.end(), frames)) {
                    std::vector<BYTE> data;
                    if (gpu) {
                        const bool first = frames == selected.front();
                        data = readback(got.Get(), width, height, first ? &first_texture : nullptr);
                        if (first)
                            first_pixels = data;
                    } else {
                        ComPtr<IMFMediaBuffer> b;
                        check(got->ConvertToContiguousBuffer(&b), "contiguous");
                        BYTE* ptr{};
                        DWORD len{};
                        check(b->Lock(&ptr, nullptr, &len), "frame");
                        data.assign(ptr, ptr + len);
                        check(b->Unlock(), "unlock");
                    }
                    char fname[48];
                    sprintf_s(fname, "%s-frame-%02lu.nv12", gpu ? "gpu" : "cpu", frames);
                    FILE* raw{};
                    if (fopen_s(&raw, fname, "wb") || fwrite(data.data(), 1, data.size(), raw) != data.size())
                        throw std::runtime_error("save frame");
                    fclose(raw);
                    fprintf(f, "frame%lu=%zu\n", frames, data.size());
                }
                if (frames++ == selected.back()) {
                    if (gpu) {
                        // Drop the decoder and its current sample. Keep only the first native texture.
                        got.Reset();
                        provided.Reset();
                        decoder.Reset();
                        if (readback_texture(first_texture.Get(), nullptr, width, height) != first_pixels)
                            throw std::runtime_error("retained native texture changed");
                        fprintf(f, "retained_first_texture=match\n");
                    }
                    fclose(bits);
                    fclose(index);
                    fclose(f);
                    return 0;
                }
            }
            if (fields == EOF) {
                fprintf(f, "packets=%u decoded=%lu\n", i, frames);
                break;
            }
        }
        throw std::runtime_error("insufficient output");
    } catch (const std::exception& e) {
        std::fprintf(f, "error=%s\n", e.what());
        return 1;
    }
    return 0;
}
