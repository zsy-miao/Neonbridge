// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Vortice.Direct3D12;
using Vortice.DXGI;
using Vortice.Mathematics;

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>
/// One run being recorded: its command list, the shader-visible heaps with the bindless resources, render targets,
/// pipeline statistics and the readbacks collected when the run finishes. Owns every object it creates.
/// </summary>
public sealed class Recording : IDisposable
{
    private const int MinimumResourceSlots = 64;
    private const int MinimumSamplerSlots = 16;
    private const int MinimumBufferBytes = 256;
    private const int StatisticsCount = 11;
    private const int UserDataDwords = 128;

    private readonly ID3D12Device _device;
    private readonly List<IDisposable> _owned = [];
    private readonly List<(int HeapIndex, ID3D12Resource Buffer, ID3D12Resource Readback, int Words)> _writable = [];
    private readonly List<(int HeapIndex, ID3D12Resource Texture, ID3D12Resource Readback, TextureLayout Layout)> _writableTextures = [];
    private readonly List<(ID3D12Resource Target, ID3D12Resource Readback)> _targets = [];
    private Framebuffer? _framebuffer;
    private ID3D12QueryHeap? _queries;
    private ID3D12Resource? _statistics;
    private ID3D12Resource? _arguments;

    public Recording(ID3D12Device device)
    {
        _device = device;
        var allocator = Own(device.CreateCommandAllocator(CommandListType.Direct));
        List = Own(device.CreateCommandList<ID3D12GraphicsCommandList>(0, CommandListType.Direct, allocator, null));
    }

    public ID3D12GraphicsCommandList List { get; }

    public ID3D12DescriptorHeap ResourceHeap { get; private set; } = null!;

    public T Own<T>(T item)
        where T : IDisposable
    {
        _owned.Add(item);

        return item;
    }

    public ID3D12Resource Buffer(long bytes, HeapType heap, ResourceStates state, ResourceFlags flags = ResourceFlags.None)
    {
        var description = ResourceDescription.Buffer((ulong)Math.Max(bytes, MinimumBufferBytes), flags);

        return Own(_device.CreateCommittedResource(new HeapProperties(heap), HeapFlags.None, description, state));
    }

    /// <summary>A default-heap buffer filled through an upload buffer, left in <paramref name="state"/>.</summary>
    public ID3D12Resource FilledBuffer(uint[] words, int bytes, ResourceStates state, ResourceFlags flags)
    {
        var upload = Buffer(bytes, HeapType.Upload, ResourceStates.GenericRead);
        var padded = new uint[bytes / 4];
        Array.Copy(words, padded, Math.Min(words.Length, padded.Length));
        Write(upload, padded);
        var buffer = Buffer(bytes, HeapType.Default, ResourceStates.Common, flags);
        List.CopyBufferRegion(buffer, 0, upload, 0, (ulong)bytes);

        List.ResourceBarrier(ResourceBarrier.BarrierTransition(buffer, ResourceStates.CopyDest, state));

        return buffer;
    }

    public static unsafe void Write(ID3D12Resource buffer, ReadOnlySpan<uint> words)
    {
        void* pointer;
        buffer.Map(0, null, &pointer).CheckError();
        words.CopyTo(new Span<uint>(pointer, words.Length));
        buffer.Unmap(0);
    }

    public static unsafe uint[] Read(ID3D12Resource buffer, int words)
    {
        void* pointer;
        buffer.Map(0, null, &pointer).CheckError();
        var result = new ReadOnlySpan<uint>(pointer, words).ToArray();
        buffer.Unmap(0);

        return result;
    }

