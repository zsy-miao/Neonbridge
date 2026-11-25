// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The 16-byte desc of device slot 73. The game passes (0, 2, 1, 0) for a list on a type-0 (direct) allocator, so the
/// first dword is read as D3D12_COMMAND_LIST_TYPE; the other three are unknown.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct CommandListDescription
{
    public int Type;
    public uint Unknown1;
    public uint Unknown2;
    public uint Unknown3;
}
