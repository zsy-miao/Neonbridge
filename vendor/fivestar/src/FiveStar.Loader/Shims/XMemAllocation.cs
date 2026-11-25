// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims;

/// <summary>A live XMemAlloc block: its requested size and whether it came from VirtualAlloc.</summary>
internal readonly record struct XMemAllocation(nuint Size, bool IsVirtual);
