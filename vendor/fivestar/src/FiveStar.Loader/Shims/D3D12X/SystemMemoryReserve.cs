// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Shims.D3D12X.Binding;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// <c>--reserve-gpu-system-memory &lt;GB&gt;</c> (debugging): upload buffers of that size made at device creation and
/// kept, so the GPU's system-memory budget is as tight as on a machine with less RAM: what is left to the game is the
/// budget less this. Unlike <c>--section-heap-budget</c>, which only caps the section heaps, every system-memory
/// allocation then meets the limit, as on that machine. The buffers are resident memory: reserving takes that much RAM.
/// </summary>
internal static class SystemMemoryReserve
{
    /// <summary>One buffer's size.</summary>
    private const ulong BufferBytes = 1UL << 30;

    /// <summary>D3D12_RESOURCE_STATE_GENERIC_READ, the state of upload buffers.</summary>
    private const int GenericRead = 0xAC3;

    private const double BytesPerGibibyte = 1UL << 30;

    private static readonly List<nint> Buffers = [];

    /// <summary>Bytes to reserve; 0 for none.</summary>
    public static ulong Bytes { get; set; }

    /// <summary>Makes the buffers on <paramref name="device"/>; logs the budget after.</summary>
    public static void Take(nint device, GpuMemoryBudget? budget, LoaderLog log)
    {
        ulong taken = 0;

        while (taken < Bytes)
        {
            ulong size = Math.Min(BufferBytes, Bytes - taken);
            var properties = new HeapProperties { Type = HeapProperties.Upload };
            nint buffer = DesktopCalls.CreateBuffer(device, properties, size, 0, GenericRead);

            if (buffer == 0)
            {
                break;
            }

            Buffers.Add(buffer);
            taken += size;
        }

        if (Bytes != 0)
        {
            log.Warn($"[d3d12_xs] --reserve-gpu-system-memory: {taken / BytesPerGibibyte:F2} GiB of upload buffers held; GPU {budget?.Describe() ?? "budget unknown"}");
        }
    }
}
