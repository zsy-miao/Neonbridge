// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Shims.D3D12X.Binding;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Memory for command-list slot 148, the console's upload allocator (size, alignment): the caller writes the bytes on
/// the CPU and passes the same pointer to the GPU as an address (XboxBvhBuilder.dll 0x180020ed6 then 0x180020f99,
/// a root SRV). One persistently mapped upload buffer, made on the first allocation, whose CPU range is entered in
/// the address table as a console GPU range, so the pointer translates to the buffer's desktop address. Allocations
/// run round the buffer; a range is reused once the ring has passed it, as the user-data ring does.
/// </summary>
internal sealed unsafe class UploadArena
{
    /// <summary>Bytes in the ring.</summary>
    public const ulong Capacity = 32UL * Mebibyte;

    private const ulong Mebibyte = 1UL << 20;

    /// <summary>D3D12_RESOURCE_STATE_GENERIC_READ, the state of upload buffers.</summary>
    private const int GenericRead = 0xAC3;

    private readonly XboxObjects _objects;
    private readonly AddressTable _addresses;
    private readonly LoaderLog _log;
    private readonly object _gate = new();
    private byte* _data;
    private bool _failed;
    private ulong _next;

    public UploadArena(XboxObjects objects, AddressTable addresses, LoaderLog log)
    {
        _objects = objects;
        _addresses = addresses;
        _log = log;
    }

    /// <summary>
    /// <paramref name="size"/> bytes aligned to <paramref name="alignment"/> (a power of two, 0 or 1 for none); null
    /// when the request does not fit the ring or the buffer cannot be made.
    /// </summary>
    public byte* Allocate(ulong size, ulong alignment)
    {
        ulong align = Math.Max(alignment, 1);

        if (size == 0 || size > Capacity || (align & (align - 1)) != 0 || size + align > Capacity)
        {
            return null;
        }

        lock (_gate)
        {
            if (_data == null && !Create())
            {
                return null;
            }

            ulong start = (_next + align - 1) & ~(align - 1);

            if (start + size > Capacity)
            {
                start = 0;
            }

            _next = start + size;

            return _data + start;
        }
    }

    private bool Create()
    {
        if (_failed)
        {
            return false;
        }

        var properties = new HeapProperties { Type = HeapProperties.Upload };
        nint buffer = DesktopCalls.CreateBuffer(_objects.DesktopDevice, properties, Capacity, 0, GenericRead);
        byte* data = buffer == 0 ? null : DesktopCalls.Map(buffer);

        if (data == null)
        {
            _failed = true;
            _log.Warn("[d3d12_xs] list slot 148: no upload buffer; allocations return null");

            return false;
        }

        _addresses.Add(new AddressRange((ulong)data, Capacity, DesktopCalls.GpuAddress(buffer), buffer));
        _data = data;
        _log.Info($"[d3d12_xs] list slot 148: upload ring of {Capacity / Mebibyte} MiB at 0x{(ulong)data:x}");

        return true;
    }
}