    /// <summary>Creates the shader-visible heaps, fills them with the bindings and sets them on the list.</summary>
    public void CreateHeaps(Bindings bindings)
    {
        var slots = bindings.Buffers.Select(buffer => buffer.HeapIndex).Concat(bindings.Textures.Select(texture => texture.HeapIndex));
        var resourceSlots = Math.Max(MinimumResourceSlots, slots.DefaultIfEmpty(0).Max() + 1);
        var samplerSlots = Math.Max(MinimumSamplerSlots, bindings.Samplers.Select(sampler => sampler.HeapIndex).DefaultIfEmpty(0).Max() + 1);
        ResourceHeap = Own(_device.CreateDescriptorHeap(new DescriptorHeapDescription(
            DescriptorHeapType.ConstantBufferViewShaderResourceViewUnorderedAccessView, (uint)resourceSlots, DescriptorHeapFlags.ShaderVisible)));
        var samplers = Own(_device.CreateDescriptorHeap(new DescriptorHeapDescription(
            DescriptorHeapType.Sampler, (uint)samplerSlots, DescriptorHeapFlags.ShaderVisible)));

        for (var slot = 0; slot < resourceSlots; slot++)
        {
            _device.CreateShaderResourceView(null, RawView(1), ResourceSlot(slot));
        }

        for (var slot = 0; slot < samplerSlots; slot++)
        {
            CreateSampler(samplers, new SamplerResource(slot, Linear: false));
        }

        foreach (var buffer in bindings.Buffers)
        {
            AddBuffer(buffer);
        }

        foreach (var texture in bindings.Textures)
        {
            AddTexture(texture);
        }

        foreach (var sampler in bindings.Samplers)
        {
            CreateSampler(samplers, sampler);
        }

        List.SetDescriptorHeaps([ResourceHeap, samplers]);
    }

    public CpuDescriptorHandle ResourceSlot(int slot)
    {
        var increment = _device.GetDescriptorHandleIncrementSize(DescriptorHeapType.ConstantBufferViewShaderResourceViewUnorderedAccessView);

        return ResourceHeap.GetCPUDescriptorHandleForHeapStart() + (int)(slot * increment);
    }

    public GpuDescriptorHandle ResourceTable(int slot)
    {
        var increment = _device.GetDescriptorHandleIncrementSize(DescriptorHeapType.ConstantBufferViewShaderResourceViewUnorderedAccessView);

        return ResourceHeap.GetGPUDescriptorHandleForHeapStart() + (int)(slot * increment);
    }

    public void CreateRawUav(ID3D12Resource buffer, int words, int slot)
    {
        var description = new UnorderedAccessViewDescription
        {
            Format = Format.R32_Typeless,
            ViewDimension = UnorderedAccessViewDimension.Buffer,
            Buffer = new BufferUnorderedAccessView { FirstElement = 0, NumElements = (uint)words, Flags = BufferUnorderedAccessViewFlags.Raw },
        };
        _device.CreateUnorderedAccessView(buffer, null, description, ResourceSlot(slot));
    }

    public void CreateRawSrv(ID3D12Resource buffer, int words, int slot)
    {
        _device.CreateShaderResourceView(buffer, RawView(words), ResourceSlot(slot));
    }

    /// <summary>How parameter 0 takes the user SGPRs.</summary>
    public UserDataBinding UserData { get; init; }

    /// <summary>User SGPRs at parameter 0: root constants (zero-padded to the bound count) or a root CBV over 128 dwords.</summary>
    public void SetComputeRoot(ID3D12RootSignature root, Bindings bindings)
    {
        List.SetComputeRootSignature(root);

        if (UserData == UserDataBinding.Buffer)
        {
            List.SetComputeRootConstantBufferView(0, UserDataBuffer(bindings));
        }
        else
        {
            List.SetComputeRoot32BitConstants(0, RootConstants(bindings), 0);
        }
    }

    public void SetGraphicsRoot(ID3D12RootSignature root, Bindings bindings)
    {
        List.SetGraphicsRootSignature(root);

        if (UserData == UserDataBinding.Buffer)
        {
            List.SetGraphicsRootConstantBufferView(0, UserDataBuffer(bindings));
        }
        else
        {
            List.SetGraphicsRoot32BitConstants(0, RootConstants(bindings), 0);
        }
    }

