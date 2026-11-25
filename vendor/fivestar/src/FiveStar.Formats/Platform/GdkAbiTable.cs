// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Formats.Platform;

/// <summary>
/// The loader's thunk ABI table (<c>fivestar-loader --gdk-abi</c>): one <c>clsid iid slot function</c> line per named
/// library thunk; module thunks without a name are listed as comments.
/// </summary>
public static class GdkAbiTable
{
    /// <summary>Matches the thunks found in a module against the library by (clsid, iid, slot).</summary>
    public static GdkModuleCoverage Coverage(string module, IReadOnlyList<GdkThunk> library, IReadOnlyList<GdkThunk> found)
    {
        var named = library.Select(Key).ToHashSet();
        var unnamed = found.Where(thunk => !named.Contains(Key(thunk))).OrderBy(Key).ToList();

        return new GdkModuleCoverage(module, found.Count, unnamed);
    }

    public static void Write(TextWriter output, string libraryName, IReadOnlyList<GdkThunk> library, IReadOnlyList<GdkModuleCoverage> modules)
    {
        output.WriteLine($"# GDK thunk ABI from {libraryName}: clsid iid slot function");

        foreach (var thunk in library.DistinctBy(Key).OrderBy(Key))
        {
            output.WriteLine(Line(thunk, thunk.Name));
        }

        foreach (var module in modules)
        {
            output.WriteLine();
            output.WriteLine(string.Create(CultureInfo.InvariantCulture, $"# {module.Module}: {module.Thunks} thunks, {module.Unnamed.Count} without a name"));

            foreach (var thunk in module.Unnamed)
            {
                output.WriteLine("# " + Line(thunk, "?"));
            }
        }
    }

    private static string Line(GdkThunk thunk, string? name)
    {
        return string.Create(CultureInfo.InvariantCulture, $"{thunk.Clsid}  {thunk.Iid}  {thunk.Slot}  {name}");
    }

    private static (Guid Clsid, Guid Iid, int Slot) Key(GdkThunk thunk)
    {
        return (thunk.Clsid, thunk.Iid, thunk.Slot);
    }
}
