// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// One wrapped interface: its name in stub log lines, the vtable slots it provides, the bytes a wrapper takes and the
/// slots that forward to the desktop object. Every other slot is a logged stub until a handler replaces it.
/// </summary>
internal sealed record XboxInterface(XboxInterfaceKind Kind, string Name, int SlotCount, int ObjectSize, IReadOnlyList<ForwardedSlot> Forwards);
