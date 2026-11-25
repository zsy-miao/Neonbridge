// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// A descriptor-heap wrapper: <see cref="XboxObject"/> plus the start of the title memory that stands for its CPU
/// descriptors (see <see cref="TitleMemoryHooks"/>), which slot 9 returns, and the GPU handle slot 10 returns; 0 when the desktop handles are used.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct XboxDescriptorHeapObject
{
    public XboxObject Header;
    public nint GameCpuStart;
    public nint GameGpuStart;
}
