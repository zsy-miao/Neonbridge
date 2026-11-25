// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>XAsyncOp (XAsyncProvider.h): what a provider is asked to do.</summary>
internal enum AsyncOperation : uint
{
    Begin = 0,
    DoWork = 1,
    GetResult = 2,
    Cancel = 3,
    Cleanup = 4,
}
