// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Graphics.Rendering;
using Vortice.Direct3D12;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// Debug readback of a desktop texture's subresource 0: copied on a private direct queue into a readback buffer,
/// between transitions out of and back into the state the texture rests in, and waited for. The caller makes sure no
/// queue is using the texture (it waited for the presenting queue to go idle). Not for per-frame use.
/// </summary>
internal sealed unsafe class TextureReadback : IDisposable
{
    private readonly object _gate = new();
    private readonly PlacedSubresourceFootPrint[] _footprint = new PlacedSubresourceFootPrint[1];
    private readonly uint[] _rows = new uint[1];
    private readonly ulong[] _rowSize = new ulong[1];
    private ID3D12Device? _device;
    private CommandContext? _context;

    /// <summary>The texels of <paramref name="resource"/> resting in <paramref name="state"/>; null for a buffer, 3D or MSAA texture.</summary>
    public ReadbackImage? Read(nint device, nint resource, int state)
    {
        lock (_gate)
        {
            var context = Context(device);
            Marshal.AddRef(resource);
            using var texture = new ID3D12Resource(resource);
            var description = texture.Description;

            if (description.Dimension != ResourceDimension.Texture2D || description.SampleDescription.Count != 1)
            {
                return null;
            }

            _device!.GetCopyableFootprints(description, 0, 1, 0, _footprint, _rows, _rowSize, out ulong total);
            using var buffer = _device.CreateCommittedResource(new HeapProperties(HeapType.Readback), HeapFlags.None, Vortice.Direct3D12.ResourceDescription.Buffer(total), ResourceStates.CopyDest);
            Record(context.List, texture, buffer, (ResourceStates)state);
            context.ExecuteAndWait();

            return Copy(buffer, description);
        }
    }

    public void Dispose()
    {
        _context?.Dispose();
        _device?.Dispose();
    }

    private CommandContext Context(nint device)
    {
        if (_context is not null)
        {
            return _context;
        }

        Marshal.AddRef(device);
        _device = new ID3D12Device(device);
        _context = new CommandContext(_device);

        return _context;
    }

    private void Record(ID3D12GraphicsCommandList list, ID3D12Resource texture, ID3D12Resource buffer, ResourceStates resting)
    {
        bool transition = resting != ResourceStates.CopySource;

        if (transition)
        {
            list.ResourceBarrierTransition(texture, resting, ResourceStates.CopySource);
        }

        list.CopyTextureRegion(new TextureCopyLocation(buffer, _footprint[0]), 0, 0, 0, new TextureCopyLocation(texture, 0), null);

        if (transition)
        {
            list.ResourceBarrierTransition(texture, ResourceStates.CopySource, resting);
        }
    }

    private ReadbackImage Copy(ID3D12Resource buffer, Vortice.Direct3D12.ResourceDescription description)
    {
        int rows = (int)_rows[0];
        int rowBytes = (int)_rowSize[0];
        int pitch = (int)_footprint[0].Footprint.RowPitch;
        var pixels = new byte[rowBytes * rows];
        void* data;
        buffer.Map(0, null, &data).CheckError();

        for (int row = 0; row < rows; row++)
        {
            new ReadOnlySpan<byte>((byte*)data + (row * pitch), rowBytes).CopyTo(pixels.AsSpan(row * rowBytes));
        }

        buffer.Unmap(0);

        return new ReadbackImage((int)description.Width, rows, (int)description.Format, rowBytes, pixels);
    }
}
