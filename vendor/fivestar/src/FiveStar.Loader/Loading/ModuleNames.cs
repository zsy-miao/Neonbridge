// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Loading;

/// <summary>Normalizes module names the way imports and LoadLibrary interpret them.</summary>
internal static class ModuleNames
{
    /// <summary>The module name that matches every importing module in the shim table.</summary>
    public const string AnyModule = "*";

    private const string DllExtension = ".dll";
    private const string ExeExtension = ".exe";

    /// <summary>Import-table key: lower case, ".dll" appended unless it ends in ".dll" or ".exe".</summary>
    public static string NormalizeImport(string module)
    {
        string name = module.Trim().ToLowerInvariant();
        bool complete = name == AnyModule || name.EndsWith(DllExtension, StringComparison.Ordinal) || name.EndsWith(ExeExtension, StringComparison.Ordinal);

        if (complete)
        {
            return name;
        }

        return name + DllExtension;
    }

    /// <summary>
    /// LoadLibrary/GetModuleHandle name: file name only, ".dll" appended when there is no extension, a trailing '.'
    /// meaning "no extension".
    /// </summary>
    public static string NormalizeRuntime(string name)
    {
        string trimmed = name.Trim().Replace('/', '\\');
        string file = trimmed;

        if (trimmed.Contains('\\', StringComparison.Ordinal))
        {
            file = Path.GetFileName(trimmed);
        }

        if (file.EndsWith('.'))
        {
            file = file.TrimEnd('.');
        }
        else if (!Path.HasExtension(file))
        {
            file += DllExtension;
        }

        return file.ToLowerInvariant();
    }

    /// <summary>A requested path completed like LoadLibrary does: ".dll" added without extension, a trailing '.' removed.</summary>
    public static string CompleteRuntimePath(string requested)
    {
        if (requested.EndsWith('.'))
        {
            return requested.TrimEnd('.');
        }

        if (Path.HasExtension(requested))
        {
            return requested;
        }

        return requested + DllExtension;
    }

    public static string SymbolKey(string? name, uint ordinal)
    {
        return name ?? $"#{ordinal}";
    }
}
