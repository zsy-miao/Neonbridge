// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// Reads the sources and turns changes into readings. Runs on the game's own calls, at most once per
/// <see cref="InputLimits.PollIntervalMicroseconds"/>, and by one thread at a time: a thread that finds a poll
/// running uses the readings already taken.
/// </summary>
internal sealed class InputPoller
{
    private const uint ConnectedStatus = GameInputLayout.StatusConnected | GameInputLayout.StatusInputEnabled;
    private const uint OutputStatus = ConnectedStatus | GameInputLayout.StatusOutputEnabled;

    private static readonly (int Key, uint Button)[] MouseButtons =
    [
        (KeyNames.MouseLeft, GameInputLayout.MouseLeft), (KeyNames.MouseRight, GameInputLayout.MouseRight),
        (KeyNames.MouseMiddle, GameInputLayout.MouseMiddle), (KeyNames.MouseButton4, GameInputLayout.MouseButton4),
        (KeyNames.MouseButton5, GameInputLayout.MouseButton5),
    ];

    private readonly IGamepadSource _gamepads;
    private readonly IDesktopSource _desktop;
    private readonly InputDevices _devices;
    private readonly LoaderLog _log;
    private readonly PressedKeys _keys = new();
    private readonly int _keyboard;
    private readonly int _mouse;
    private readonly int _virtual;
    private volatile InputConfig _config;
    private volatile bool _virtualOn;
    private int _polling;
    private ulong _nextPoll;
    private ulong _lastPoll;
    private bool _cursorKnown;
    private CursorPosition _cursor;
    private long _mouseX;
    private long _mouseY;
    private bool _toggleHeld;

    public InputPoller(IGamepadSource gamepads, IDesktopSource desktop, InputDevices devices, InputConfig config, LoaderLog log)
    {
        _gamepads = gamepads;
        _desktop = desktop;
        _devices = devices;
        _config = config;
        _log = log;
        _keyboard = gamepads.SlotCount + InputDeviceCatalog.KeyboardAfterSlots;

        _mouse = gamepads.SlotCount + InputDeviceCatalog.MouseAfterSlots;
        _virtual = gamepads.SlotCount + InputDeviceCatalog.VirtualAfterSlots;
    }

    /// <summary>Connects the keyboard, the mouse and, when the config says so, the virtual gamepad.</summary>
    public void Start(ulong now)
    {
        _devices.SetStatus(_keyboard, ConnectedStatus, now);
        _devices.SetStatus(_mouse, ConnectedStatus, now);
        SetVirtual(_config.VirtualGamepad, now);
    }

    public void Configure(InputConfig config, ulong now)
    {
        _config = config;
        SetVirtual(config.VirtualGamepad, now);
    }

    [HotPath]
    public void Poll(ulong now)
    {
        if (now < Volatile.Read(ref _nextPoll) || Interlocked.CompareExchange(ref _polling, 1, 0) != 0)
        {
            return;
        }

        try
        {
            float seconds = 0;

            if (_lastPoll != 0)
            {
                seconds = (float)(now - _lastPoll) / InputLimits.MicrosecondsPerSecond;
            }

            _lastPoll = now;
            Volatile.Write(ref _nextPoll, now + InputLimits.PollIntervalMicroseconds);
            PollGamepads(now);
            PollDesktop(now, seconds);
        }
        finally
        {
            Volatile.Write(ref _polling, 0);
        }
    }

    [HotPath]
    private void PollGamepads(ulong now)
    {
        for (int slot = 0; slot < _gamepads.SlotCount; slot++)
        {
            if (!_gamepads.TryRead(slot, (long)now, out var state))
            {
                _devices.SetStatus(slot, GameInputLayout.StatusNone, now);

                continue;
            }

            _devices.SetStatus(slot, OutputStatus, now);
            _devices.PublishGamepad(slot, state, now);
        }
    }

    [HotPath]
    private void PollDesktop(ulong now, float seconds)
    {
        _desktop.ReadKeys(_keys);
        var cursor = _desktop.ReadCursor();
        long deltaX = 0;
        long deltaY = 0;

        if (_cursorKnown)
        {
            deltaX = cursor.X - _cursor.X;
            deltaY = cursor.Y - _cursor.Y;
        }

        _cursor = cursor;
        _cursorKnown = true;
        _mouseX += deltaX;
        _mouseY += deltaY;
        PublishKeyboard(now);
        PublishMouse(now);

        PollToggle(now);

        if (_virtualOn)
        {
            var state = VirtualGamepad.Map(_config, _keys, deltaX, deltaY, seconds);
            _devices.PublishGamepad(_virtual, state, now);
        }
    }

    [HotPath]
    private void PublishKeyboard(ulong now)
    {
        var keys = default(KeyBuffer);
        int count = 0;

        for (int key = 1; key < InputLimits.VirtualKeyCount && count < InputLimits.MaxKeys; key++)
        {
            if (KeyNames.IsKeyboardKey(key) && _keys.IsDown(key))
            {
                keys[count] = new KeyState { ScanCode = _desktop.ScanCode(key), VirtualKey = (byte)key };
                count++;
            }
        }

        _devices.PublishKeyboard(_keyboard, ref keys, count, now);
    }

    [HotPath]
    private void PublishMouse(ulong now)
    {
        var state = default(MouseState);
        state.PositionX = _mouseX;
        state.PositionY = _mouseY;

        foreach (var (key, button) in MouseButtons)
        {
            if (_keys.IsDown(key))
            {
                state.Buttons |= button;
            }
        }

        _devices.PublishMouse(_mouse, state, now);
    }

    [HotPath]
    private void PollToggle(ulong now)
    {
        bool held = _keys.IsDown(_config.ToggleKey);

        if (held && !_toggleHeld)
        {
            SetVirtual(!_virtualOn, now);
        }

        _toggleHeld = held;
    }

    private void SetVirtual(bool on, ulong now)
    {
        uint status = on ? ConnectedStatus : GameInputLayout.StatusNone;
        string change = on ? "connected" : "disconnected";

        if (on != _virtualOn)
        {
            _log.Info($"  [input] keyboard-and-mouse gamepad {change}");
        }

        _virtualOn = on;
        _devices.SetStatus(_virtual, status, now);
    }
}
