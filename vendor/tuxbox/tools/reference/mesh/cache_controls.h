// Exercise stripped-stage failures before creating or submitting any GPU work.
#pragma once
#include "mesh_fixture.h"
#include <filesystem>
#include <vector>

namespace mesh_fixture {
inline std::string cache_controls(const Context& ctx, MeshPipelineStream stream,
                                  const std::vector<BYTE>& portableMesh,
                                  const std::vector<BYTE>& strippedAmplification) {
    D3D12_PIPELINE_STATE_STREAM_DESC desc{sizeof(stream), &stream};
    auto reject = [&](const char* reason) {
        void* result = reinterpret_cast<void*>(1);
        HRESULT hr = method<CreateStream>(ctx.device2.Get(), 86)(
            ctx.device2.Get(), &desc, __uuidof(ID3D12PipelineState), &result);
        if (hr != E_NOTIMPL || result) throw std::runtime_error(reason);
    };
    reject("missing cooked mesh translation");
    // The caller supplies a fresh, private cache, with no host compiler running.
    const auto cache = std::filesystem::path("temp") / "tuxbox_shader_cache" / "by_source";
    std::filesystem::copy_file("wrong-stage.cso", cache /
        "0d4059821b7f492a8b6672e87fc9bdff6e4d826966b6bb5c78de9a37dc42dcb6.cso");
    reject("pixel DXIL in the mesh cache");
    // Frosty has no cooked amplification entries. Only the stripped AS
    // container envelope is needed to check the cached program's stage.
    stream.ms.payload = {portableMesh.data(), portableMesh.size()};
    stream.as.payload = {strippedAmplification.data(), strippedAmplification.size()};
    reject("pixel DXIL in the amplification cache");
    return "nanite-mesh-cache PASS missing_ms=E_NOTIMPL wrong_ms=E_NOTIMPL "
           "wrong_as=E_NOTIMPL cleared_outputs=3";
}
}
