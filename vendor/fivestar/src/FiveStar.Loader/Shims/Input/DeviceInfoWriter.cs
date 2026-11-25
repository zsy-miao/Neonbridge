// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// Writes the GameInputDeviceInfo of a device into one native block: the info, then the gamepad, keyboard or mouse
/// info it points at, then the display name. The block lives as long as the service.
/// </summary>
internal static unsafe class DeviceInfoWriter
{
    /// <summary>HID usage page Generic Desktop.</summary>
    private const ushort GenericDesktopPage = 0x01;

    private const uint AllMouseButtons = GameInputLayout.MouseLeft | GameInputLayout.MouseRight | GameInputLayout.MouseMiddle | GameInputLayout.MouseButton4 | GameInputLayout.MouseButton5;

    private const int GamepadPart = GameInputLayout.DeviceInfoSize;
    private const int KeyboardPart = GamepadPart + GameInputLayout.GamepadInfoSize;
    private const int MousePart = KeyboardPart + GameInputLayout.KeyboardInfoSize;
    private const int StringPart = MousePart + GameInputLayout.MouseInfoSize;
    private const int NamePart = StringPart + GameInputLayout.StringSize;

    /// <summary>The APP_LOCAL_DEVICE_ID of a device: the SHA-256 of its name, stable across runs.</summary>
    public static byte[] DeviceId(string name)
    {
        return SHA256.HashData(Encoding.UTF8.GetBytes(name));
    }

    public static nint Write(DeviceDescription device)
    {
        byte[] name = Encoding.UTF8.GetBytes(device.Name);
        byte* block = (byte*)NativeMemory.AllocZeroed((nuint)(NamePart + name.Length + 1));
        WriteIdentity(block, device);

        if ((device.Kind & GameInputLayout.KindGamepad) != 0)
        {
            WriteGamepad(block);
        }

        if ((device.Kind & GameInputLayout.KindKeyboard) != 0)
        {
            WriteKeyboard(block);
        }

        if ((device.Kind & GameInputLayout.KindMouse) != 0)
        {
            WriteMouse(block);
        }

        name.CopyTo(new Span<byte>(block + NamePart, name.Length));
        byte* text = block + StringPart;
        *(uint*)(text + GameInputLayout.StringSizeOffset) = (uint)name.Length;
        *(uint*)(text + GameInputLayout.StringCodePointsOffset) = (uint)device.Name.EnumerateRunes().Count();
        *(nint*)(text + GameInputLayout.StringDataOffset) = (nint)(block + NamePart);
        *(nint*)(block + GameInputLayout.InfoDisplayNameOffset) = (nint)text;

        return (nint)block;
    }

    private static void WriteIdentity(byte* info, DeviceDescription device)
    {
        byte[] id = DeviceId(device.Name);
        *(uint*)(info + GameInputLayout.InfoSizeOffset) = GameInputLayout.DeviceInfoSize;
        *(ushort*)(info + GameInputLayout.InfoVendorIdOffset) = device.VendorId;
        *(ushort*)(info + GameInputLayout.InfoProductIdOffset) = device.ProductId;
        *(ushort*)(info + GameInputLayout.InfoUsagePageOffset) = GenericDesktopPage;
        *(ushort*)(info + GameInputLayout.InfoUsageIdOffset) = device.UsageId;

        id.CopyTo(new Span<byte>(info + GameInputLayout.InfoDeviceIdOffset, GameInputLayout.DeviceIdSize));
        id.CopyTo(new Span<byte>(info + GameInputLayout.InfoDeviceRootIdOffset, GameInputLayout.DeviceIdSize));
        *(int*)(info + GameInputLayout.InfoFamilyOffset) = device.Family;
        *(uint*)(info + GameInputLayout.InfoSupportedInputOffset) = device.Kind;
        *(uint*)(info + GameInputLayout.InfoRumbleMotorsOffset) = device.RumbleMotors;
    }

    private static void WriteGamepad(byte* info)
    {
        int* labels = (int*)(info + GamepadPart);

        for (int i = 0; i < GameInputLayout.GamepadInfoLabels; i++)
        {
            labels[i] = GameInputLayout.XboxGamepadLabels[i];
        }

        *(nint*)(info + GameInputLayout.InfoGamepadInfoOffset) = (nint)labels;
    }

    private static void WriteKeyboard(byte* info)
    {
        byte* keyboard = info + KeyboardPart;
        *(int*)(keyboard + GameInputLayout.KeyboardKindOffset) = GameInputLayout.KeyboardUnknown;
        *(uint*)(keyboard + GameInputLayout.KeyboardKeyCountOffset) = InputLimits.VirtualKeyCount;
        *(uint*)(keyboard + GameInputLayout.KeyboardMaxKeysOffset) = InputLimits.MaxKeys;
        *(nint*)(info + GameInputLayout.InfoKeyboardInfoOffset) = (nint)keyboard;
    }

    private static void WriteMouse(byte* info)
    {
        byte* mouse = info + MousePart;
        *(uint*)(mouse + GameInputLayout.MouseSupportedButtonsOffset) = AllMouseButtons;
        *(nint*)(info + GameInputLayout.InfoMouseInfoOffset) = (nint)mouse;
    }
}
