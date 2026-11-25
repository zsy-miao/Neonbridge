// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// One IGameInputReading in the history ring (our own layout). <see cref="Sequence"/> is 0 while the slot is empty or
/// being written; the state getters copy straight out of the object.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct ReadingObject
{
    public ObjectHeader Header;
    public long Sequence;
    public ulong Timestamp;
    public nint Device;
    public uint Kind;
    public int KeyCount;
    public GamepadState Gamepad;
    public MouseState Mouse;
    public KeyBuffer Keys;
}
