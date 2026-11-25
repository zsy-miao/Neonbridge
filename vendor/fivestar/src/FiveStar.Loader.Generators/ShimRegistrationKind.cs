// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Generators;

/// <summary>What a registration adds, in the order a type's registrations run.</summary>
internal enum ShimRegistrationKind
{
    /// <summary>A [ShimModule] of the type.</summary>
    Module,

    /// <summary>A [NotImplementedTable] field.</summary>
    NotImplementedTable,

    /// <summary>A method with one or more [Shim] attributes.</summary>
    Method,
}
