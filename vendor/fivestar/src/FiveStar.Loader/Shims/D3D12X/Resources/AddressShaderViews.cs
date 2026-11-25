// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Pipelines;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// Device slot 70: an SRV described by a resource desc and a console GPU address. The address resolves to the
/// registered texture there (refreshed from memory; a GPU-written one of the same extent is taken whatever its format),
/// else to a texture made from the desc over readable memory; with neither, or for a depth texture, the descriptor is
/// a null view of the same dimension.
/// </summary>
internal sealed unsafe class AddressShaderViews
{
    /// <summary>ID3D12Device::CreateShaderResourceView (d3d12.h).</summary>
    private const int DesktopShaderResourceViewSlot = 18;

    /// <summary>Views between two log lines with the counters (after the powers of two up to it).</summary>
    private const int SummaryInterval = 4096;

    private readonly GpuTextures _textures;
    private readonly AddressBufferViews _buffers;
    private readonly OnceLog _warnings;
    private readonly LoaderLog _log;

    public AddressShaderViews(GpuTextures textures, AddressBufferViews buffers, OnceLog warnings, LoaderLog log)
    {
        _textures = textures;
        _buffers = buffers;
        _warnings = warnings;
        _log = log;
    }

    /// <summary>Writes the view into <paramref name="handle"/>, a desktop CPU descriptor handle; returns its texture, null for a null view.</summary>
    public GpuTexture? Create(nint device, ResourceDescription* resource, AddressViewDescription* view, nint handle, Binding.BufferRecord record)
    {
        var counters = _textures.Counters;

        long views = counters.Add(GpuResourceEvent.View);

        if (BitOperations.IsPow2(views) || views % SummaryInterval == 0)
        {
            _log.Detail($"  [d3d12_xs] console-address textures: {counters}; {_textures.Registry.Summary}; {_textures.Placed.Summary}");
        }

        if (IsBuffer(resource) && view->Address != 0)
        {
            counters.Add(GpuResourceEvent.ViewBuffer);

            if (_buffers.Create(device, view, handle, record))
            {
                return null;
            }

            _warnings.Warn("slot70-buffer", $"[d3d12_xs] ID3D12Device.X::Slot70: a buffer view at 0x{view->Address:x} outside known game memory or its buffer: null descriptor");
        }

        var texture = Resolve(device, resource, view);
        var desktop = *view;
        int viewFormat = DepthFormats.ShaderView(view->Format);
        nint target = 0;

        if (texture is not null)
        {
            desktop.Format = ViewFormats.ForView(texture.DesktopFormat, viewFormat);
            desktop.ComponentMapping = TargetFormats.ComponentMapping(viewFormat, desktop.ComponentMapping);
            target = texture.Desktop;
            NoteLoss(texture, viewFormat, desktop.Format);
        }

        if (target == 0 || desktop.Format == ViewFormats.Unknown)
        {
            counters.Add(GpuResourceEvent.ViewNull);
            _warnings.Warn("slot70-null", $"[d3d12_xs] ID3D12Device.X::Slot70: no readable texture at 0x{view->Address:x} (format {view->Format}): null descriptor");
            desktop.Format = viewFormat;
            target = 0;
            texture = null;
        }

        var create = (delegate* unmanaged<nint, nint, AddressViewDescription*, nint, void>)ComObject.Slot(device, DesktopShaderResourceViewSlot);
        create(device, target, &desktop, handle);

        return texture;
    }

    /// <summary>Counts a view that reads in another format than the game asked for, and names each pair of formats once.</summary>
    private void NoteLoss(GpuTexture texture, int viewFormat, int created)
    {
        var loss = ViewFormats.Loss(texture.DesktopFormat, viewFormat, created);

        // A format with a render-target substitute is read in the substitute on purpose (TargetFormats).
        if (loss == ViewFormatLoss.None || TargetFormats.HasSubstitute(viewFormat))
        {
            return;
        }

        var kind = loss == ViewFormatLoss.Cast ? GpuResourceEvent.ViewCastLost : GpuResourceEvent.ViewReinterpretLost;
        _textures.Counters.Add(kind);
        _warnings.Warn($"slot70-format-{texture.DesktopFormat}-{viewFormat}", $"[d3d12_xs] ID3D12Device.X::Slot70: a view in format {viewFormat} of a {Written(texture)} texture in format {texture.DesktopFormat} reads in {created} ({loss}: {Meaning(loss)}); counted as {kind}");
    }

    private static string Written(GpuTexture texture)
    {
        if (texture.GpuWritten)
        {
            return "GPU-written";
        }

        return "read-only";
    }

    private static string Meaning(ViewFormatLoss loss)
    {
        if (loss == ViewFormatLoss.Cast)
        {
            return "the sRGB decode or the integer reading is lost";
        }

        return "its bytes are not read as the view's format";
    }

    /// <summary>A buffer desc, or a texture desc without a format (raw or structured views): not a texture to make.</summary>
    private static bool IsBuffer(ResourceDescription* resource)
    {
        return resource != null && (resource->Dimension == ResourceDescription.BufferDimension || resource->Format == ViewFormats.Unknown);
    }

    private GpuTexture? Resolve(nint device, ResourceDescription* resource, AddressViewDescription* view)
    {
        ulong address = view->Address;

        if (resource == null || address == 0)
        {
            return null;
        }

        if (IsBuffer(resource))
        {
            return null;
        }

        var texture = _textures.Registry.Find(address);

        if (texture is not null && (texture.Matches(address, *resource) || (texture.GpuWritten && texture.SameExtent(address, *resource))))
        {
            _textures.Counters.Add(GpuResourceEvent.ViewResolved);
            _textures.Refresh(device, texture);

            return texture;
        }

        // A view inside a texture the GPU writes is not memory to read: making a texture there would retire it.
        if (texture is not null && texture.GpuWritten)
        {
            return null;
        }

        texture =_textures.CreateForView(device, address, resource, view->Format);

        if (texture is not null)
        {
            _textures.Counters.Add(GpuResourceEvent.ViewCreatedTexture);
        }

        return texture;
    }
}
