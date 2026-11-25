#include "../visibility_fixture.h"
#include <iomanip>

namespace {
std::string pipelines(const wchar_t* adapter, bool) {
    using namespace visibility_fixture;
    const auto temp = fs::absolute("temp");
    if (!SetEnvironmentVariableW(L"TMP", temp.c_str()) || !SetEnvironmentVariableW(L"TEMP", temp.c_str()))
        throw std::runtime_error("set shader cache directory");
    Context gpu(adapter, 96);
    std::ifstream entries("pipelines.txt");
    std::ofstream results("pipeline-results.txt");
    if (!entries || !results) throw std::runtime_error("cannot open micropoly pipeline manifest or results");
    UINT index, count = 0;
    while (entries >> index) {
        const auto source = load(std::to_string(index) + ".dxbc");
        ComPtr<ID3D12RootSignature> root;
        check(gpu.device->CreateRootSignature(0, source.data(), source.size(), IID_PPV_ARGS(&root)),
              "create embedded micropoly root signature");
        D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
        desc.pRootSignature = root.Get();
        desc.CS = {source.data(), source.size()};
        ComPtr<ID3D12PipelineState> state;
        const HRESULT hr = method<Pipeline>(gpu.device.Get(), 62)(
            gpu.device.Get(), &desc, 0, nullptr, IID_PPV_ARGS(&state));
        results << std::dec << index << ' ' << std::hex << std::setw(8) << std::setfill('0')
                << static_cast<UINT>(hr) << std::endl;
        if (hr != S_OK || !state) throw std::runtime_error("micropoly pipeline failed at entry " + std::to_string(index));
        ++count;
    }
    if (!entries.eof() || !count || !results) throw std::runtime_error("incomplete micropoly pipeline survey");
    char text[96];
    std::snprintf(text, sizeof(text), "micropoly-pipelines PASS pci=%04x:%04x created=%u",
                  gpu.adapter.VendorId, gpu.adapter.DeviceId, count);
    return text;
}
}

int wmain(int argc, wchar_t** argv) {
    if (argc > 1 && !std::wcscmp(argv[1], L"--pipelines"))
        return tuxbox_test::frame_main(argc - 1, argv + 1, "micropoly", false, pipelines);
    return tuxbox_test::frame_main(argc, argv, "micropoly", false,
                                  [](const wchar_t* adapter, bool) { return visibility_fixture::run(adapter, true); });
}
