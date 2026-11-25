// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Diagnostics;

/// <summary>One frame of a native stack walk: the address, where it is, and whether unwind data was found for it.</summary>
internal readonly record struct StackFrame(ulong Pc, string Where, bool HasUnwindInfo);
