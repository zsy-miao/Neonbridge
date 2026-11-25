// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>A registry value: name (empty for the default value), type number and raw data.</summary>
public sealed record RegistryValue(string Name, uint Type, byte[] Data)
{
    public const uint TypeNone = 0;
    public const uint TypeString = 1;
    public const uint TypeExpandString = 2;
    public const uint TypeBinary = 3;
    public const uint TypeDword = 4;
    public const uint TypeMultiString = 7;
    public const uint TypeQword = 11;
}
