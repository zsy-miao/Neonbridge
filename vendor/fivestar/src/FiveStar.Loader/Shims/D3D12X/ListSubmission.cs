// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Where a command list was last submitted, kept in its wrapper past the binding state: the queue's shim fence and
/// the value that fence reaches once the submission has finished. Reset does not clear it.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct ListSubmission
{
    /// <summary>Offset in the command-list wrapper, after <see cref="Binding.CommandListBindings"/> (0x800 to 0xC28).</summary>
    public const int Offset = 0xE00;

    public nint Fence;
    public ulong Value;

    public static ListSubmission* Of(nint list)
    {
        return (ListSubmission*)((byte*)list + Offset);
    }
}
