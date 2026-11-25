// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Exe;

/// <summary>One GDK function the game calls through a QueryApiImpl thunk, with its call-site count.</summary>
/// <param name="Name">The function name from the GDK library, or null when no library entry names it.</param>
/// <param name="Clsid">The class id of the thunk.</param>
/// <param name="Iid">The interface id of the thunk.</param>
/// <param name="Slot">The vtable slot.</param>
/// <param name="Thunk">RVA of the thunk function.</param>
/// <param name="Sites">Number of direct call sites.</param>
/// <param name="Shimmed">True when the loader has a shim for the function; null when it was not classified.</param>
public sealed record GdkCall(string? Name, string Clsid, string Iid, int Slot, uint Thunk, int Sites, bool? Shimmed);
