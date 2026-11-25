// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>One status array entry: 1 once complete, and the first failure of the requests before it (or S_OK).</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct StatusEntry
{
    public int Complete;
    public int Result;
}
