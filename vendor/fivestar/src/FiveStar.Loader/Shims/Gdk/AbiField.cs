// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>The fields of a line of an ABI table file: "clsid iid slot FunctionName".</summary>
internal enum AbiField
{
    Clsid = 0,
    Iid = 1,
    Slot = 2,
    Function = 3,
}
