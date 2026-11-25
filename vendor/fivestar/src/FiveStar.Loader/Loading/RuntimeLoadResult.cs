// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Loading;

/// <summary>A LoadLibrary answered by the loader: the handle (0 on failure), the Win32 error and how it was handled.</summary>
internal sealed record RuntimeLoadResult(nint Handle, int Error, string How);
