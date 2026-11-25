// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics.Pipelines;
using Vortice.Direct3D12;
using Vortice.DXGI;

namespace FiveStar.Graphics.Rendering;

/// <summary>An offscreen D32_FLOAT depth target, cleared to the far plane of its <see cref="DepthConvention"/>.</summary>
public sealed class DepthTarget : IDisposable
{
    public const Format DepthFormat = Format.D32_Float;

    private readonly ID3D12Resource _texture;
    private readonly ID3D12DescriptorHeap _views;

    public DepthTarget(ID3D12Device device, int width, int height, DepthConvention convention)
    {
        Convention = convention;

        var description = ResourceDescription.Texture2D(DepthFormat, (uint)width, (uint)height, 1, 1, 1, 0, ResourceFlags.AllowDepthStencil);
        var clear = new ClearValue(DepthFormat, new DepthStencilValue(convention.Far, 0));
        _texture = device.CreateCommittedResource(new HeapProperties(HeapType.Default), HeapFlags.None, description, ResourceStates.DepthWrite, clear);
        _views = device.CreateDescriptorHeap(new DescriptorHeapDescription(DescriptorHeapType.DepthStencilView, 1));
        device.CreateDepthStencilView(_texture, null, View);
    }

    public DepthConvention Convention { get; }

    public CpuDescriptorHandle View => _views.GetCPUDescriptorHandleForHeapStart();

    public void Clear(ID3D12GraphicsCommandList list)
    {
        list.ClearDepthStencilView(View, ClearFlags.Depth, Convention.Far, 0);
    }

    public void Dispose()
    {
        _views.Dispose();
        _texture.Dispose();
    }
}
