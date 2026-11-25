// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>One call site of a GDK thunk: the thunk and the RVA of the <c>call</c> to <c>QueryApiImpl</c>.</summary>
/// <param name="Thunk">The (clsid, iid, slot) thunk the call site resolves to.</param>
/// <param name="Rva">RVA of the <c>call</c> instruction.</param>
public sealed record GdkThunkSite(GdkThunk Thunk, uint Rva);
