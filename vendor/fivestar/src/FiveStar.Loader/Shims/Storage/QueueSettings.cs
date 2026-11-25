// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>A DSTORAGE_QUEUE_DESC as the game fills it. Capacity and priority are kept for the log only.</summary>
internal sealed record QueueSettings(bool MemorySource, ushort Capacity, sbyte Priority, string Name)
{
    public static unsafe QueueSettings Read(byte* desc)
    {
        bool memory = *(int*)(desc + DStorageLayout.DescSourceType) == DStorageLayout.SourceTypeMemory;
        ushort capacity = *(ushort*)(desc + DStorageLayout.DescCapacity);
        sbyte priority = *(sbyte*)(desc + DStorageLayout.DescPriority);
        nint name = *(nint*)(desc + DStorageLayout.DescName);

        return new QueueSettings(memory, capacity, priority, Marshal.PtrToStringAnsi(name) ?? string.Empty);
    }
}
