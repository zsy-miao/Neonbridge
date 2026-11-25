// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Input;

/// <summary>One IGameInputDevice (our own layout): its GameInputDeviceInfo block, its status and its input kinds.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct DeviceObject
{
    public ObjectHeader Header;
    public nint Info;
    public uint Status;
    public uint Kind;
}
