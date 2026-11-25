// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims;

/// <summary>One pagefile-backed section of physical pages and the view of all of it the store keeps mapped.</summary>
/// <param name="Section">The section handle; every mapping of a page in this chunk is a view of it.</param>
/// <param name="Window">A read-write view of the whole section, for zeroing freed pages and for the D3D12 shim.</param>
/// <param name="FirstPage">The store's index of the section's first page.</param>
/// <param name="Pages">How many pages the section holds.</param>
internal sealed record XMemPageChunk(nint Section, nint Window, ulong FirstPage, ulong Pages);
