// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Archive;
using FiveStar.Formats.Platform;

namespace FiveStar.Cli.Platform;

/// <summary>
/// <c>fivestar platform gdk-abi &lt;xgameruntime.lib&gt; [module ...] [--out file]</c>: writes the loader's thunk ABI
/// table from the GDK library and reports, per module, how many of its thunks the table names.
/// </summary>
internal static class GdkAbiCommand
{
    private const int Success = 0;

    public static int Run(CommandArguments arguments)
    {
        var libraryPath = arguments.Positionals[0];
        var library = GdkThunkLibrary.Read(File.ReadAllBytes(libraryPath));
        var classes = library.Select(thunk => thunk.Clsid).ToHashSet();
        var modules = new List<GdkModuleCoverage>();

        foreach (var path in arguments.Positionals.Skip(1))
        {
            var found = GdkThunkScanner.Scan(new PeImage(File.ReadAllBytes(path)), classes);
            modules.Add(GdkAbiTable.Coverage(Path.GetFileName(path), library, found));
        }

        var outPath = arguments.Option("--out");

        if (outPath is null)
        {
            GdkAbiTable.Write(Console.Out, Path.GetFileName(libraryPath), library, modules);
        }
        else
        {
            using var file = new StreamWriter(outPath);
            GdkAbiTable.Write(file, Path.GetFileName(libraryPath), library, modules);
        }

        Console.Error.WriteLine($"{library.Count} named thunks in {Path.GetFileName(libraryPath)}");

        foreach (var module in modules)
        {
            Console.Error.WriteLine($"{module.Module}: {module.Thunks} thunks, {module.Thunks - module.Unnamed.Count} named, {module.Unnamed.Count} without a name");
        }

        return Success;
    }
}
