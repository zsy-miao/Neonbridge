// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Loading;

/// <summary>
/// Where a module comes from, as facts about this machine and the package: whether the PC host has it (a System32
/// file, or an API set the OS implements) and whether the package folder holds it. Xbox-only system modules
/// (acphal, xapu, xg_xs...) are in neither, which is how they are told apart without a list.
/// </summary>
internal static unsafe class HostModules
{
    private const string DllExtension = ".dll";

    private static readonly string[] ApiSetPrefixes = ["api-ms-", "ext-ms-"];

    /// <summary>Whether the PC host can load <paramref name="key"/> (a normalized import name).</summary>
    public static bool IsPresent(string key)
    {
        if (IsApiSet(key))
        {
            return IsApiSetImplemented(key);
        }

        return File.Exists(Path.Combine(Environment.SystemDirectory, key));
    }

    /// <summary>The file of <paramref name="key"/> in the first directory holding it (any case), or null.</summary>
    public static string? FindInDirectories(IEnumerable<string> directories, string key)
    {
        foreach (string directory in directories)
        {
            string file = Path.Combine(directory, key);

            if (File.Exists(file))
            {
                return file;
            }

            if (!Directory.Exists(directory))
            {
                continue;
            }

            var hit = Directory.EnumerateFiles(directory).FirstOrDefault(f => string.Equals(Path.GetFileName(f), key, StringComparison.OrdinalIgnoreCase));

            if (hit is not null)
            {
                return hit;
            }
        }

        return null;
    }

    private static bool IsApiSet(string key)
    {
        return ApiSetPrefixes.Any(prefix => key.StartsWith(prefix, StringComparison.OrdinalIgnoreCase));
    }

    private static bool IsApiSetImplemented(string key)
    {
        string contract = key;

        if (contract.EndsWith(DllExtension, StringComparison.OrdinalIgnoreCase))
        {
            contract = contract[..^DllExtension.Length];
        }

        byte[] ansi = Encoding.ASCII.GetBytes(contract + "\0");

        fixed (byte* pointer = ansi)
        {
            return KernelBase.IsApiSetImplemented(pointer) != 0;
        }
    }
}
