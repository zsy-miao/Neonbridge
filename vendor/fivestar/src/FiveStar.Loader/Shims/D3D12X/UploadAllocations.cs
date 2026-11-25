// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Command-list slot 148: void* Allocate(UINT32 size, UINT32 alignment), memory the CPU writes and the GPU reads at the
/// same address (<see cref="UploadArena"/>). XboxBvhBuilder.dll calls it with (count * 40, 16) and treats null as
/// failure ("Failed to allocate upload buffer"); the game's executable has no call site.
/// </summary>
internal static unsafe class UploadAllocations
{
    public const int AllocateSlot = 148;

    public static void Bind(XboxObjects objects)
    {
        delegate* unmanaged<XboxObject*, uint, uint, byte*> allocate = &Allocate;
        objects.SetHandler(XboxInterfaceKind.CommandList, AllocateSlot, (nint)allocate);
    }

    [UnmanagedCallersOnly]
    public static byte* Allocate(XboxObject* self, uint size, uint alignment)
    {
        try
        {
            return ShimContext.Runtime.D3D12X.Uploads.Allocate(size, alignment);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "UploadAllocations.Allocate");

            return default;
        }
    }
}
