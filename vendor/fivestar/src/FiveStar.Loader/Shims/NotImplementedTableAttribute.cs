// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims;

/// <summary>
/// Marks a static readonly field of type <c>(string Name, long ReturnValue)[]</c> listing functions of a module that
/// are known but deliberately not implemented: each binds to a stub that returns the value and logs its first call.
/// </summary>
[AttributeUsage(AttributeTargets.Field)]
internal sealed class NotImplementedTableAttribute : Attribute
{
    public NotImplementedTableAttribute(string module)
    {
        Module = module;
    }

    public string Module { get; }
}
