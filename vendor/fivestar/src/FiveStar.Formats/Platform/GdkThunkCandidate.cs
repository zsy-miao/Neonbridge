// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>A call with the shape of a GDK thunk, before the scanner knows whether its target is QueryApiImpl.</summary>
/// <param name="Thunk">The (clsid, iid, slot) the call would resolve to.</param>
/// <param name="Rva">RVA of the <c>call</c> instruction.</param>
/// <param name="Target">RVA the <c>call</c> goes to.</param>
/// <param name="ConstantGuids">True when both GUIDs sit in <c>.rdata</c>, where the library's GUID constants live.</param>
internal sealed record GdkThunkCandidate(GdkThunk Thunk, uint Rva, uint Target, bool ConstantGuids);