    /// <summary>Creates, clears and binds the render targets, viewport and scissor.</summary>
    public Format[] CreateTargets(Framebuffer framebuffer)
    {
        _framebuffer = framebuffer;
        var format = framebuffer.FloatTargets ? Format.R32G32B32A32_Float : Format.R32G32B32A32_UInt;
        var views = Own(_device.CreateDescriptorHeap(new DescriptorHeapDescription(DescriptorHeapType.RenderTargetView, (uint)framebuffer.Count)));
        var increment = _device.GetDescriptorHandleIncrementSize(DescriptorHeapType.RenderTargetView);
        var handles = new CpuDescriptorHandle[framebuffer.Count];

        for (var index = 0; index < framebuffer.Count; index++)
        {
            var description = ResourceDescription.Texture2D(format, (uint)framebuffer.Width, (uint)framebuffer.Height, 1, 1, 1, 0, ResourceFlags.AllowRenderTarget);
            var target = Own(_device.CreateCommittedResource(new HeapProperties(HeapType.Default), HeapFlags.None, description, ResourceStates.RenderTarget));
            handles[index] = views.GetCPUDescriptorHandleForHeapStart() + (int)(index * increment);
            _device.CreateRenderTargetView(target, null, handles[index]);
            List.ClearRenderTargetView(handles[index], ClearColor(framebuffer.FloatTargets));
            _targets.Add((target, Buffer(TargetPitch(framebuffer) * framebuffer.Height, HeapType.Readback, ResourceStates.CopyDest)));
        }

        List.RSSetViewport(new Viewport(0, 0, framebuffer.ViewportWidth, framebuffer.ViewportHeight, 0, 1));
        List.RSSetScissorRect(new RectI(0, 0, framebuffer.Width, framebuffer.Height));
        List.OMSetRenderTargets(handles, null);

        return [.. Enumerable.Repeat(format, framebuffer.Count)];
    }

    public void BeginStatistics()
    {
        _queries = Own(_device.CreateQueryHeap<ID3D12QueryHeap>(new QueryHeapDescription(QueryHeapType.PipelineStatistics, 1)));
        _statistics = Buffer(MinimumBufferBytes, HeapType.Readback, ResourceStates.CopyDest);
        List.BeginQuery(_queries, QueryType.PipelineStatistics, 0);
    }

    public void EndStatistics()
    {
        List.EndQuery(_queries!, QueryType.PipelineStatistics, 0);
        List.ResolveQueryData(_queries!, QueryType.PipelineStatistics, 0, 1, _statistics!, 0);
    }

    /// <summary>Copies the first six dwords of a buffer (in COPY_SOURCE state) into the result's indirect arguments.</summary>
    public void ReadArguments(ID3D12Resource counter)
    {
        _arguments = Buffer(MinimumBufferBytes, HeapType.Readback, ResourceStates.CopyDest);
        List.CopyBufferRegion(_arguments, 0, counter, 0, 32);
    }

