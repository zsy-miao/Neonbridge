// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>D3D12_COMMAND_QUEUE_DESC (d3d12.h): type, priority, flags, node mask; the D3D12.X slot 48 desc has the same 16 bytes.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct CommandQueueDescription
{
    public int Type;
    public int Priority;
    public int Flags;
    public uint NodeMask;
}
