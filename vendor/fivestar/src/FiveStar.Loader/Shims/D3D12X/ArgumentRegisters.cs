// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>Argument registers of a forwarded call that may hold a wrapped object and are unwrapped first.</summary>
[Flags]
internal enum ArgumentRegisters
{
    None = 0,
    Rdx = 1,
    R8 = 2,
    R9 = 4,
}
