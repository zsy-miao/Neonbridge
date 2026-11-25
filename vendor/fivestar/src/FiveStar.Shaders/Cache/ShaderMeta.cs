// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>The metadata entry of one cached shader.</summary>
/// <param name="Version">The u32 at 0: 2 for graphics shaders, 0x403 for compute shaders.</param>
/// <param name="UserData">Its tag 0x41 entries.</param>
/// <param name="Resources">Its tag 0x42 entries.</param>
/// <param name="Hash">The 16 bytes after tag 0x43 as hex (the last entry).</param>
public sealed record ShaderMeta(uint Version, IReadOnlyList<UserDataEntry> UserData, IReadOnlyList<ResourceEntry> Resources, string Hash);
