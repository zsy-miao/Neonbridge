#pragma once
#include "../compute_buffers.h"
#include <fstream>
#include <iterator>
#include <string>
#include <vector>

namespace shader_control {
using namespace compute_buffers;

inline std::vector<char> read_shader(const std::string &name) {
    std::ifstream input(name + ".cso", std::ios::binary);
    std::vector<char> code{std::istreambuf_iterator<char>{input}, {}};
    if (code.empty()) throw std::runtime_error("missing shader " + name);
    return code;
}

inline void write_buffer(ID3D12Resource *resource, const std::string &name, SIZE_T bytes) {
    D3D12_RANGE read{0, bytes}, none{};
    void *mapped{};
    check(resource->Map(0, &read, &mapped), "map readback");
    std::ofstream output(name + ".bin", std::ios::binary);
    output.write(static_cast<const char *>(mapped), bytes);
    resource->Unmap(0, &none);
    output.close();
    if (!output) throw std::runtime_error("write readback " + name);
}

struct Context {
    ComPtr<ID3D12Device> device;
    ComPtr<ID3D12CommandQueue> queue;
    explicit Context(FILE *receipt) {
        check(D3D12CreateDevice(nullptr, D3D_FEATURE_LEVEL_12_0, IID_PPV_ARGS(&device)), "device");
        ComPtr<IDXGIFactory4> factory;
        ComPtr<IDXGIAdapter1> adapter;
        DXGI_ADAPTER_DESC1 gpu{};
        check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "factory");
        check(factory->EnumAdapterByLuid(device->GetAdapterLuid(), IID_PPV_ARGS(&adapter)), "used adapter");
        check(adapter->GetDesc1(&gpu), "adapter description");
        std::fprintf(receipt, "adapter=%04x:%04x\n", gpu.VendorId, gpu.DeviceId);
        D3D12_COMMAND_QUEUE_DESC qd{};
        check(device->CreateCommandQueue(&qd, IID_PPV_ARGS(&queue)), "queue");
    }
};
}
