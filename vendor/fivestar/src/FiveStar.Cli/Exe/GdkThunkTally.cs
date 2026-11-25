// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Exe;

/// <summary>A distinct GDK thunk found in the image and how many call sites reach it.</summary>
/// <param name="Name">The library name of the function, or null when no library entry names it.</param>
/// <param name="Clsid">The class id.</param>
/// <param name="Iid">The interface id.</param>
/// <param name="Slot">The vtable slot.</param>
/// <param name="Thunk">RVA of the (first) thunk function.</param>
/// <param name="Sites">Direct calls into the thunk functions.</param>
public sealed record GdkThunkTally(string? Name, Guid Clsid, Guid Iid, int Slot, uint Thunk, int Sites);
