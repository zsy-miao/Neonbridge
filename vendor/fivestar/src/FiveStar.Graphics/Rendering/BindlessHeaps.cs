// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Vortice.Direct3D12;
using Vortice.DXGI;

namespace FiveStar.Graphics.Rendering;

/// <summary>
/// The two shader-visible heaps translated programs index (<c>ResourceDescriptorHeap[]</c>,
/// <c>SamplerDescriptorHeap[]</c>). A slot holds whatever view a descriptor's dword0 names; unused resource slots hold
/// a null raw-buffer view and unused sampler slots a point/clamp sampler, so a stray index reads zeros.
/// </summary>
public sealed class BindlessHeaps : IDisposable
{
    private const int RawWordBytes = sizeof(uint);

    private readonly ID3D12Device _device;
    private readonly ID3D12DescriptorHeap _resources;
    private readonly ID3D12DescriptorHeap _samplers;
    private readonly uint _resourceIncrement;
    private readonly uint _samplerIncrement;

    public BindlessHeaps(ID3D12Device device, int resourceSlots, int samplerSlots)
    {
        _device = device;
        _resources = device.CreateDescriptorHeap(new DescriptorHeapDescription(
            DescriptorHeapType.ConstantBufferViewShaderResourceViewUnorderedAccessView, (uint)resourceSlots, DescriptorHeapFlags.ShaderVisible));
        _samplers = device.CreateDescriptorHeap(new DescriptorHeapDescription(DescriptorHeapType.Sampler, (uint)samplerSlots, DescriptorHeapFlags.ShaderVisible));
        _resourceIncrement = device.GetDescriptorHandleIncrementSize(DescriptorHeapType.ConstantBufferViewShaderResourceViewUnorderedAccessView);
        _samplerIncrement = device.GetDescriptorHandleIncrementSize(DescriptorHeapType.Sampler);

        for (var slot = 0; slot < resourceSlots; slot++)
        {
            device.CreateShaderResourceView(null, RawView(RawWordBytes), ResourceSlot(slot));
        }

        var fallback = new SamplerDescription(Filter.MinMagMipPoint, TextureAddressMode.Clamp, TextureAddressMode.Clamp, TextureAddressMode.Clamp);

        for (var slot = 0; slot < samplerSlots; slot++)
        {
            Sampler(fallback, slot);
        }
    }

    /// <summary>A <c>ByteAddressBuffer</c> view of the first <paramref name="bytes"/> bytes of a buffer.</summary>
    public void RawBuffer(ID3D12Resource buffer, int bytes, int slot)
    {
        _device.CreateShaderResourceView(buffer, RawView(bytes), ResourceSlot(slot));
    }

    /// <summary>The default view of a texture (its format, every mip).</summary>
    public void Texture(ID3D12Resource texture, int slot)
    {
        _device.CreateShaderResourceView(texture, null, ResourceSlot(slot));
    }

    public void Sampler(SamplerDescription description, int slot)
    {
        _device.CreateSampler(ref description, _samplers.GetCPUDescriptorHandleForHeapStart() + (int)(slot * _samplerIncrement));
    }

    /// <summary>Sets both heaps on a command list (before its root signature is set).</summary>
    public void Bind(ID3D12GraphicsCommandList list)
    {
        list.SetDescriptorHeaps([_resources, _samplers]);
    }

    public void Dispose()
    {
        _samplers.Dispose();
        _resources.Dispose();
    }

    private CpuDescriptorHandle ResourceSlot(int slot)
    {
        return _resources.GetCPUDescriptorHandleForHeapStart() + (int)(slot * _resourceIncrement);
    }

    private static ShaderResourceViewDescription RawView(int bytes)
    {
        return new ShaderResourceViewDescription
        {
            Format = Format.R32_Typeless,
            ViewDimension = ShaderResourceViewDimension.Buffer,
            Shader4ComponentMapping = ShaderComponentMapping.Default,
            Buffer = new BufferShaderResourceView { FirstElement = 0, NumElements = (uint)(bytes / RawWordBytes), Flags = BufferShaderResourceViewFlags.Raw },
        };
    }
}
