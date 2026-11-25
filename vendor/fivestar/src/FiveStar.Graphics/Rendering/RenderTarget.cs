// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Vortice.Direct3D12;
using Vortice.DXGI;
using Vortice.Mathematics;

namespace FiveStar.Graphics.Rendering;

/// <summary>
/// An offscreen colour target of a 32-bit format (such as R8G8B8A8_UNORM) with its readback buffer, drawn together
/// with a <see cref="DepthTarget"/>. Record
/// <see cref="Begin"/> before the draws and <see cref="CopyOut"/> after them; once the list ran, <see cref="ReadPixels"/>
/// returns the image.
/// </summary>
public sealed class RenderTarget : IDisposable
{
    public const int BytesPerPixel = 4;

    // D3D12_TEXTURE_DATA_PITCH_ALIGNMENT.
    private const int PitchAlignment = 256;

    private readonly ID3D12Resource _texture;
    private readonly ID3D12DescriptorHeap _views;
    private readonly ID3D12Resource _readback;
    private readonly int _pitch;

    public RenderTarget(ID3D12Device device, Format format, int width, int height)
    {
        Format = format;
        Width = width;
        Height = height;
        _pitch = ResourceArena.Align(width * BytesPerPixel, PitchAlignment);

        var description = ResourceDescription.Texture2D(format, (uint)width, (uint)height, 1, 1, 1, 0, ResourceFlags.AllowRenderTarget);
        _texture = device.CreateCommittedResource(new HeapProperties(HeapType.Default), HeapFlags.None, description, ResourceStates.RenderTarget);
        _views = device.CreateDescriptorHeap(new DescriptorHeapDescription(DescriptorHeapType.RenderTargetView, 1));
        device.CreateRenderTargetView(_texture, null, _views.GetCPUDescriptorHandleForHeapStart());
        _readback = device.CreateCommittedResource(
            new HeapProperties(HeapType.Readback), HeapFlags.None, ResourceDescription.Buffer((ulong)(_pitch * height)), ResourceStates.CopyDest);
    }

    public Format Format { get; }

    public int Width { get; }

    public int Height { get; }

    /// <summary>Clears the target and a depth target and binds both with a viewport and scissor covering them.</summary>
    public void Begin(ID3D12GraphicsCommandList list, Color4 clear, DepthTarget depth)
    {
        var view = _views.GetCPUDescriptorHandleForHeapStart();
        list.ClearRenderTargetView(view, clear);
        depth.Clear(list);
        list.OMSetRenderTargets(view, depth.View);
        list.RSSetViewport(new Viewport(0, 0, Width, Height, 0, 1));
        list.RSSetScissorRect(new RectI(0, 0, Width, Height));
    }

    /// <summary>Copies the target into the readback buffer; the target is left in COPY_SOURCE.</summary>
    public void CopyOut(ID3D12GraphicsCommandList list)
    {
        list.ResourceBarrier(ResourceBarrier.BarrierTransition(_texture, ResourceStates.RenderTarget, ResourceStates.CopySource));
        var footprint = new SubresourceFootPrint(Format, (uint)Width, (uint)Height, 1, (uint)_pitch);
        var destination = new TextureCopyLocation(_readback, new PlacedSubresourceFootPrint { Footprint = footprint });
        list.CopyTextureRegion(destination, 0, 0, 0, new TextureCopyLocation(_texture, 0), null);
    }

    /// <summary>The pixels as tightly packed rows, top row first.</summary>
    public byte[] ReadPixels()
    {
        var all = ResourceArena.Read(_readback, _pitch * Height);
        var rowBytes = Width * BytesPerPixel;
        var pixels = new byte[rowBytes * Height];

        for (var y = 0; y < Height; y++)
        {
            all.AsSpan(y * _pitch, rowBytes).CopyTo(pixels.AsSpan(y * rowBytes));
        }

        return pixels;
    }

    public void Dispose()
    {
        _readback.Dispose();
        _views.Dispose();
        _texture.Dispose();
    }
}
