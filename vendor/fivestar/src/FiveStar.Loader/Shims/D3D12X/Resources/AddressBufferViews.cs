// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Binding;
using FiveStar.Loader.Shims.Xg;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// Device slot 70 with a buffer desc: a buffer SRV (raw, structured or typed) at a console GPU address. The address
/// lies in a desktop buffer over game memory (<see cref="XboxGameMemoryBuffers.TryFind"/>); the view's first element
/// moves by the address's offset in that buffer. An address outside known memory, off the element size or past the
/// buffer's end gets no view (the caller writes a null one), as does a raw view whose first byte is off the 16 bytes
/// D3D12 requires (D3D12_RAW_UAV_SRV_BYTE_ALIGNMENT): such a view is invalid, and a driver may move it.
/// </summary>
internal sealed unsafe class AddressBufferViews
{
    /// <summary>ID3D12Device::CreateShaderResourceView (d3d12.h).</summary>
    private const int DesktopShaderResourceViewSlot = 18;

    /// <summary>Bytes of a raw view's element (R32_TYPELESS).</summary>
    private const uint RawElementBytes = sizeof(uint);

    /// <summary>D3D12_RAW_UAV_SRV_BYTE_ALIGNMENT.</summary>
    private const uint RawAlignment = 16;

    private readonly XboxGameMemoryBuffers _memory;

    public AddressBufferViews(XboxGameMemoryBuffers memory)
    {
        _memory = memory;
    }

    /// <summary>
    /// Writes the buffer view into <paramref name="handle"/>, raw when its <paramref name="record"/> keeps the heap index
    /// (<see cref="Binding.RawBufferViews"/>); false when the address has no desktop buffer.
    /// </summary>
    [HotPath]
    public bool Create(nint device, AddressViewDescription* view, nint handle, Binding.BufferRecord record)
    {
        BufferViewDescription desktop;
        Buffer.MemoryCopy(view, &desktop, sizeof(BufferViewDescription), AddressViewDescription.DesktopViewSize);
        uint element = ElementBytes(desktop);

        if (desktop.Dimension != BufferViewDescription.BufferDimension || element == 0 || !_memory.TryFind(view->Address, out var range) || range.Resource == 0)
        {
            return false;
        }

        ulong offset = view->Address - range.GameAddress;
        ulong end = offset + ((desktop.SrvFirstElement + desktop.SrvElements) * element);

        if (offset % element != 0 || end > range.Size)
        {
            return false;
        }

        desktop.SrvFirstElement += offset / element;

        if ((desktop.SrvFlags & BufferViewDescription.RawFlag) != 0 && (desktop.SrvFirstElement * RawElementBytes) % RawAlignment != 0)
        {
            return false;
        }

        var create = (delegate* unmanaged<nint, nint, BufferViewDescription*, nint, void>)ComObject.Slot(device, DesktopShaderResourceViewSlot);
        nint target = range.Resource;
        BufferViewDescription raw;
        var written = (BufferViewDescription*)Binding.RawBufferViews.View(&desktop, (byte*)&raw, false, record, ref target);
        create(device, target, written, handle);

        return true;
    }

    /// <summary>
    /// A raw SRV of <paramref name="size"/> bytes at console address <paramref name="address"/>, written for a
    /// constant-buffer view in one of the game's title-memory heaps: translated programs read every V#, a CBV's too, as
    /// a ByteAddressBuffer. False when the address has no desktop buffer, the range leaves it or it starts off 16 bytes.
    /// </summary>
    [HotPath]
    public bool CreateRaw(nint device, ulong address, uint size, nint handle)
    {
        if (!_memory.TryFind(address, out var range) || range.Resource == 0)
        {
            return false;
        }

        ulong offset = address - range.GameAddress;
        ulong bytes = range.Size - offset;

        if (size < bytes)
        {
            bytes = size;
        }

        if (offset % RawAlignment != 0 || bytes < RawElementBytes)
        {
            return false;
        }

        var desktop = BufferViewDescription.RawShaderResource((uint)(bytes / RawElementBytes));
        desktop.SrvFirstElement = offset / RawElementBytes;
        var create = (delegate* unmanaged<nint, nint, BufferViewDescription*, nint, void>)ComObject.Slot(device, DesktopShaderResourceViewSlot);
        create(device, range.Resource, &desktop, handle);

        return true;
    }

    /// <summary>Bytes per element: 4 for raw views, the stride of structured ones, the format's size for typed ones; 0 when unknown.</summary>
    [HotPath]
    private static uint ElementBytes(in BufferViewDescription view)
    {
        if ((view.SrvFlags & BufferViewDescription.RawFlag) != 0)
        {
            return RawElementBytes;
        }

        if (view.SrvStride != 0)
        {
            return view.SrvStride;
        }

        if (view.Format == ViewFormats.Unknown)
        {
            return 0;
        }

        return (uint)XgFormats.Find((uint)view.Format).BytesPerElement;
    }
}
