// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Input;

/// <summary>A RegisterDeviceCallback registration: which devices it watches and the native function to call.</summary>
/// <param name="Token">The GameInputCallbackToken handed back.</param>
/// <param name="Device">The watched device object, 0 for every device.</param>
/// <param name="Kind">GameInputKind bits a device must supply.</param>
/// <param name="StatusFilter">GameInputDeviceStatus bits whose changes are reported.</param>
/// <param name="Context">The caller's context pointer.</param>
/// <param name="Function">The GameInputDeviceCallback.</param>
internal sealed record DeviceCallback(ulong Token, nint Device, uint Kind, uint StatusFilter, nint Context, nint Function);
