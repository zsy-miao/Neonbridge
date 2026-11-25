// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Loading;

/// <summary>A found export: its address, or for a forwarded export the forwarder string (and address 0).</summary>
internal sealed record ExportTarget(nint Address, string? Forwarder);