    /// <summary>Copies targets and writable buffers back, executes the list and collects the result.</summary>
    public WarpResult Finish(WarpDevice device)
    {
        foreach (var (target, readback) in _targets)
        {
            List.ResourceBarrier(ResourceBarrier.BarrierTransition(target, ResourceStates.RenderTarget, ResourceStates.CopySource));
            var footprint = new PlacedSubresourceFootPrint { Footprint = TargetFootprint(_framebuffer!) };
            List.CopyTextureRegion(new TextureCopyLocation(readback, footprint), 0, 0, 0, new TextureCopyLocation(target, 0), null);
        }

        foreach (var (_, buffer, readback, words) in _writable)
        {
            List.ResourceBarrier(ResourceBarrier.BarrierTransition(buffer, ResourceStates.UnorderedAccess, ResourceStates.CopySource));
            List.CopyBufferRegion(readback, 0, buffer, 0, (ulong)words * 4);
        }

        foreach (var (_, texture, readback, layout) in _writableTextures)
        {
            List.ResourceBarrier(ResourceBarrier.BarrierTransition(texture, ResourceStates.UnorderedAccess, ResourceStates.CopySource));

            for (var subresource = 0; subresource < layout.Subresources; subresource++)
            {
                List.CopyTextureRegion(new TextureCopyLocation(readback, layout.Footprint(subresource)), 0, 0, 0, new TextureCopyLocation(texture, (uint)subresource), null);
            }
        }

        device.Execute(List);
        var targets = _targets.Select(entry => TargetWords(entry.Readback)).ToList();
        var buffers = _writable.ToDictionary(entry => entry.HeapIndex, entry => Read(entry.Readback, entry.Words));
        var textures = _writableTextures.ToDictionary(entry => entry.HeapIndex, entry => entry.Layout.Unstage(Read(entry.Readback, entry.Layout.StagedBytes / 4)));
        uint[]? arguments = null;

        if (_arguments is not null)
        {
            arguments = Read(_arguments, 6);
        }

        return new WarpResult(targets, _framebuffer?.Width ?? 0, buffers, Statistics(), arguments) { Textures = textures };
    }

    public void Dispose()
    {
        for (var index = _owned.Count - 1; index >= 0; index--)
        {
            _owned[index].Dispose();
        }
    }

    private static ShaderResourceViewDescription RawView(int words)
    {
        return new ShaderResourceViewDescription
        {
            Format = Format.R32_Typeless,
            ViewDimension = ShaderResourceViewDimension.Buffer,
            Shader4ComponentMapping = ShaderComponentMapping.Default,
            Buffer = new BufferShaderResourceView { FirstElement = 0, NumElements = (uint)words, Flags = BufferShaderResourceViewFlags.Raw },
        };
    }

    private static Color4 ClearColor(bool isFloat)
    {
        if (isFloat)
        {
            var value = BitConverter.UInt32BitsToSingle(Framebuffer.ClearFloatBits);

            return new Color4(value, value, value, value);
        }

        return new Color4(Framebuffer.ClearUint, Framebuffer.ClearUint, Framebuffer.ClearUint, Framebuffer.ClearUint);
    }

    private static int TargetPitch(Framebuffer framebuffer)
    {
        return ((framebuffer.Width * 16) + 255) & ~255;
    }

    private static SubresourceFootPrint TargetFootprint(Framebuffer framebuffer)
    {
        var format = framebuffer.FloatTargets ? Format.R32G32B32A32_Float : Format.R32G32B32A32_UInt;

        return new SubresourceFootPrint(format, (uint)framebuffer.Width, (uint)framebuffer.Height, 1, (uint)TargetPitch(framebuffer));
    }

    private uint[] TargetWords(ID3D12Resource readback)
    {
        var framebuffer = _framebuffer!;
        var pitchWords = TargetPitch(framebuffer) / 4;
        var rowWords = framebuffer.Width * WarpResult.WordsPerPixel;
        var all = Read(readback, pitchWords * framebuffer.Height);
        var words = new uint[rowWords * framebuffer.Height];

        for (var y = 0; y < framebuffer.Height; y++)
        {
            Array.Copy(all, y * pitchWords, words, y * rowWords, rowWords);
        }

        return words;
    }

    private PipelineStatistics? Statistics()
    {
        if (_statistics is null)
        {
            return null;
        }

        var words = Read(_statistics, 2 * StatisticsCount);
        var values = Enumerable.Range(0, StatisticsCount).Select(index => words[2 * index] | ((ulong)words[(2 * index) + 1] << 32)).ToArray();

        return new PipelineStatistics(values[0], values[1], values[2], values[3], values[4], values[5], values[6], values[7], values[10]);
    }

    private uint[] RootConstants(Bindings bindings)
    {
        var count = bindings.RootConstantCount;

        if (UserData == UserDataBinding.UserConstants)
        {
            count = UserDataBindings.UserConstants;
        }

        var words = new uint[count];

        for (var index = 0; index < Math.Min(words.Length, bindings.UserSgprs.Count); index++)
        {
            words[index] = bindings.UserSgprs[index];
        }

        return words;
    }

