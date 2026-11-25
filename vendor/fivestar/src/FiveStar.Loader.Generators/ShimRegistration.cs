// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Generators;

/// <summary>
/// One statement of a shim type's generated <c>RegisterShims</c>, with what places it: the declaring type and the
/// source position of the attributed declaration. Positional records need IsExternalInit, which netstandard2.0 lacks.
/// </summary>
internal sealed record ShimRegistration
{
    public ShimRegistration(ShimDeclaringType type, ShimRegistrationKind kind, string path, int position, string statement)
    {
        Type = type;
        Kind = kind;
        Path = path;
        Position = position;
        Statement = statement;
    }

    public ShimDeclaringType Type { get; }

    public ShimRegistrationKind Kind { get; }

    public string Path { get; }

    public int Position { get; }

    public string Statement { get; }
}
