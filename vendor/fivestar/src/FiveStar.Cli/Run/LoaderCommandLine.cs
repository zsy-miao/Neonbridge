// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Cli.Run;

/// <summary>The <c>fivestar-loader</c> arguments that boot the game of a <see cref="RunPlan"/>.</summary>
internal static class LoaderCommandLine
{
    private const string AbiOption = "--gdk-abi";
    private const string TablesOption = "--bcpack-tables";
    private const string CacheOption = "--pipeline-cache";
    private const string NoCacheOption = "--no-pipeline-cache";
    private const string DxcOption = "dxc";

    /// <summary>
    /// Maps and runs the game executable with the work folder's shaders (translated on first use into the program cache,
    /// or ahead of time with --translate-ahead or prebuilt folders), pipeline cache, TLS folder, GDK table and BCPack tables, the full log in
    /// <paramref name="logFile"/>, then the extra loader arguments. An extra <c>--gdk-abi</c> replaces the work folder's table.
    /// </summary>
    public static IReadOnlyList<string> Build(RunPlan plan, string logFile)
    {
        var arguments = new List<string>
        {
            "--load", plan.Executable,
            "--no-exports",
            "--work-dir", plan.Context.TlsFolder,
            "--log", logFile,
            "--run",
        };

        if (plan.Options.Timeout is not null)
        {
            arguments.AddRange(["--run-timeout", plan.Options.Timeout.Value.ToString(CultureInfo.InvariantCulture)]);
        }

        arguments.AddRange(["--shader-dir", plan.ProgramFolder, "--program-cache", plan.Context.ProgramCacheFolder]);

        if (plan.Options.PrebuiltShaders || plan.Options.TranslateAhead)
        {
            arguments.AddRange(["--translated-dir", plan.TranslatedFolder]);
        }

        var dxc = plan.Options.Tools.Option(DxcOption);

        if (dxc is not null)
        {
            arguments.AddRange(["--dxc", Path.GetFullPath(dxc)]);
        }

        if (!plan.Options.LoaderArguments.Any(argument => argument is CacheOption or NoCacheOption))
        {
            arguments.AddRange([CacheOption, plan.Context.PipelineLibraryFolder]);
        }

        if (File.Exists(plan.Context.AbiTable) && !plan.Options.LoaderArguments.Contains(AbiOption))
        {
            arguments.AddRange([AbiOption, plan.Context.AbiTable]);
        }

        if (File.Exists(plan.Context.BcPackTables) && !plan.Options.LoaderArguments.Contains(TablesOption))
        {
            arguments.AddRange([TablesOption, plan.Context.BcPackTables]);
        }

        if (plan.Options.Debug)
        {
            arguments.Add("--d3d12-debug");
        }

        arguments.AddRange(plan.Options.LoaderArguments);

        return arguments;
    }
}
