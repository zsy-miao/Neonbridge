// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>The outcome of a managed async operation's work: its HRESULT and the size of its result.</summary>
internal readonly record struct AsyncWorkResult(int HResult, nuint Size);
