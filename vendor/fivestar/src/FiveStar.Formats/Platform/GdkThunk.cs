// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>
/// One GDK API thunk: <c>QueryApiImpl(clsid, iid, &amp;object)</c> followed by a call through vtable slot
/// <paramref name="Slot"/> of the object.
/// </summary>
/// <param name="Clsid">The class id passed in rcx.</param>
/// <param name="Iid">The interface id passed in rdx.</param>
/// <param name="Slot">The vtable slot (byte offset / 8).</param>
/// <param name="Name">The public function name, or null when no library names it.</param>
public sealed record GdkThunk(Guid Clsid, Guid Iid, int Slot, string? Name);
