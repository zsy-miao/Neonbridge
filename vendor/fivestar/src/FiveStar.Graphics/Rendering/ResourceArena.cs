// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Vortice.Direct3D12;

namespace FiveStar.Graphics.Rendering;

/// <summary>
/// Creates buffers and textures and owns them (and the upload buffers that fill them) until disposal, so nothing is
/// released while a command list still uses it.
/// </summary>
public sealed class ResourceArena : IDisposable
{
    // D3D12_TEXTURE_DATA_PITCH_ALIGNMENT, and the alignment of a constant buffer's address.
    private const int PitchAlignment = 256;

    private readonly ID3D12Device _device;
    private readonly List<IDisposable> _owned = [];

    public ResourceArena(ID3D12Device device)
    {
        _device = device;
    }

    /// <summary>
    /// A buffer in the upload heap holding <paramref name="data"/>, rounded up to 256 bytes. The GPU reads it in
    /// place, as vertices, a root constant buffer or a shader resource.
    /// </summary>
    public ID3D12Resource UploadBuffer(ReadOnlySpan<byte> data)
    {
        var buffer = Buffer(Align(Math.Max(data.Length, 1), PitchAlignment), HeapType.Upload, ResourceStates.GenericRead);
        Write(buffer, data);

        return buffer;
    }

    /// <summary>A buffer the GPU copies into and the CPU reads after the copy.</summary>
    public ID3D12Resource ReadbackBuffer(long bytes)
    {
        return Buffer(bytes, HeapType.Readback, ResourceStates.CopyDest);
    }

    /// <summary>A one-mip texture filled through an upload buffer; the copy is recorded on <paramref name="list"/>.</summary>
    public ID3D12Resource Texture2D(ID3D12GraphicsCommandList list, TextureData texture)
    {
        var pitch = Align(texture.RowBytes, PitchAlignment);
        var staged = new byte[pitch * texture.Rows];

        for (var row = 0; row < texture.Rows; row++)
        {
            texture.Data.AsSpan(row * texture.RowBytes, texture.RowBytes).CopyTo(staged.AsSpan(row * pitch));
        }

        var upload = UploadBuffer(staged);
        var description = ResourceDescription.Texture2D(texture.Format, (uint)texture.Width, (uint)texture.Height, 1, 1);
        var resource = Own(_device.CreateCommittedResource(new HeapProperties(HeapType.Default), HeapFlags.None, description, ResourceStates.Common));
        var footprint = new SubresourceFootPrint(texture.Format, (uint)texture.Width, (uint)texture.Height, 1, (uint)pitch);
        var source = new TextureCopyLocation(upload, new PlacedSubresourceFootPrint { Footprint = footprint });

        list.CopyTextureRegion(new TextureCopyLocation(resource, 0), 0, 0, 0, source, null);
        list.ResourceBarrier(ResourceBarrier.BarrierTransition(resource, ResourceStates.CopyDest, ResourceStates.AllShaderResource));

        return resource;
    }

    public T Own<T>(T item)
        where T : IDisposable
    {
        _owned.Add(item);

        return item;
    }

    public static unsafe void Write(ID3D12Resource buffer, ReadOnlySpan<byte> data)
    {
        void* pointer;
        buffer.Map(0, null, &pointer).CheckError();
        data.CopyTo(new Span<byte>(pointer, data.Length));
        buffer.Unmap(0);
    }

    public static unsafe byte[] Read(ID3D12Resource buffer, int bytes)
    {
        void* pointer;
        buffer.Map(0, null, &pointer).CheckError();
        var data = new ReadOnlySpan<byte>(pointer, bytes).ToArray();
        buffer.Unmap(0);

        return data;
    }

    public static int Align(int value, int alignment)
    {
        return (value + alignment - 1) / alignment * alignment;
    }

    public void Dispose()
    {
        for (var index = _owned.Count - 1; index >= 0; index--)
        {
            _owned[index].Dispose();
        }

        _owned.Clear();
    }

    private ID3D12Resource Buffer(long bytes, HeapType heap, ResourceStates state)
    {
        var description = ResourceDescription.Buffer((ulong)bytes);

        return Own(_device.CreateCommittedResource(new HeapProperties(heap), HeapFlags.None, description, state));
    }
}
