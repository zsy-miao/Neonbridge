// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims;

/// <summary>
/// Marks a static [UnmanagedCallersOnly] method as the implementation of an import. Module "*" matches every importing
/// module name, for functions that live in several API sets (RtlPcToFileHeader comes from kernel32, ntdll or an
/// api-ms-win-core set). Writing such a method is the whole registration.
/// </summary>
[AttributeUsage(AttributeTargets.Method, AllowMultiple = true)]
internal sealed class ShimAttribute : Attribute
{
    public ShimAttribute(string module, string name)
    {
        Module = module;
        Name = name;
    }

    public string Module { get; }

    public string Name { get; }

    /// <summary>
    /// The shim reports errors through the last-error bridge; it is entered through a generated wrapper that makes
    /// the value visible to the caller (at most 4 integer arguments).
    /// </summary>
    public bool SetsLastError { get; set; }
}
