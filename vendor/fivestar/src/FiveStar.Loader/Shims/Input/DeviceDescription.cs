// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Input;

/// <summary>What the GameInputDeviceInfo of one device says.</summary>
/// <param name="Name">The display name; its SHA-256 is the device id.</param>
/// <param name="Family">GameInputDeviceFamily.</param>
/// <param name="Kind">The GameInputKind bits the device supplies.</param>
/// <param name="RumbleMotors">GameInputRumbleMotors bits.</param>
/// <param name="VendorId">USB vendor id, 0 for virtual devices.</param>
/// <param name="ProductId">USB product id, 0 for virtual devices.</param>
/// <param name="UsageId">HID usage on the Generic Desktop page.</param>
internal sealed record DeviceDescription(
    string Name,
    int Family,
    uint Kind,
    uint RumbleMotors,
    ushort VendorId,
    ushort ProductId,
    ushort UsageId);
