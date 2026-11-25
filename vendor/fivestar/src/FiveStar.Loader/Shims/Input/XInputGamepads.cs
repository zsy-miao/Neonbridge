// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// The four XInput slots as gamepads. Sticks are scaled to -1..1 and triggers to 0..1 without dead zones, as GameInput
/// reports them. An empty slot is probed again only every <see cref="InputLimits.ProbeIntervalMicroseconds"/>.
/// </summary>
internal sealed unsafe class XInputGamepads : IGamepadSource
{
    private const float StickScale = short.MaxValue;
    private const float TriggerScale = byte.MaxValue;
    private const float MotorScale = ushort.MaxValue;

    private static readonly (ushort XInput, uint GameInput)[] ButtonMap =
    [
        (XInput.DPadUp, GameInputLayout.GamepadDPadUp), (XInput.DPadDown, GameInputLayout.GamepadDPadDown),
        (XInput.DPadLeft, GameInputLayout.GamepadDPadLeft), (XInput.DPadRight, GameInputLayout.GamepadDPadRight),
        (XInput.Start, GameInputLayout.GamepadMenu), (XInput.Back, GameInputLayout.GamepadView),
        (XInput.LeftThumb, GameInputLayout.GamepadLeftThumbstick), (XInput.RightThumb, GameInputLayout.GamepadRightThumbstick),
        (XInput.LeftShoulder, GameInputLayout.GamepadLeftShoulder), (XInput.RightShoulder, GameInputLayout.GamepadRightShoulder),
        (XInput.A, GameInputLayout.GamepadA), (XInput.B, GameInputLayout.GamepadB),
        (XInput.X, GameInputLayout.GamepadX), (XInput.Y, GameInputLayout.GamepadY),
    ];

    private readonly long[] _nextProbe = new long[InputLimits.XInputSlots];

    public int SlotCount => InputLimits.XInputSlots;

    [HotPath]
    public bool TryRead(int slot, long now, out GamepadState state)
    {
        state = default;

        if (now < _nextProbe[slot])
        {
            return false;
        }

        XInputState raw;

        if (XInput.XInputGetState((uint)slot, &raw) != Win32Constants.ErrorSuccess)
        {
            _nextProbe[slot] = now + InputLimits.ProbeIntervalMicroseconds;

            return false;
        }

        state.Buttons = Buttons(raw.Buttons);
        state.LeftTrigger = raw.LeftTrigger / TriggerScale;
        state.RightTrigger = raw.RightTrigger / TriggerScale;

        state.LeftX = Axis(raw.ThumbLeftX);
        state.LeftY = Axis(raw.ThumbLeftY);
        state.RightX = Axis(raw.ThumbRightX);
        state.RightY = Axis(raw.ThumbRightY);

        return true;
    }

    public void SetRumble(int slot, float lowFrequency, float highFrequency)
    {
        var vibration = new XInputVibration
        {
            LeftMotor = (ushort)(Math.Clamp(lowFrequency, 0f, 1f) * MotorScale),
            RightMotor = (ushort)(Math.Clamp(highFrequency, 0f, 1f) * MotorScale),
        };

        _ = XInput.XInputSetState((uint)slot, &vibration);
    }

    [HotPath]
    private static uint Buttons(ushort raw)
    {
        uint buttons = 0;

        foreach (var (xinput, gameInput) in ButtonMap)
        {
            if ((raw & xinput) != 0)
            {
                buttons |= gameInput;
            }
        }

        return buttons;
    }

    /// <summary>-32768 maps slightly past -1, so the value is clamped.</summary>
    [HotPath]
    private static float Axis(short value)
    {
        return Math.Clamp(value / StickScale, -1f, 1f);
    }
}
