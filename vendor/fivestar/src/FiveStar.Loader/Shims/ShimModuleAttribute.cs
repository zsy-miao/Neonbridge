// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims;

/// <summary>
/// Declares a module that exists only on Xbox. Its imports never fall through to LoadLibrary: a function without a
/// [Shim] becomes a stub returning <see cref="UnresolvedReturn"/>.
/// </summary>
[AttributeUsage(AttributeTargets.Class, AllowMultiple = true)]
internal sealed class ShimModuleAttribute : Attribute
{
    public ShimModuleAttribute(string module)
    {
        Module = module;
    }

    public string Module { get; }

    /// <summary>What an unshimmed function of the module returns (E_NOTIMPL for an HRESULT-only API).</summary>
    public long UnresolvedReturn { get; set; }

    /// <summary>
    /// Other module names (a regular expression over the normalized lower-case name) this module answers for, for
    /// symbols starting with <see cref="AliasSymbolPrefix"/>: API-set names whose exact spelling is not known.
    /// </summary>
    public string? AliasPattern { get; set; }

    public string AliasSymbolPrefix { get; set; } = string.Empty;
}
