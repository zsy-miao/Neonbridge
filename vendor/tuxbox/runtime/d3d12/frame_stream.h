// Guest layout written by Frosty's 0x20004 graphics pipeline builder.
#pragma once
#include "frame_abi.h"

namespace frame_abi {
#ifdef _MSC_VER
#pragma warning(push)
#pragma warning(disable : 4324)
#endif
template <typename T> struct alignas(void *) Part {
  UINT tag;
  T payload;
};
#ifdef _MSC_VER
#pragma warning(pop)
#endif
struct RasterPipelineStream {
  Part<UINT> flags{20, 0};
  Part<ID3D12RootSignature *> root{0, nullptr};
  Part<D3D12_INPUT_LAYOUT_DESC> layout{12, {}};
  Part<UINT> strip{13, 0}, topology{14, D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE};
  Part<D3D12_SHADER_BYTECODE> vs{1, {}}, gs{5, {}}, ps{2, {}};
  Part<D3D12_BLEND_DESC> blend{8, {}};
  Part<D3D12_DEPTH_STENCIL_DESC1> depth{21, {}};
  Part<DXGI_FORMAT> dsv{16, DXGI_FORMAT_D32_FLOAT};
  Part<D3D12_RASTERIZER_DESC> raster{10, {}};
  Part<D3D12_RT_FORMAT_ARRAY> formats{15, {}};
  Part<DXGI_SAMPLE_DESC> samples{17, {1, 0}};
  Part<UINT> mask{9, ~0u};
  Part<D3D12_CACHED_PIPELINE_STATE> cached{19, {}};
  UINT extension = 26;
  BYTE stencil[28]{};
};
static_assert(offsetof(RasterPipelineStream, vs) == 0x40 &&
              offsetof(RasterPipelineStream, extension) == 0x2a8 && sizeof(RasterPipelineStream) == 0x2c8);

struct MeshPipelineStream {
    Part<UINT> flags{20, 0};
    Part<ID3D12RootSignature*> root{0, nullptr};
    Part<D3D12_PRIMITIVE_TOPOLOGY_TYPE> topology{14, D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE};
    Part<D3D12_SHADER_BYTECODE> ms{24, {}}, as{23, {}}, ps{2, {}};
    Part<D3D12_BLEND_DESC> blend{8, {}};
    Part<D3D12_DEPTH_STENCIL_DESC1> depth{21, {}};
    Part<DXGI_FORMAT> dsv{16, DXGI_FORMAT_UNKNOWN};
    Part<D3D12_RASTERIZER_DESC> raster{10, {}};
    Part<D3D12_RT_FORMAT_ARRAY> formats{15, {}};
    Part<DXGI_SAMPLE_DESC> samples{17, {1, 0}};
    Part<UINT> mask{9, ~0u};
    Part<D3D12_CACHED_PIPELINE_STATE> cached{19, {}};
    UINT extension = 26;
    BYTE stencil[28]{};
};
static_assert(offsetof(MeshPipelineStream, ms) == 0x20 && offsetof(MeshPipelineStream, as) == 0x38);
static_assert(offsetof(MeshPipelineStream, ps) == 0x50 && offsetof(MeshPipelineStream, depth) == 0x1b8);
static_assert(offsetof(MeshPipelineStream, extension) == 0x288 && sizeof(MeshPipelineStream) == 0x2a8);
}
