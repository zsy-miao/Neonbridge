// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>A range as the game addresses it and the desktop address it starts at.</summary>
/// <param name="GameAddress">First byte as the game addresses it (a console GPU address or a descriptor's CPU address).</param>
/// <param name="Size">Bytes in the range.</param>
/// <param name="DesktopAddress">The desktop address of the first byte.</param>
/// <param name="Resource">The desktop buffer that starts at <paramref name="DesktopAddress"/>; 0 for descriptor ranges.</param>
internal readonly record struct AddressRange(ulong GameAddress, ulong Size, ulong DesktopAddress, nint Resource = 0);
