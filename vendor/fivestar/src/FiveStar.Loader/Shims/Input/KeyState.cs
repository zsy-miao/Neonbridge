// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Input;

/// <summary>GameInputKeyState: one pressed key.</summary>
[StructLayout(LayoutKind.Explicit, Size = GameInputLayout.KeyStateSize)]
internal struct KeyState
{
    [FieldOffset(GameInputLayout.KeyScanCodeOffset)]
    public uint ScanCode;

    [FieldOffset(GameInputLayout.KeyCodePointOffset)]
    public uint CodePoint;

    [FieldOffset(GameInputLayout.KeyVirtualKeyOffset)]
    public byte VirtualKey;

    [FieldOffset(GameInputLayout.KeyDeadKeyOffset)]
    public byte IsDeadKey;
}
