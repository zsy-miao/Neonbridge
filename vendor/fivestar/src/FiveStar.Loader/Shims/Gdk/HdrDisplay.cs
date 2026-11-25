// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>What the PC display reports: whether the desktop runs in HDR, and its luminance range in nits.</summary>
internal readonly record struct HdrDisplay(bool Enabled, float MinLuminance, float MaxLuminance, float MaxFullFrameLuminance);
