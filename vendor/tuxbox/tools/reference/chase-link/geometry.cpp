// Exercise the actual three-stage TuxBox PSO with two distinct consumers.
#include "frame_stream.h"
#include <algorithm>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <vector>
using namespace frame_abi;
namespace fs = std::filesystem;

std::vector<char> read(const char *name) {
  std::ifstream stream(name, std::ios::binary);
  if (!stream)
    throw std::runtime_error(name);
  return {std::istreambuf_iterator<char>(stream), {}};
}

int wmain(int argc, wchar_t **argv) {
  const bool offline = argc == 3 && wcscmp(argv[2], L"offline") == 0;
  const bool dynamic = argc == 3 && wcscmp(argv[2], L"dynamic") == 0;
  if (argc != 2 && !offline && !dynamic)
    return 2;
  try {
    // All cache mutations in this control are confined to the trial.
    const auto local = fs::absolute("temp");
    fs::create_directory(local);
    if (!SetEnvironmentVariableW(L"TMP", local.c_str()) ||
        !SetEnvironmentVariableW(L"TEMP", local.c_str()) ||
        !fs::equivalent(fs::temp_directory_path(), local))
      throw std::runtime_error("private TMP");
    const fs::path cache = local / "tuxbox_shader_cache";
    fs::create_directory(cache);
    if (!fs::exists(cache / "by_source"))
      fs::copy("by_source", cache / "by_source", fs::copy_options::recursive);
    DXGI_ADAPTER_DESC1 adapter{};
    std::array<UINT, 18> profile{0x20004, 0, ~0u, 0, 0, 0, ~0u, ~0u, 0, 1};
    auto device = create_device(argv[1], adapter, profile.data());
    auto vs = read("vertex.dxbc"), gs = read("geometry.dxbc");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0, vs.data(), vs.size(),
                                      IID_PPV_ARGS(&root)),
          "cooked root");
    RasterPipelineStream stream;
    stream.root.payload = root.Get();
    stream.vs.payload = {vs.data(), vs.size()};
    stream.gs.payload = {gs.data(), gs.size()};
    std::array<D3D12_INPUT_ELEMENT_DESC, 2> layout{
        {{"ATTRIBUTE", 0, DXGI_FORMAT_R32G32_FLOAT, 0, 0,
          D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0},
         {"ATTRIBUTE", 1, DXGI_FORMAT_R32G32_FLOAT, 0, 8,
          D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0}}};
    stream.layout.payload = {layout.data(), static_cast<UINT>(layout.size())};
    stream.dsv.payload = DXGI_FORMAT_UNKNOWN;
    stream.formats.payload.NumRenderTargets = 2;
    stream.formats.payload.RTFormats[0] = stream.formats.payload.RTFormats[1] =
        DXGI_FORMAT_R16G16B16A16_FLOAT;
    stream.raster.payload.FillMode = D3D12_FILL_MODE_SOLID;
    stream.raster.payload.CullMode = D3D12_CULL_MODE_NONE;
    for (auto &rt : stream.blend.payload.RenderTarget)
      rt.RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL;
    D3D12_PIPELINE_STATE_STREAM_DESC raw{sizeof(stream), &stream};
    using Make = HRESULT(WINAPI *)(ID3D12Device *,
                                   const D3D12_PIPELINE_STATE_STREAM_DESC *,
                                   REFIID, void **);
    auto make = method<Make>(device.Get(), 86);
    auto probe = [&](const char *pixel, bool expected) {
      auto ps = read(pixel);
      stream.ps.payload = {ps.data(), ps.size()};
      ComPtr<ID3D12PipelineState> result;
      auto hr = make(device.Get(), &raw, IID_PPV_ARGS(&result));
      if (expected)
        check(hr, "paired pipeline creation");
      if (bool(result) != expected || (FAILED(hr) == expected))
        throw std::runtime_error("unexpected pipeline result");
    };
    if (offline || dynamic) {
      probe("pixel0.dxbc", dynamic);
      probe("vertex.dxbc", false);
    } else {
      const fs::path variants =
          fs::temp_directory_path() / "tuxbox_shader_cache" / "by_gs_ps";
      fs::create_directories(variants);
      auto path = [&](const char *key) {
        return variants / (std::string("4566ef26d1185681caa1cf06ae1a4df41db01a5eb"
                                       "9c3f161a109438879555f33_") +
                           key + ".cso");
      };
      auto p0 = path(
          "bfc8310727d73a1e98ac4d4d406173b8ccfcdfda2e355b21fb4fe8e96cbed830");
      auto p1 = path(
          "dbb0ca9c58d6b9baa842fce81b1d81a8f02d74c316124ccb7efff5baeaefaccc");
      if (fs::exists(p0) || fs::exists(p1))
        throw std::runtime_error("control requires private empty pair cache");
      probe("pixel0.dxbc", false);
      fs::copy_file("linked0.cso", p0);
      probe("pixel1.dxbc", false);
      probe("pixel0.dxbc", true);
      fs::copy_file("wrong-stage.cso", p1);
      probe("pixel1.dxbc", false);
      fs::copy_file("linked1.cso", p1, fs::copy_options::overwrite_existing);
      probe("pixel1.dxbc", true);
    }
    const bool frame_seven = fs::exists("vertex7.dxbc");
    const bool bridge = fs::exists("bridge.txt");
    if (frame_seven) {
      auto vertex = read("vertex7.dxbc");
      ComPtr<ID3D12RootSignature> material_root;
      check(device->CreateRootSignature(0, vertex.data(), vertex.size(), IID_PPV_ARGS(&material_root)),
            "material root");
      stream.root.payload = material_root.Get();
      stream.vs.payload = {vertex.data(), vertex.size()};
      stream.gs.payload = {};
      layout[0].Format = DXGI_FORMAT_R32G32B32A32_FLOAT;
      layout[1].SemanticIndex = 13;
      layout[1].Format = DXGI_FORMAT_R32_UINT;
      layout[1].AlignedByteOffset = 16;
      stream.formats.payload.NumRenderTargets = bridge ? 4 : 5;
      const DXGI_FORMAT targets[]{DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_R10G10B10A2_UNORM,
                                  DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
                                  DXGI_FORMAT_B8G8R8A8_UNORM};
      std::copy_n(targets, stream.formats.payload.NumRenderTargets, stream.formats.payload.RTFormats);
      if (offline || dynamic) {
        probe("pixel7.dxbc", dynamic);
      } else {
        const auto vs_pairs = cache / "by_vs_ps";
        fs::create_directory(vs_pairs);
        auto vs_key = read("vertex7.key"), wrong = read("other-consumer.key");
        const auto correct = vs_pairs / std::string(vs_key.begin(), vs_key.end());
        probe("pixel7.dxbc", false);
        fs::copy_file("linked7.cso", vs_pairs / std::string(wrong.begin(), wrong.end()));
        probe("pixel7.dxbc", false);
        fs::copy_file("wrong-vertex-stage.cso", correct);
        probe("pixel7.dxbc", false);
        fs::copy_file("linked7.cso", correct, fs::copy_options::overwrite_existing);
        probe("pixel7.dxbc", true);
        if (bridge) {
          // The failed title call had a translated producer and a missing consumer.
          const std::string key(vs_key.begin(), vs_key.end());
          const auto pixel = cache / "by_source" / key.substr(key.find('_') + 1);
          fs::rename(pixel, "held-pixel.cso");
          probe("pixel7.dxbc", false);
          fs::rename("held-pixel.cso", pixel);
        }
      }
    }
    FILE *f = nullptr;
    if (fopen_s(&f, "result.txt", "wb"))
      throw std::runtime_error("write result");
    if (offline || dynamic) {
      std::fprintf(f, "pci=%04x:%04x\ngeometry=%d\nvertex=%d\nwrong_pixel=0\n",
                   adapter.VendorId, adapter.DeviceId, int(dynamic), int(frame_seven && dynamic));
    } else {
      std::fprintf(
          f,
          "pci=%04x:%04x\nlinked=2\nabsent=1\nwrong_consumer=1\nwrong_stage=1\n",
          adapter.VendorId, adapter.DeviceId);
      if (frame_seven)
        std::fputs("vertex_linked=1\nvertex_absent=1\nvertex_wrong_consumer=1\nvertex_wrong_stage=1\n", f);
      if (bridge)
        std::fputs("bridge_pixel_absent=1\n", f);
    }
    if (fclose(f))
      throw std::runtime_error("close result");
    return 0;
  } catch (const std::exception &e) {
    std::fprintf(stderr, "GS-link failed: %s\n", e.what());
    return 1;
  }
}
