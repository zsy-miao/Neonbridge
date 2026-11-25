// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The device wrapper: <see cref="XboxObject"/>, then the byte at +0x18 that the game (0x14006ddf5, 0x144d9f5dc,
/// 0x144fd2b7e) and the FSR DLL read before writing a descriptor. Zero: they write a raw buffer descriptor or copy the
/// 32-byte record themselves; non-zero: they call device slot 17 (CreateConstantBufferView) or 24
/// (CopyDescriptorsSimple). The shim sets it, since its records hold desktop indices, not hardware descriptors.
/// </summary>
[StructLayout(LayoutKind.Explicit, Size = 0x20)]
internal struct XboxDeviceObject
{
    /// <summary>The value of <see cref="DescriptorCalls"/>: descriptors go through the device's slots.</summary>
    public const byte ThroughSlots = 1;

    [FieldOffset(0)]
    public XboxObject Header;

    [FieldOffset(0x18)]
    public byte DescriptorCalls;
}
