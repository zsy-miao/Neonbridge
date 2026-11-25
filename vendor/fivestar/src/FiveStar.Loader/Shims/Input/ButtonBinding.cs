// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Input;

/// <summary>A virtual key that holds one gamepad button (a GameInputGamepadButtons bit); a null key is unbound.</summary>
internal readonly record struct ButtonBinding(uint Button, int? Key);
