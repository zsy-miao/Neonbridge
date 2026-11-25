// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Graphics.Rendering;
using FiveStar.Loader.Shims.Xg;
using Vortice.Direct3D12;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// Fills a desktop texture from console memory: each subresource is detiled with its XG layout
/// (<see cref="XgCopier.From"/>) into an upload buffer at the desktop's copyable footprint, copied into the texture
/// on a private direct queue between transitions out of and back into its resting state, and waited for. Uploads run
/// one at a time; the upload buffer grows to the largest texture seen. A fill the desktop refuses (no memory for the
/// upload buffer: it is system memory, which heaps over the game's sections share, docs/d3d12x-shim.md) or that does not
/// finish fails without an exception: the uploader is called from the game's threads, and an exception there ended the
/// process (0xE0434352). The next fill starts with a new upload buffer and queue.
/// </summary>
internal sealed unsafe class GpuTextureUploader : IDisposable
{
    /// <summary>D3D12_RESOURCE_STATE_COPY_DEST (d3d12.h).</summary>
    private const int CopyDestState = 0x400;

    /// <summary>Subresources one upload handles: 15 mips times the array slices XG lays out (docs/xg.md).</summary>
    private const int MaxSubresources = XgLimits.MaxMips * 64;

    private readonly object _gate = new();
    private readonly PlacedSubresourceFootPrint[] _footprints = new PlacedSubresourceFootPrint[MaxSubresources];
    private readonly uint[] _rows = new uint[MaxSubresources];
    private readonly ulong[] _rowSizes = new ulong[MaxSubresources];
    private ID3D12Device? _device;
    private CommandContext? _context;
    private ID3D12Resource? _staging;
    private byte* _stagingData;
    private ulong _stagingSize;

    /// <summary>Makes the upload buffer of a size; the desktop's committed upload buffer (tests make it fail).</summary>
    public Func<ID3D12Device, ulong, ID3D12Resource> CreateStaging { get; set; } = UploadBuffer;

    /// <summary>Why the last fill failed, or null.</summary>
    public string? Failure { get; private set; }

    /// <summary>Copies <paramref name="source"/> (the texture's console bytes) into <paramref name="texture"/>; false when it cannot.</summary>
    public bool Upload(nint device, GpuTexture texture, byte* source)
    {
        lock (_gate)
        {
            Failure = null;

            try
            {
                return Fill(device, texture, source);
            }
            catch (Exception exception) when (exception is SharpGen.Runtime.SharpGenException or InvalidOperationException or TimeoutException)
            {
                Failure = exception.Message;
                Discard();

                return false;
            }
        }
    }

    public void Dispose()
    {
        Discard();
        _device?.Dispose();
    }

    private bool Fill(nint device, GpuTexture texture, byte* source)
    {
        var context = Context(device);
        Marshal.AddRef(texture.Desktop);
        using var resource = new ID3D12Resource(texture.Desktop);
        var description = resource.Description;
        int count = description.MipLevels;

        if (description.Dimension != ResourceDimension.Texture3D)
        {
            count *= description.DepthOrArraySize;
        }

        if (count > MaxSubresources || count > texture.Layout.Planes[0].MipCount * texture.Layout.Planes[0].Slices)
        {
            return false;
        }

        _device!.GetCopyableFootprints(description, 0, (uint)count, 0, _footprints, _rows, _rowSizes, out ulong total);

        if (!Fits(texture, count))
        {
            return false;
        }

        EnsureStaging(total);
        Detile(texture, source, count);
        Record(context.List, resource, texture.State, count);
        context.ExecuteAndWait();

        return true;
    }

    /// <summary>Drops the upload buffer and the queue, so the next fill makes new ones.</summary>
    private void Discard()
    {
        _staging?.Dispose();
        _staging = null;
        _stagingData = null;
        _stagingSize = 0;

        _context?.Dispose();
        _context = null;
    }

    private CommandContext Context(nint device)
    {
        if (_context is not null)
        {
            return _context;
        }

        if (_device is null)
        {
            Marshal.AddRef(device);
            _device = new ID3D12Device(device);
        }

        _context = new CommandContext(_device);

        return _context;
    }

    private void EnsureStaging(ulong size)
    {
        if (size <= _stagingSize)
        {
            return;
        }

        // A failed creation or Map leaves no upload buffer, never the disposed one.
        _staging?.Dispose();
        _staging = null;
        _stagingSize = 0;

        _staging = CreateStaging(_device!, size);
        void* data;
        _staging.Map(0, null, &data).CheckError();
        _stagingData = (byte*)data;
        _stagingSize = size;
    }

    private static ID3D12Resource UploadBuffer(ID3D12Device device, ulong size)
    {
        return device.CreateCommittedResource(new HeapProperties(HeapType.Upload), HeapFlags.None, Vortice.Direct3D12.ResourceDescription.Buffer(size), ResourceStates.GenericRead);
    }

    /// <summary>Whether every XG mip's rows fit the desktop footprint (element size and block rows agree).</summary>
    private bool Fits(GpuTexture texture, int count)
    {
        var layout = texture.Layout;
        ref readonly var plane = ref layout.Planes[0];

        for (int subresource = 0; subresource < count; subresource++)
        {
            ref readonly var mip = ref plane.Mips[subresource % plane.MipCount];
            ulong rowBytes = (ulong)mip.WidthElements * (ulong)plane.BytesPerElement;

            if (rowBytes > _rowSizes[subresource] || mip.HeightElements > _rows[subresource])
            {
                return false;
            }
        }

        return true;
    }

    private void Detile(GpuTexture texture, byte* source, int count)
    {
        var layout = texture.Layout;

        for (int subresource = 0; subresource < count; subresource++)
        {
            ref readonly var footprint = ref _footprints[subresource];
            uint rowPitch = footprint.Footprint.RowPitch;
            uint slicePitch = rowPitch * _rows[subresource];
            XgCopier.From(layout, _stagingData + footprint.Offset, 0, (uint)subresource, source, rowPitch, slicePitch);
        }
    }

    private void Record(ID3D12GraphicsCommandList list, ID3D12Resource resource, int state, int count)
    {
        var resting = (ResourceStates)state;
        bool transition = state != CopyDestState;

        if (transition)
        {
            list.ResourceBarrierTransition(resource, resting, ResourceStates.CopyDest);
        }

        for (int subresource = 0; subresource < count; subresource++)
        {
            var source = new TextureCopyLocation(_staging!, _footprints[subresource]);
            list.CopyTextureRegion(new TextureCopyLocation(resource, (uint)subresource), 0, 0, 0, source, null);
        }

        if (transition)
        {
            list.ResourceBarrierTransition(resource, ResourceStates.CopyDest, resting);
        }
    }
}
