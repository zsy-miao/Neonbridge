// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Input;

/// <summary>The keyboard-and-mouse virtual gamepad: whether it is on and how keys and mouse motion map onto it.</summary>
/// <param name="VirtualGamepad">Whether the virtual gamepad device is connected at start.</param>
/// <param name="ToggleKey">A key that connects and disconnects the virtual gamepad; null for none.</param>
/// <param name="LeftStick">The keys of the left stick.</param>
/// <param name="MouseSensitivity">Right-stick deflection per 1000 pixels per second of mouse motion.</param>
/// <param name="MouseCurve">Exponent applied to the deflection after it is clamped to 1 (1 is linear).</param>
/// <param name="Buttons">The button bindings.</param>
/// <param name="LeftTrigger">The key that pulls the left trigger fully.</param>
/// <param name="RightTrigger">The key that pulls the right trigger fully.</param>
internal sealed record InputConfig(
    bool VirtualGamepad,
    int? ToggleKey,
    StickKeys LeftStick,
    float MouseSensitivity,
    float MouseCurve,
    IReadOnlyList<ButtonBinding> Buttons,
    int? LeftTrigger,
    int? RightTrigger);