    /// <summary>The user SGPRs in an upload buffer of 128 dwords, as the loader's ring block; its GPU address.</summary>
    private ulong UserDataBuffer(Bindings bindings)
    {
        var words = new uint[UserDataDwords];
        var count = Math.Min(words.Length, bindings.UserSgprs.Count);

        for (var index = 0; index < count; index++)
        {
            words[index] = bindings.UserSgprs[index];
        }

        var buffer = Buffer(UserDataDwords * sizeof(uint), HeapType.Upload, ResourceStates.GenericRead);
        Write(buffer, words);

        return buffer.GPUVirtualAddress;
    }

    private void AddBuffer(BufferResource resource)
    {
        var words = (Math.Max(resource.Words.Length, 4) + 3) & ~3;
        var state = resource.Writable ? ResourceStates.UnorderedAccess : ResourceStates.AllShaderResource;
        var flags = resource.Writable ? ResourceFlags.AllowUnorderedAccess : ResourceFlags.None;
        var buffer = FilledBuffer(resource.Words, words * 4, state, flags);

        if (!resource.Writable)
        {
            CreateRawSrv(buffer, words, resource.HeapIndex);

            return;
        }

        CreateRawUav(buffer, words, resource.HeapIndex);
        _writable.Add((resource.HeapIndex, buffer, Buffer(words * 4, HeapType.Readback, ResourceStates.CopyDest), words));
    }

    private void AddTexture(TextureResource texture)
    {
        var layout = new TextureLayout(texture);
        var staged = layout.Stage();
        var upload = Buffer(staged.Length * 4, HeapType.Upload, ResourceStates.GenericRead);
        Write(upload, staged);
        var resource = Own(_device.CreateCommittedResource(new HeapProperties(HeapType.Default), HeapFlags.None, layout.Resource(), ResourceStates.Common));

        for (var subresource = 0; subresource < layout.Subresources; subresource++)
        {
            List.CopyTextureRegion(new TextureCopyLocation(resource, (uint)subresource), 0, 0, 0, new TextureCopyLocation(upload, layout.Footprint(subresource)), null);
        }

        if (!texture.Writable)
        {
            List.ResourceBarrier(ResourceBarrier.BarrierTransition(resource, ResourceStates.CopyDest, ResourceStates.AllShaderResource));
            _device.CreateShaderResourceView(resource, layout.ShaderResourceView(), ResourceSlot(texture.HeapIndex));

            return;
        }

        List.ResourceBarrier(ResourceBarrier.BarrierTransition(resource, ResourceStates.CopyDest, ResourceStates.UnorderedAccess));
        _device.CreateUnorderedAccessView(resource, null, layout.UnorderedAccessView(), ResourceSlot(texture.HeapIndex));
        _writableTextures.Add((texture.HeapIndex, resource, Buffer(layout.StagedBytes, HeapType.Readback, ResourceStates.CopyDest), layout));
    }

    private void CreateSampler(ID3D12DescriptorHeap heap, SamplerResource sampler)
    {
        var filter = sampler.Linear ? Filter.MinMagMipLinear : Filter.MinMagMipPoint;

        if (sampler.Comparison)
        {
            filter = sampler.Linear ? Filter.ComparisonMinMagMipLinear : Filter.ComparisonMinMagMipPoint;
        }

        var description = sampler.Description ?? new SamplerDescription(filter, TextureAddressMode.Clamp, TextureAddressMode.Clamp, TextureAddressMode.Clamp)
        {
            ComparisonFunction = ComparisonFunction.LessEqual,
        };
        var increment = _device.GetDescriptorHandleIncrementSize(DescriptorHeapType.Sampler);
        _device.CreateSampler(ref description, heap.GetCPUDescriptorHandleForHeapStart() + (int)(sampler.HeapIndex * increment));
    }
}
