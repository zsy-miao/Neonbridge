// Exercise real Xbox pipeline creation with simultaneous native shader requests.
#include "frame_abi.h"
#include <atomic>
#include <thread>
#include <vector>
#include <fstream>
#include <filesystem>
#include <iterator>

using namespace frame_abi;
using Pipeline = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*,
                                  UINT, const void*, REFIID, void**);

int main() {
    try {
        DXGI_ADAPTER_DESC1 selected{};
        std::wifstream adapter_input("adapter.txt");
        std::wstring adapter;
        adapter_input >> adapter;
        if (adapter.empty()) throw std::runtime_error("missing adapter");
        auto device = create_device(adapter.c_str(), selected);
        D3D12_ROOT_SIGNATURE_DESC desc{};
        ComPtr<ID3DBlob> serialized;
        check(D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &serialized, nullptr),
              "serialize empty root");
        ComPtr<ID3D12RootSignature> root;
        check(device->CreateRootSignature(0, serialized->GetBufferPointer(), serialized->GetBufferSize(),
              IID_PPV_ARGS(&root)), "create empty root");
        constexpr unsigned unique = 16, total = 24;
        std::array<std::vector<char>, unique> inputs;
        for (unsigned i = 0; i < unique; ++i) {
            std::ifstream input("shader" + std::to_string(i) + ".dxbc", std::ios::binary);
            inputs[i].assign(std::istreambuf_iterator<char>(input), {});
            if (inputs[i].empty()) throw std::runtime_error("missing shader");
        }
        std::array<HRESULT, total> results{};
        std::array<ComPtr<ID3D12PipelineState>, total> pipelines;
        std::atomic<unsigned> ready{0};
        std::atomic<bool> go{false};
        std::vector<std::thread> threads;
        threads.reserve(total);
        for (unsigned i = 0; i < total; ++i) {
            threads.emplace_back([&, i] {
                const auto& code = inputs[i % unique];
                D3D12_COMPUTE_PIPELINE_STATE_DESC pipeline{};
                pipeline.pRootSignature = root.Get();
                pipeline.CS = {code.data(), code.size()};
                ++ready;
                while (!go.load()) std::this_thread::yield();
                results[i] = method<Pipeline>(device.Get(), 62)(
                    device.Get(), &pipeline, 0, nullptr, IID_PPV_ARGS(&pipelines[i]));
            });
        }
        while (ready.load() != total) std::this_thread::yield();
        go.store(true);
        for (auto& thread : threads) thread.join();
        unsigned matched = 0;
        for (unsigned i = 0; i < total; ++i) {
            if (SUCCEEDED(results[i]) && pipelines[i]) ++matched;
            else std::fprintf(stderr, "pipeline %u failed: 0x%08lx\n", i,
                              static_cast<unsigned long>(results[i]));
        }
        std::ofstream out("result.txt");
        out << "adapter=" << std::hex << selected.VendorId << ':' << selected.DeviceId
            << std::dec << "\nunique=" << unique << "\ncalls=" << total
            << "\ncreated=" << matched << "\n";
        out.close();
        return matched == total ? 0 : 1;
    } catch (const std::exception& error) {
        std::fprintf(stderr, "%s\n", error.what());
        return 2;
    }
}
