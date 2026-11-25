// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;
using System.Globalization;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// Reads the input config: one "setting value..." line each, '#' starting a comment. A setting that is not given
/// keeps its default (<see cref="Defaults"/>).
/// </summary>
internal static class InputConfigFile
{
    private const char CommentMark = '#';
    private const string On = "on";
    private const string Off = "off";
    private const float DefaultSensitivity = 1.0f;
    private const float DefaultCurve = 1.0f;

    private static readonly FrozenDictionary<string, uint> ButtonNames = new Dictionary<string, uint>
    {
        ["a"] = GameInputLayout.GamepadA,
        ["b"] = GameInputLayout.GamepadB,
        ["x"] = GameInputLayout.GamepadX,
        ["y"] = GameInputLayout.GamepadY,
        ["menu"] = GameInputLayout.GamepadMenu,
        ["view"] = GameInputLayout.GamepadView,
        ["left-shoulder"] = GameInputLayout.GamepadLeftShoulder,
        ["right-shoulder"] = GameInputLayout.GamepadRightShoulder,
        ["left-thumb"] = GameInputLayout.GamepadLeftThumbstick,
        ["right-thumb"] = GameInputLayout.GamepadRightThumbstick,
        ["dpad-up"] = GameInputLayout.GamepadDPadUp,
        ["dpad-down"] = GameInputLayout.GamepadDPadDown,
        ["dpad-left"] = GameInputLayout.GamepadDPadLeft,
        ["dpad-right"] = GameInputLayout.GamepadDPadRight,
    }.ToFrozenDictionary(StringComparer.OrdinalIgnoreCase);

    /// <summary>
    /// WASD on the left stick, the mouse on the right stick, Space/C/R/F on A/B/X/Y, Q/E on the shoulders, Shift/V on
    /// the stick buttons, the arrow keys on the d-pad, Escape/Tab on Menu/View, the right and left mouse buttons on the
    /// left and right triggers, Scroll Lock to switch the virtual gamepad on and off.
    /// </summary>
    public static InputConfig Defaults()
    {
        ButtonBinding[] buttons =
        [
            Bind("a", "Space"), Bind("b", "C"), Bind("x", "R"), Bind("y", "F"),
            Bind("left-shoulder", "Q"), Bind("right-shoulder", "E"), Bind("left-thumb", "LeftShift"),
            Bind("right-thumb", "V"), Bind("menu", "Escape"), Bind("view", "Tab"), Bind("dpad-up", "Up"),
            Bind("dpad-down", "Down"), Bind("dpad-left", "Left"), Bind("dpad-right", "Right"),
        ];
        var stick = new StickKeys(KeyNames.Parse("W"), KeyNames.Parse("A"), KeyNames.Parse("S"), KeyNames.Parse("D"));

        return new InputConfig(true, KeyNames.Parse("ScrollLock"), stick, DefaultSensitivity, DefaultCurve, buttons, KeyNames.MouseRight, KeyNames.MouseLeft);
    }

    public static InputConfig Load(string path)
    {
        return Parse(File.ReadLines(path));
    }

    public static InputConfig Parse(IEnumerable<string> lines)
    {
        var config = Defaults();

        foreach (string raw in lines)
        {
            string[] fields = raw.Split(CommentMark)[0].Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries);

            if (fields.Length == 0)
            {
                continue;
            }

            if (fields.Length == 1)
            {
                throw new FormatException($"input config: '{fields[0]}' needs a value");
            }

            config = Apply(config, fields[0].ToLowerInvariant(), fields[1..]);
        }

        return config;
    }

    private static InputConfig Apply(InputConfig config, string setting, string[] values)
    {
        switch (setting)
        {
            case "virtual-gamepad":
                return config with { VirtualGamepad = Switch(values[0]) };
            case "toggle":
                return config with { ToggleKey = KeyNames.Parse(values[0]) };
            case "left-stick":
                return config with { LeftStick = Stick(values) };
            case "mouse-sensitivity":
                return config with { MouseSensitivity = Number(values[0]) };
            case "mouse-curve":
                return config with { MouseCurve = Number(values[0]) };
            case "left-trigger":
                return config with { LeftTrigger = KeyNames.Parse(values[0]) };
            case "right-trigger":
                return config with { RightTrigger = KeyNames.Parse(values[0]) };
            default:
                return config with { Buttons = Rebind(config.Buttons, setting, values[0]) };
        }
    }

    private static bool Switch(string value)
    {
        if (value.Equals(On, StringComparison.OrdinalIgnoreCase))
        {
            return true;
        }

        if (value.Equals(Off, StringComparison.OrdinalIgnoreCase))
        {
            return false;
        }

        throw new FormatException($"input config: expected on or off, got '{value}'");
    }

    private static StickKeys Stick(string[] values)
    {
        if (values is not [string up, string left, string down, string right])
        {
            throw new FormatException("input config: left-stick takes four keys: up left down right");
        }

        return new StickKeys(KeyNames.Parse(up), KeyNames.Parse(left), KeyNames.Parse(down), KeyNames.Parse(right));
    }

    private static float Number(string value)
    {
        return float.Parse(value, NumberStyles.Float, CultureInfo.InvariantCulture);
    }

    private static ButtonBinding[] Rebind(IReadOnlyList<ButtonBinding> buttons, string setting, string key)
    {
        var binding = Bind(setting, key);

        return [.. buttons.Where(b => b.Button != binding.Button), binding];
    }

    private static ButtonBinding Bind(string button, string key)
    {
        if (!ButtonNames.TryGetValue(button, out uint bit))
        {
            throw new FormatException($"input config: unknown setting '{button}'");
        }

        return new ButtonBinding(bit, KeyNames.Parse(key));
    }
}
