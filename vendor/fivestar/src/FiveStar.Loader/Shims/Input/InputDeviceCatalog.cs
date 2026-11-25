// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// The devices the shim exposes, in index order: one gamepad per source slot, then the keyboard, the mouse and the
/// keyboard-and-mouse virtual gamepad.
/// </summary>
internal static class InputDeviceCatalog
{
    public const int KeyboardAfterSlots = 0;
    public const int MouseAfterSlots = 1;
    public const int VirtualAfterSlots = 2;
    public const int DevicesAfterSlots = 3;

    /// <summary>USB vendor id of Microsoft.</summary>
    private const ushort MicrosoftVendor = 0x045E;

    /// <summary>USB product id of the Xbox Series X|S controller.</summary>
    private const ushort XboxSeriesController = 0x0B12;

    // HID usages on the Generic Desktop page.
    private const ushort MouseUsage = 0x02;
    private const ushort GamepadUsage = 0x05;
    private const ushort KeyboardUsage = 0x06;

    private const uint Rumble = GameInputLayout.RumbleLowFrequency | GameInputLayout.RumbleHighFrequency;

    public static DeviceDescription[] Describe(int slots)
    {
        var devices = new DeviceDescription[slots + DevicesAfterSlots];

        for (int slot = 0; slot < slots; slot++)
        {
            devices[slot] = new DeviceDescription(GamepadName(slot), GameInputLayout.FamilyXboxOne, GameInputLayout.KindGamepad, Rumble, MicrosoftVendor, XboxSeriesController, GamepadUsage);
        }

        devices[slots + KeyboardAfterSlots] = new DeviceDescription("Keyboard", GameInputLayout.FamilyHid, GameInputLayout.KindKeyboard, 0, 0, 0, KeyboardUsage);
        devices[slots + MouseAfterSlots] = new DeviceDescription("Mouse", GameInputLayout.FamilyHid, GameInputLayout.KindMouse, 0, 0, 0, MouseUsage);
        devices[slots + VirtualAfterSlots] = new DeviceDescription("Keyboard and mouse gamepad", GameInputLayout.FamilyVirtual, GameInputLayout.KindGamepad, 0, 0, 0, GamepadUsage);

        return devices;
    }

    /// <summary>The name of the gamepad in source slot <paramref name="slot"/> (0-based); its device id is derived from it.</summary>
    public static string GamepadName(int slot)
    {
        return string.Create(CultureInfo.InvariantCulture, $"XInput gamepad {slot + 1}");
    }
}
