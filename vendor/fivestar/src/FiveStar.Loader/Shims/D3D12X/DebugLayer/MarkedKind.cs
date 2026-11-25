// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;
/// <summary>The kind of a <see cref="MarkedOperation"/>.</summary>
internal enum MarkedKind
{
    Command,
    Draw,
    Dispatch,
    Indirect,
}
