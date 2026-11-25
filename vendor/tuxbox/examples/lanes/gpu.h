#pragma once

#include "../../runtime/d3d12/frame_abi.h"

Microsoft::WRL::ComPtr<ID3D12Device> create_lanes_device(const wchar_t* requested, std::string& pci);
Microsoft::WRL::ComPtr<ID3D12CommandQueue> create_lanes_queue(ID3D12Device* device, D3D12_COMMAND_LIST_TYPE type);
// The Xbox factory hands back a closed list. Reset it before recording.
Microsoft::WRL::ComPtr<ID3D12GraphicsCommandList> create_lanes_list(
    ID3D12Device* device, D3D12_COMMAND_LIST_TYPE type, ID3D12CommandAllocator* allocator);
