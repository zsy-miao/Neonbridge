// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats;

/// <summary>
/// Marks a method that runs per block or per decoder clock of a texture page decode. Such a method allocates nothing
/// and never blocks: no lambda, LINQ, string building, array, <c>new</c> of a reference type, lock or await.
/// </summary>
[AttributeUsage(AttributeTargets.Method)]
internal sealed class HotPathAttribute : Attribute
{
}
