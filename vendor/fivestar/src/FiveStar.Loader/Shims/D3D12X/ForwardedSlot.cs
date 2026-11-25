// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>A D3D12.X vtable slot that tail-calls a desktop slot of the wrapped object.</summary>
/// <param name="Slot">The slot in the D3D12.X vtable.</param>
/// <param name="DesktopSlot">The slot in the desktop vtable (d3d12.h).</param>
/// <param name="StructReturn">The game passes the return buffer in rcx and the object in rdx; the thunk swaps them.</param>
/// <param name="Unwrap">Registers that may carry a wrapped object and are replaced by the desktop object.</param>
internal readonly record struct ForwardedSlot(int Slot, int DesktopSlot, bool StructReturn, ArgumentRegisters Unwrap);
