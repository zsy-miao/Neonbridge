// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;
using System.Globalization;

namespace FiveStar.Loader.Shims.Input;

/// <summary>Key names of the input config and the Windows virtual-key codes they stand for (WinUser.h VK_*).</summary>
internal static class KeyNames
{
    public const string None = "none";

    public const int MouseLeft = 0x01;
    public const int MouseRight = 0x02;
    public const int MouseMiddle = 0x04;
    public const int MouseButton4 = 0x05;
    public const int MouseButton5 = 0x06;

    // VK_SHIFT, VK_CONTROL, VK_MENU: GetAsyncKeyState reports them besides the left and right keys.
    private const int Shift = 0x10;
    private const int Control = 0x11;
    private const int Alt = 0x12;

    private const int FirstFunctionKey = 0x70;
    private const int FunctionKeyCount = 24;
    private const string HexPrefix = "0x";

    private static readonly (string Name, int Key)[] Listed =
    [
        ("MouseLeft", MouseLeft), ("MouseRight", MouseRight), ("MouseMiddle", MouseMiddle), ("Mouse4", MouseButton4),
        ("Mouse5", MouseButton5), ("Backspace", 0x08), ("Tab", 0x09), ("Enter", 0x0D), ("CapsLock", 0x14),
        ("Escape", 0x1B), ("Space", 0x20), ("PageUp", 0x21), ("PageDown", 0x22), ("End", 0x23), ("Home", 0x24),
        ("Left", 0x25), ("Up", 0x26), ("Right", 0x27), ("Down", 0x28), ("Insert", 0x2D), ("Delete", 0x2E),
        ("ScrollLock", 0x91), ("LeftShift", 0xA0), ("RightShift", 0xA1), ("LeftCtrl", 0xA2), ("RightCtrl", 0xA3),
        ("LeftAlt", 0xA4), ("RightAlt", 0xA5),
    ];

    private static readonly FrozenDictionary<string, int> Names = BuildNames();

    /// <summary>The virtual key of a config name: a listed name, a letter, a digit, F1..F24 or a 0x-prefixed code; null for "none".</summary>
    public static int? Parse(string name)
    {
        if (name.Equals(None, StringComparison.OrdinalIgnoreCase))
        {
            return null;
        }

        if (Names.TryGetValue(name, out int key))
        {
            return key;
        }

        bool hex = name.StartsWith(HexPrefix, StringComparison.OrdinalIgnoreCase);

        if (hex && int.TryParse(name.AsSpan(HexPrefix.Length), NumberStyles.HexNumber, CultureInfo.InvariantCulture, out int code) && code > 0 && code < InputLimits.VirtualKeyCount)
        {
            return code;
        }

        throw new FormatException($"unknown key name '{name}'");
    }

    /// <summary>Whether a virtual key belongs in a keyboard reading: not a mouse button and not a side-less modifier.</summary>
    [HotPath]
    public static bool IsKeyboardKey(int virtualKey)
    {
        if (virtualKey <= MouseButton5)
        {
            return false;
        }

        return virtualKey != Shift && virtualKey != Control && virtualKey != Alt;
    }

    private static FrozenDictionary<string, int> BuildNames()
    {
        var names = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);

        foreach (var (name, key) in Listed)
        {
            names[name] = key;
        }

        // VK_0..VK_9 and VK_A..VK_Z equal their ASCII characters.
        for (char c = '0'; c <= '9'; c++)
        {
            names[c.ToString()] = c;
        }

        for (char c = 'A'; c <= 'Z'; c++)
        {
            names[c.ToString()] = c;
        }

        for (int i = 0; i < FunctionKeyCount; i++)
        {
            names[$"F{i + 1}"] = FirstFunctionKey + i;
        }

        return names.ToFrozenDictionary(StringComparer.OrdinalIgnoreCase);
    }
}
