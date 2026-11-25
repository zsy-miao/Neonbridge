// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>A registry key with its values and subkeys in stored order.</summary>
public sealed record RegistryKey(string Name, IReadOnlyList<RegistryValue> Values, IReadOnlyList<RegistryKey> Subkeys);
