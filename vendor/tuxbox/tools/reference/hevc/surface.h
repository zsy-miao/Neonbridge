#pragma once
#include "frame_abi.h"
#include <d3d11.h>
#include <mfapi.h>
#include <mfd3d12.h>
#include <mferror.h>
#include <mfidl.h>
#include <mutex>
#include <mftransform.h>
#include <wrl/implements.h>
using Microsoft::WRL::ComPtr;
using Microsoft::WRL::Make;
template <typename... T>
using Classic = Microsoft::WRL::RuntimeClass<Microsoft::WRL::RuntimeClassFlags<Microsoft::WRL::ClassicCom>, T...>;
inline constexpr GUID queue_lifetime{0x57e39624, 0x6e1c, 0x4821, {0x98, 0x32, 0x7b, 0xba, 0x50, 0xf5, 0x4c, 0x1e}};
inline constexpr GUID sync_key{0x2a7c8d6a, 0x85a6, 0x494d, {0xa0, 0x46, 0x06, 0xea, 0x1a, 0x13, 0x8f, 0x4b}};
inline constexpr GUID adapter_clsid{0x73f6991b, 0x5eb1, 0x4eb5, {0x91, 0x2a, 0x2d, 0x0b, 0x6f, 0x83, 0x63, 0x7a}};
#define CHECK_MEDIA(call)                                                                                         \
    do {                                                                                                          \
        auto result = (call);                                                                                     \
        if (FAILED(result))                                                                                       \
            return result;                                                                                        \
    } while (false)
HRESULT decode_queue(IMFDXGIDeviceManager*, ID3D12CommandQueue**);
class Nv12TextureBatch {
    std::mutex mutex;
    ComPtr<ID3D12Device> owner;
    ComPtr<ID3D12Heap> heap;
    UINT64 offset{}, remaining{};

  public:
    void reset();
    HRESULT create(ID3D12Device*, const D3D12_HEAP_PROPERTIES&, const D3D12_RESOURCE_DESC&, ComPtr<ID3D12Resource>&);
};
HRESULT upload_nv12(Nv12TextureBatch&, ID3D12CommandQueue*, UINT, UINT, IMFSample*);
